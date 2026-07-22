use std::str::FromStr;

use samply_debugid::CodeId;
use samply_symbols::debugid::DebugId;
use samply_symbols::{
    object, BinaryImage, CodeByteReadingError, FileLoadError, FileTypes, LibraryInfo,
    LookupAddress, SymbolMap,
};
use yaxpeax_arch::{Arch, DecodeError, LengthedInstruction, Reader, U8Reader};

use self::response_json::Response;
use crate::asm::response_json::DecodedInstruction;
use crate::query_state::{ApiQueryState, ApiStep};
use crate::QueryApiJsonResult;

pub mod request_json;
pub mod response_json;

#[derive(thiserror::Error, Debug)]
pub enum AsmError {
    #[error("An error occurred when loading the binary: {0}")]
    LoadBinaryError(#[from] samply_symbols::Error),

    #[error("object parse error: {0}")]
    ObjectParseError(#[from] object::Error),

    #[error("The requested address was not found in any section in the binary.")]
    AddressNotFound,

    #[error("Could not read the requested address range from the section (might be out of bounds or the section might not have any bytes in the file)")]
    ByteRangeNotInSection,

    #[error("Unrecognized architecture {0:?}")]
    UnrecognizedArch(String),

    #[error("Could not read the requested address range from the file: {0}")]
    FileIO(#[from] FileLoadError),
}

const MAX_INSTR_LEN: u32 = 15; // TODO: Get the correct max length for this arch

/// Sans-IO state-machine implementation of `/asm/v1`.
pub struct AsmApiQueryState<FT: FileTypes> {
    state: AsmState<FT>,
}

enum AsmState<FT: FileTypes> {
    AwaitingBinary {
        library_info: LibraryInfo,
        start_address: u32,
        size: u32,
        continue_until_function_end: bool,
    },
    /// Optional symbol map fetch used to extend `disassembly_len` to the end
    /// of the symbol containing `start_address`.
    AwaitingFunctionEndSymbolMap {
        library_info: LibraryInfo,
        binary_image: BinaryImage<FT::F>,
        start_address: u32,
        size: u32,
    },
    /// Final symbol map fetch used to annotate branch targets.
    AwaitingBranchTargetSymbolMap {
        binary_image: BinaryImage<FT::F>,
        rel_address: u32,
        disassembly_len: u32,
        architecture: Option<String>,
    },
    Done(Result<response_json::Response, AsmError>),
    Poisoned,
}

impl<FT: FileTypes> AsmApiQueryState<FT> {
    pub fn from_request_json(request_json: &str) -> Result<Self, crate::Error> {
        let request: request_json::Request = serde_json::from_str(request_json)?;
        Ok(Self::new(&request))
    }

    pub fn new(request: &request_json::Request) -> Self {
        let debug_id = request
            .debug_id
            .as_deref()
            .and_then(|debug_id| DebugId::from_breakpad(debug_id).ok());
        let code_id = request
            .code_id
            .as_deref()
            .and_then(|code_id| CodeId::from_str(code_id).ok());
        let library_info = LibraryInfo {
            debug_name: request.debug_name.clone(),
            debug_id,
            name: request.name.clone(),
            code_id,
            ..Default::default()
        };
        Self {
            state: AsmState::AwaitingBinary {
                library_info,
                start_address: request.start_address,
                size: request.size,
                continue_until_function_end: request.continue_until_function_end,
            },
        }
    }
}

impl<FT: FileTypes> ApiQueryState<FT> for AsmApiQueryState<FT> {
    fn poll(&self) -> ApiStep<FT> {
        match &self.state {
            AsmState::AwaitingBinary { library_info, .. } => {
                ApiStep::NeedBinary(library_info.clone())
            }
            AsmState::AwaitingFunctionEndSymbolMap { library_info, .. } => {
                ApiStep::NeedSymbolMap(library_info.clone())
            }
            AsmState::AwaitingBranchTargetSymbolMap { binary_image, .. } => {
                ApiStep::NeedSymbolMap(binary_image.library_info())
            }
            AsmState::Done(_) => ApiStep::Done,
            AsmState::Poisoned => unreachable!("invalid AsmApiQueryState state"),
        }
    }

    fn provide_symbol_map(&mut self, result: Result<SymbolMap<FT>, samply_symbols::Error>) {
        let state = std::mem::replace(&mut self.state, AsmState::Poisoned);
        match state {
            AsmState::AwaitingFunctionEndSymbolMap {
                library_info,
                binary_image,
                start_address,
                size,
            } => {
                let mut disassembly_len = size;
                if let Ok(symbol_map) = result {
                    if let Some(end_address) = function_end_address(&symbol_map, start_address) {
                        if end_address >= start_address && end_address - start_address > size {
                            disassembly_len = end_address - start_address;
                        }
                    }
                }
                let _ = library_info;
                self.transition_to_branch_target_phase(
                    binary_image,
                    start_address,
                    disassembly_len,
                );
            }
            AsmState::AwaitingBranchTargetSymbolMap {
                binary_image,
                rel_address,
                disassembly_len,
                architecture,
            } => {
                let symbol_map = result.ok();
                self.state = AsmState::Done(decode_with(
                    &binary_image,
                    architecture.as_deref(),
                    rel_address,
                    disassembly_len,
                    symbol_map.as_ref(),
                ));
            }
            _ => panic!("provide_symbol_map called in unexpected state"),
        }
    }

    fn provide_source_file(&mut self, _result: Result<String, samply_symbols::Error>) {
        panic!("asm query never asks for a source file");
    }

    fn provide_file(&mut self, _result: Result<FT::F, samply_symbols::FileLoadError>) {
        panic!("asm query never asks for a raw file");
    }

    fn provide_binary(&mut self, result: Result<BinaryImage<FT::F>, samply_symbols::Error>) {
        let state = std::mem::replace(&mut self.state, AsmState::Poisoned);
        let AsmState::AwaitingBinary {
            library_info,
            start_address,
            size,
            continue_until_function_end,
        } = state
        else {
            panic!("provide_binary called when not awaiting a binary");
        };
        let binary_image = match result {
            Ok(b) => b,
            Err(e) => {
                self.state = AsmState::Done(Err(AsmError::LoadBinaryError(e)));
                return;
            }
        };
        if continue_until_function_end {
            self.state = AsmState::AwaitingFunctionEndSymbolMap {
                library_info,
                binary_image,
                start_address,
                size,
            };
        } else {
            self.transition_to_branch_target_phase(binary_image, start_address, size);
        }
    }

    fn finish(self: Box<Self>) -> QueryApiJsonResult<FT> {
        match self.state {
            AsmState::Done(Ok(response)) => QueryApiJsonResult::AsmResponse(response),
            AsmState::Done(Err(e)) => QueryApiJsonResult::Err(crate::Error::Asm(e)),
            _ => panic!("AsmApiQueryState::finish called before reaching Done"),
        }
    }
}

impl<FT: FileTypes> AsmApiQueryState<FT> {
    fn transition_to_branch_target_phase(
        &mut self,
        binary_image: BinaryImage<FT::F>,
        start_address: u32,
        disassembly_len: u32,
    ) {
        let architecture = binary_image.arch().map(str::to_owned);
        let rel_address = match architecture.as_deref() {
            Some("arm64" | "arm64e") => start_address & !0b11,
            Some("arm") => start_address & !0b1,
            _ => start_address,
        };
        self.state = AsmState::AwaitingBranchTargetSymbolMap {
            binary_image,
            rel_address,
            disassembly_len,
            architecture,
        };
    }
}

fn function_end_address<FT: FileTypes>(
    symbol_map: &SymbolMap<FT>,
    address_within_function: u32,
) -> Option<u32> {
    let info = symbol_map.lookup_sync(LookupAddress::Relative(address_within_function))?;
    info.symbol.address.checked_add(info.symbol.size?)
}

fn decode_with<FT: FileTypes>(
    binary_image: &BinaryImage<FT::F>,
    arch: Option<&str>,
    rel_address: u32,
    disassembly_len: u32,
    symbol_map: Option<&SymbolMap<FT>>,
) -> Result<response_json::Response, AsmError> {
    let bytes = binary_image
        .read_bytes_at_relative_address(rel_address, disassembly_len + MAX_INSTR_LEN)
        .map_err(|e| match e {
            CodeByteReadingError::AddressNotFound => AsmError::AddressNotFound,
            CodeByteReadingError::ObjectParseError(e) => AsmError::ObjectParseError(e),
            CodeByteReadingError::ByteRangeNotInSection => AsmError::ByteRangeNotInSection,
            CodeByteReadingError::FileIO(e) => AsmError::FileIO(e),
        })?;
    decode_arch::<FT>(bytes, arch, rel_address, disassembly_len, symbol_map)
}

fn decode_arch<FT: FileTypes>(
    bytes: &[u8],
    arch: Option<&str>,
    rel_address: u32,
    decode_len: u32,
    symbol_map: Option<&samply_symbols::SymbolMap<FT>>,
) -> Result<Response, AsmError> {
    Ok(match arch {
        Some("x86") => decode::<yaxpeax_x86::protected_mode::Arch, FT>(
            bytes,
            rel_address,
            decode_len,
            symbol_map,
        ),
        Some("x86_64" | "x86_64h") => {
            decode::<yaxpeax_x86::amd64::Arch, FT>(bytes, rel_address, decode_len, symbol_map)
        }
        Some("arm64" | "arm64e") => {
            decode::<yaxpeax_arm::armv8::a64::ARMv8, FT>(bytes, rel_address, decode_len, symbol_map)
        }
        Some("arm") => {
            decode::<yaxpeax_arm::armv7::ARMv7, FT>(bytes, rel_address, decode_len, symbol_map)
        }
        _ => {
            return Err(AsmError::UnrecognizedArch(
                arch.map_or_else(|| "unknown".to_string(), |a| a.to_string()),
            ))
        }
    })
}

/// Formats a branch target address with symbol information if available.
/// Returns a string like "<symbol_name>" or "<symbol_name+0x123>".
fn format_branch_symbol<FT: FileTypes>(
    target_address: u32,
    symbol_map: Option<&samply_symbols::SymbolMap<FT>>,
) -> Option<String> {
    let symbol_map = symbol_map?;
    let lookup_result = symbol_map.lookup_sync(LookupAddress::Relative(target_address))?;
    let symbol_name = symbol_map.resolve_symbol_name(lookup_result.symbol.name);

    // Check if we're at the function start or inside it.
    Some(if lookup_result.symbol.address == target_address {
        format!("<{}>", symbol_name)
    } else {
        let offset = target_address - lookup_result.symbol.address;
        format!("<{}+0x{:x}>", symbol_name, offset)
    })
}

trait InstructionDecoding: Arch {
    const ARCH_NAME: &'static str;
    const SYNTAX: &'static [&'static str];
    const ADJUST_BY_AFTER_ERROR: usize;
    fn make_decoder() -> Self::Decoder;
    fn stringify_inst<FT: FileTypes>(
        rel_address: u32,
        offset: u32,
        inst: Self::Instruction,
        symbol_map: Option<&samply_symbols::SymbolMap<FT>>,
    ) -> DecodedInstruction;
}

impl InstructionDecoding for yaxpeax_x86::amd64::Arch {
    const ARCH_NAME: &'static str = "x86_64";
    const SYNTAX: &'static [&'static str] = &["Intel", "C style"];
    const ADJUST_BY_AFTER_ERROR: usize = 1;

    fn make_decoder() -> Self::Decoder {
        yaxpeax_x86::amd64::InstDecoder::default()
    }

    fn stringify_inst<FT: FileTypes>(
        rel_address: u32,
        offset: u32,
        inst: Self::Instruction,
        symbol_map: Option<&samply_symbols::SymbolMap<FT>>,
    ) -> DecodedInstruction {
        use yaxpeax_x86::amd64::{Opcode, Operand};

        let (mut intel_insn, mut c_insn) = (
            inst.display_with(yaxpeax_x86::amd64::DisplayStyle::Intel)
                .to_string(),
            inst.display_with(yaxpeax_x86::amd64::DisplayStyle::C)
                .to_string(),
        );

        let is_relative_branch = matches!(
            inst.opcode(),
            Opcode::JMP
                | Opcode::JRCXZ
                | Opcode::LOOP
                | Opcode::LOOPZ
                | Opcode::LOOPNZ
                | Opcode::JO
                | Opcode::JNO
                | Opcode::JB
                | Opcode::JNB
                | Opcode::JZ
                | Opcode::JNZ
                | Opcode::JNA
                | Opcode::JA
                | Opcode::JS
                | Opcode::JNS
                | Opcode::JP
                | Opcode::JNP
                | Opcode::JL
                | Opcode::JGE
                | Opcode::JLE
                | Opcode::JG
                | Opcode::CALL
        );

        if is_relative_branch {
            match inst.operand(0) {
                Operand::ImmediateI8 { imm } => {
                    let rel = imm;
                    let dest = rel_address as i64
                        + offset as i64
                        + inst.len().to_const() as i64
                        + rel as i64;
                    intel_insn = format!("{} 0x{:x}", inst.opcode(), dest);
                    if let Some(symbol_info) = format_branch_symbol(dest as u32, symbol_map) {
                        intel_insn = format!("{} {}", intel_insn, symbol_info);
                    }
                    c_insn.clone_from(&intel_insn);
                }
                Operand::ImmediateI32 { imm } => {
                    let rel = imm;
                    let dest = rel_address as i64
                        + offset as i64
                        + inst.len().to_const() as i64
                        + rel as i64;
                    intel_insn = format!("{} 0x{:x}", inst.opcode(), dest);
                    if let Some(symbol_info) = format_branch_symbol(dest as u32, symbol_map) {
                        intel_insn = format!("{} {}", intel_insn, symbol_info);
                    }
                    c_insn.clone_from(&intel_insn);
                }
                _ => {}
            };
        }

        DecodedInstruction {
            offset,
            decoded_string_per_syntax: vec![intel_insn, c_insn],
        }
    }
}

impl InstructionDecoding for yaxpeax_x86::protected_mode::Arch {
    const ARCH_NAME: &'static str = "i686";
    const SYNTAX: &'static [&'static str] = &["Intel"];
    const ADJUST_BY_AFTER_ERROR: usize = 1;

    fn make_decoder() -> Self::Decoder {
        yaxpeax_x86::protected_mode::InstDecoder::default()
    }

    fn stringify_inst<FT: FileTypes>(
        _rel_address: u32,
        offset: u32,
        inst: Self::Instruction,
        _symbol_map: Option<&samply_symbols::SymbolMap<FT>>,
    ) -> DecodedInstruction {
        // TODO: Extract branch targets for x86 protected mode if needed.
        DecodedInstruction {
            offset,
            decoded_string_per_syntax: vec![inst.to_string()],
        }
    }
}

impl InstructionDecoding for yaxpeax_arm::armv8::a64::ARMv8 {
    const ARCH_NAME: &'static str = "aarch64";
    const SYNTAX: &'static [&'static str] = &["ARM"];
    const ADJUST_BY_AFTER_ERROR: usize = 4;

    fn make_decoder() -> Self::Decoder {
        yaxpeax_arm::armv8::a64::InstDecoder::default()
    }

    fn stringify_inst<FT: FileTypes>(
        rel_address: u32,
        offset: u32,
        inst: Self::Instruction,
        symbol_map: Option<&samply_symbols::SymbolMap<FT>>,
    ) -> DecodedInstruction {
        use yaxpeax_arm::armv8::a64::{Opcode, Operand};

        let mut inst_str = inst.to_string();

        let is_relative_branch = matches!(
            inst.opcode,
            Opcode::B
                | Opcode::BL
                | Opcode::Bcc(_)
                | Opcode::BCcc(_)
                | Opcode::CBZ
                | Opcode::CBNZ
                | Opcode::TBZ
                | Opcode::TBNZ
        );

        if is_relative_branch {
            // Extract the branch target from the instruction operands.
            // Different branch types have the target in different operand positions:
            // - B, BL, Bcc: operand 0 is the offset.
            // - CBZ, CBNZ: operand 1 is the offset (operand 0 is the register).
            // - TBZ, TBNZ: operand 2 is the offset (operands 0-1 are register and bit).
            let operand_index = match inst.opcode {
                Opcode::TBZ | Opcode::TBNZ => 2,
                Opcode::CBZ | Opcode::CBNZ => 1,
                _ => 0,
            };

            if let Operand::PCOffset(imm) = inst.operands[operand_index] {
                // PC-relative offset in bytes.
                // Unlike ARM32 BranchOffset/BranchThumbOffset, yaxpeax-arm returns ARM64
                // PCOffset values already shifted, not as instruction units.
                let dest = rel_address as i64 + offset as i64 + imm;

                // Format the instruction with the absolute address.
                inst_str = match inst.opcode {
                    Opcode::TBZ | Opcode::TBNZ => {
                        format!(
                            "{} {}, {}, 0x{:x}",
                            inst.opcode, inst.operands[0], inst.operands[1], dest
                        )
                    }
                    Opcode::CBZ | Opcode::CBNZ => {
                        format!("{} {}, 0x{:x}", inst.opcode, inst.operands[0], dest)
                    }
                    _ => {
                        format!("{} 0x{:x}", inst.opcode, dest)
                    }
                };

                // Add symbol information if available.
                if let Some(symbol_info) = format_branch_symbol(dest as u32, symbol_map) {
                    inst_str = format!("{} {}", inst_str, symbol_info);
                }
            }
        }

        DecodedInstruction {
            offset,
            decoded_string_per_syntax: vec![inst_str],
        }
    }
}

impl InstructionDecoding for yaxpeax_arm::armv7::ARMv7 {
    const ARCH_NAME: &'static str = "arm";
    const SYNTAX: &'static [&'static str] = &["ARM"];
    const ADJUST_BY_AFTER_ERROR: usize = 2;

    fn make_decoder() -> Self::Decoder {
        // TODO: Detect whether the instructions in the requested address range
        // use thumb or non-thumb mode.
        // I'm not quite sure how to do this. The same object file can contain both
        // types of code in different functions. We basically have two options:
        //  1. Have the API caller tell us whether to use thumb, or
        //  2. Detect the mode based on the content in the file.
        // For 2., we could look up the closest symbol to the start address and
        // check whether its symbol address has the thumb bit set. But the function
        // may not have a symbol in the binary that we have access to here.
        //
        // For now we just always assume thumb.
        yaxpeax_arm::armv7::InstDecoder::default_thumb()
    }

    fn stringify_inst<FT: FileTypes>(
        rel_address: u32,
        offset: u32,
        inst: Self::Instruction,
        symbol_map: Option<&samply_symbols::SymbolMap<FT>>,
    ) -> DecodedInstruction {
        use yaxpeax_arm::armv7::{ConditionedOpcode, Opcode, Operand};
        let mut inst_str = inst.to_string();

        let is_relative_branch = matches!(inst.opcode, Opcode::B | Opcode::BL | Opcode::BLX);

        if is_relative_branch {
            match inst.operands[0] {
                Operand::BranchThumbOffset(imm) => {
                    // For Thumb mode, the immediate is left-shifted by 1.
                    // Thumb instructions are 2-byte aligned.
                    let byte_offset = imm << 1;
                    let dest = rel_address as i64 + offset as i64 + byte_offset as i64;
                    let opcode =
                        ConditionedOpcode(inst.opcode, inst.s, inst.thumb_w, inst.condition);
                    inst_str = format!("{} 0x{:x}", opcode, dest);
                    if let Some(symbol_info) = format_branch_symbol(dest as u32, symbol_map) {
                        inst_str = format!("{} {}", inst_str, symbol_info);
                    }
                }
                Operand::BranchOffset(imm) => {
                    // For ARM mode (non-Thumb), the immediate is left-shifted by 2.
                    // ARM instructions are 4-byte aligned.
                    let byte_offset = imm << 2;
                    let dest = rel_address as i64 + offset as i64 + byte_offset as i64;
                    let opcode =
                        ConditionedOpcode(inst.opcode, inst.s, inst.thumb_w, inst.condition);
                    inst_str = format!("{} 0x{:x}", opcode, dest);
                    if let Some(symbol_info) = format_branch_symbol(dest as u32, symbol_map) {
                        inst_str = format!("{} {}", inst_str, symbol_info);
                    }
                }
                _ => {}
            }
        }

        DecodedInstruction {
            offset,
            decoded_string_per_syntax: vec![inst_str],
        }
    }
}

fn decode<'a, A: InstructionDecoding, FT: FileTypes>(
    bytes: &'a [u8],
    rel_address: u32,
    decode_len: u32,
    symbol_map: Option<&samply_symbols::SymbolMap<FT>>,
) -> Response
where
    u64: From<A::Address>,
    U8Reader<'a>: yaxpeax_arch::Reader<A::Address, A::Word>,
{
    use yaxpeax_arch::Decoder;
    let mut reader = yaxpeax_arch::U8Reader::new(bytes);
    let decoder = A::make_decoder();
    let mut instructions = Vec::new();
    let mut offset = 0;
    loop {
        if offset >= decode_len {
            break;
        }
        let before = u64::from(reader.total_offset()) as u32;
        match decoder.decode(&mut reader) {
            Ok(inst) => {
                instructions.push(A::stringify_inst(rel_address, offset, inst, symbol_map));
                let after = u64::from(reader.total_offset()) as u32;
                offset += after - before;
            }
            Err(e) => {
                if e.data_exhausted() {
                    break;
                }

                let remaining_bytes = &bytes[offset as usize..];
                let s = remaining_bytes
                    .iter()
                    .take(A::ADJUST_BY_AFTER_ERROR)
                    .map(|b| format!("{b:#04x}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                let s2 = remaining_bytes
                    .iter()
                    .take(A::ADJUST_BY_AFTER_ERROR)
                    .map(|b| format!("{b:02X}"))
                    .collect::<Vec<_>>()
                    .join(" ");

                instructions.push(DecodedInstruction {
                    offset,
                    decoded_string_per_syntax: A::SYNTAX
                        .iter()
                        .map(|_| {
                            format!(
                                ".byte {s:width$} # Invalid instruction {s2}: {e}",
                                width = A::ADJUST_BY_AFTER_ERROR * 6
                            )
                        })
                        .collect(),
                });

                offset += A::ADJUST_BY_AFTER_ERROR as u32;
                let Some(reader_bytes) = bytes.get(offset as usize..) else {
                    break;
                };
                reader = U8Reader::new(reader_bytes);
            }
        }
    }
    let final_offset = u64::from(yaxpeax_arch::Reader::<A::Address, A::Word>::total_offset(
        &mut reader,
    )) as u32;

    Response {
        start_address: rel_address,
        size: final_offset,
        arch: A::ARCH_NAME.to_string(),
        syntax: A::SYNTAX.iter().map(ToString::to_string).collect(),
        instructions,
    }
}
