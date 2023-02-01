use std::str::FromStr;

use samply_symbols::{
    debugid::DebugId,
    object::{self, Architecture, Object, ObjectSegment},
    relative_address_base, CodeId, FileAndPathHelper, LibraryInfo, SymbolManager,
};
use serde_json::json;
use yaxpeax_arch::{Arch, DecodeError, U8Reader};

use crate::asm::response_json::DecodedInstruction;

use self::response_json::Response;

mod request_json;
mod response_json;

#[derive(thiserror::Error, Debug)]
enum AsmError {
    #[error("Couldn't parse request: {0}")]
    ParseRequestErrorSerde(#[from] serde_json::error::Error),

    #[error("An error occurred when loading the binary: {0}")]
    LoadBinaryError(#[from] samply_symbols::Error),

    #[error("object parse error: {0}")]
    ObjectParseError(#[from] object::Error),

    #[error("The requested address was not found in any section in the binary.")]
    AddressNotFound,

    #[error("Could not read the requested address range from the section (might be out of bounds or the section might not have any bytes in the file)")]
    ByteRangeNotInSection,

    #[error("Unrecognized architecture {0:?}")]
    UnrecognizedArch(Architecture),
}

pub struct AsmApi<'a, 'h: 'a, H: FileAndPathHelper<'h>> {
    symbol_manager: &'a SymbolManager<'h, H>,
}

impl<'a, 'h: 'a, H: FileAndPathHelper<'h>> AsmApi<'a, 'h, H> {
    /// Create an [`AsmApi`] instance which uses the provided [`SymbolManager`].
    pub fn new(symbol_manager: &'a SymbolManager<'h, H>) -> Self {
        Self { symbol_manager }
    }

    pub async fn query_api_json(&self, request_json: &str) -> String {
        match self.query_api_fallible_json(request_json).await {
            Ok(response_json) => response_json,
            Err(err) => json!({ "error": err.to_string() }).to_string(),
        }
    }

    async fn query_api_fallible_json(&self, request_json: &str) -> Result<String, AsmError> {
        let request: request_json::Request = serde_json::from_str(request_json)?;
        let response = self.query_api(&request).await?;
        Ok(serde_json::to_string(&response)?)
    }

    async fn query_api(
        &self,
        request: &request_json::Request,
    ) -> Result<response_json::Response, AsmError> {
        let request_json::Request {
            debug_id,
            debug_name,
            name,
            code_id,
            start_address,
            size,
            continue_until_function_end,
            ..
        } = request;

        let debug_id = debug_id
            .as_deref()
            .and_then(|debug_id| DebugId::from_breakpad(debug_id).ok());
        let code_id = code_id
            .as_deref()
            .and_then(|code_id| CodeId::from_str(code_id).ok());

        let library_info = LibraryInfo {
            debug_name: debug_name.clone(),
            debug_id,
            name: name.clone(),
            code_id,
            ..Default::default()
        };

        let binary_image = self
            .symbol_manager
            .load_binary(&library_info)
            .await
            .map_err(AsmError::LoadBinaryError)?;

        let mut disassembly_len = *size;

        if *continue_until_function_end {
            if let Some(function_end_address) = self
                .get_function_end_address(&library_info, *start_address)
                .await
            {
                if function_end_address >= *start_address
                    && function_end_address - *start_address > *size
                {
                    disassembly_len = function_end_address - *start_address;
                }
            }
        }

        compute_response(&binary_image.make_object(), *start_address, disassembly_len)
    }

    async fn get_function_end_address(
        &self,
        library_info: &LibraryInfo,
        address_within_function: u32,
    ) -> Option<u32> {
        let symbol_map_res = self.symbol_manager.load_symbol_map(library_info).await;
        let symbol = symbol_map_res.ok()?.lookup(address_within_function)?.symbol;
        symbol.address.checked_add(symbol.size?)
    }
}

fn compute_response<'data: 'file, 'file>(
    object: &'file impl Object<'data, 'file>,
    start_address: u32,
    disassembly_len: u32,
) -> Result<response_json::Response, AsmError> {
    // Align the start address, for architectures with instruction alignment.
    // For example, on ARM, you might be looking for the instructions of a
    // function whose function symbol has address 0x2001. But this address is
    // really two pieces of information: 0x2000 is the address of the function's
    // first instruction (ARM instructions are two-byte aligned), and the 0x1 bit
    // is the "thumb" bit, meaning that the instructions need to be decoded
    // with the thumb decoder.
    let architecture = object.architecture();
    let relative_start_address = match architecture {
        Architecture::Aarch64 => start_address & !0b11,
        Architecture::Arm => start_address & !0b1,
        _ => start_address,
    };

    // Translate start_address from a "relative address" into an
    // SVMA ("stated virtual memory address").
    let image_base = relative_address_base(object);
    let start_svma = image_base + u64::from(relative_start_address);

    // Find the section and segment which contains our start_svma.
    use object::ObjectSection;
    let (section, section_end_svma) = object
        .sections()
        .find_map(|section| {
            let section_start_svma = section.address();
            let section_end_svma = section_start_svma.checked_add(section.size())?;
            if !(section_start_svma..section_end_svma).contains(&start_svma) {
                return None;
            }

            Some((section, section_end_svma))
        })
        .ok_or(AsmError::AddressNotFound)?;

    let segment = object.segments().find(|segment| {
        let segment_start_svma = segment.address();
        if let Some(segment_end_svma) = segment_start_svma.checked_add(segment.size()) {
            (segment_start_svma..segment_end_svma).contains(&start_svma)
        } else {
            false
        }
    });

    // Pad out the number of bytes we read a little, to allow for reading one
    // more instruction.
    // We've been asked to decode the instructions whose instruction addresses
    // are in the range start_svma .. (start_svma + size). If the end of
    // this range points into the middle of an instruction, we still want to
    // decode the entire instruction, so we need all of its bytes.
    // We have another check later to make sure we don't return instructions whose
    // address is beyond the requested range.
    const MAX_INSTR_LEN: u64 = 15; // TODO: Get the correct max length for this arch
    let max_read_len = section_end_svma - start_svma;
    let read_len = (u64::from(disassembly_len) + MAX_INSTR_LEN).min(max_read_len);

    // Now read the instruction bytes from the file.
    let bytes = if let Some(segment) = segment {
        segment
            .data_range(start_svma, read_len)?
            .ok_or(AsmError::ByteRangeNotInSection)?
    } else {
        // We don't have a segment, try reading via the section.
        // We hit this path with synthetic .so files created by `perf inject --jit`;
        // those only have sections, no segments (i.e. no ELF LOAD commands).
        // For regular files, we prefer to read the data via the segment, because
        // the segment is more likely to have correct file offset information.
        // Specifically, incorrect section file offset information was observed in
        // the arm64e dyld cache on macOS 13.0.1, FB11929250.
        section
            .data_range(start_svma, read_len)?
            .ok_or(AsmError::ByteRangeNotInSection)?
    };

    let reader = yaxpeax_arch::U8Reader::new(bytes);
    decode_arch(
        reader,
        architecture,
        image_base,
        start_svma,
        disassembly_len,
    )
}

fn decode_arch(
    reader: U8Reader,
    arch: Architecture,
    image_base: u64,
    start_svma: u64,
    decode_len: u32,
) -> Result<Response, AsmError> {
    Ok(match arch {
        Architecture::I386 => {
            decode::<yaxpeax_x86::protected_mode::Arch>(reader, image_base, start_svma, decode_len)
        }
        Architecture::X86_64 => {
            decode::<yaxpeax_x86::amd64::Arch>(reader, image_base, start_svma, decode_len)
        }
        Architecture::Aarch64 => {
            decode::<yaxpeax_arm::armv8::a64::ARMv8>(reader, image_base, start_svma, decode_len)
        }
        Architecture::Arm => {
            decode::<yaxpeax_arm::armv7::ARMv7>(reader, image_base, start_svma, decode_len)
        }
        _ => return Err(AsmError::UnrecognizedArch(arch)),
    })
}

trait InstructionDecoding: Arch {
    const ARCH_NAME: &'static str;
    const SYNTAX: &'static [&'static str];
    fn make_decoder() -> Self::Decoder;
    fn stringify_inst(offset: u32, inst: Self::Instruction) -> DecodedInstruction;
}

impl InstructionDecoding for yaxpeax_x86::amd64::Arch {
    const ARCH_NAME: &'static str = "x86_64";
    const SYNTAX: &'static [&'static str] = &["Intel", "C style"];

    fn make_decoder() -> Self::Decoder {
        yaxpeax_x86::amd64::InstDecoder::default()
    }

    fn stringify_inst(offset: u32, inst: Self::Instruction) -> DecodedInstruction {
        DecodedInstruction {
            offset,
            decoded_string_per_syntax: vec![
                inst.display_with(yaxpeax_x86::amd64::DisplayStyle::Intel)
                    .to_string(),
                inst.display_with(yaxpeax_x86::amd64::DisplayStyle::C)
                    .to_string(),
            ],
        }
    }
}

impl InstructionDecoding for yaxpeax_x86::protected_mode::Arch {
    const ARCH_NAME: &'static str = "i686";
    const SYNTAX: &'static [&'static str] = &["Intel"];

    fn make_decoder() -> Self::Decoder {
        yaxpeax_x86::protected_mode::InstDecoder::default()
    }

    fn stringify_inst(offset: u32, inst: Self::Instruction) -> DecodedInstruction {
        DecodedInstruction {
            offset,
            decoded_string_per_syntax: vec![inst.to_string()],
        }
    }
}

impl InstructionDecoding for yaxpeax_arm::armv8::a64::ARMv8 {
    const ARCH_NAME: &'static str = "aarch64";
    const SYNTAX: &'static [&'static str] = &["ARM"];

    fn make_decoder() -> Self::Decoder {
        yaxpeax_arm::armv8::a64::InstDecoder::default()
    }

    fn stringify_inst(offset: u32, inst: Self::Instruction) -> DecodedInstruction {
        DecodedInstruction {
            offset,
            decoded_string_per_syntax: vec![inst.to_string()],
        }
    }
}

impl InstructionDecoding for yaxpeax_arm::armv7::ARMv7 {
    const ARCH_NAME: &'static str = "arm";
    const SYNTAX: &'static [&'static str] = &["ARM"];

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

    fn stringify_inst(offset: u32, inst: Self::Instruction) -> DecodedInstruction {
        DecodedInstruction {
            offset,
            decoded_string_per_syntax: vec![inst.to_string()],
        }
    }
}

fn decode<'a, A: InstructionDecoding>(
    mut reader: U8Reader<'a>,
    image_base: u64,
    start_svma: u64,
    decode_len: u32,
) -> Response
where
    u64: From<A::Address>,
    U8Reader<'a>: yaxpeax_arch::Reader<A::Address, A::Word>,
{
    use yaxpeax_arch::Decoder;
    let decoder = A::make_decoder();
    let mut instructions = Vec::new();
    loop {
        let offset = u64::from(yaxpeax_arch::Reader::<A::Address, A::Word>::total_offset(
            &mut reader,
        )) as u32;
        if offset >= decode_len {
            break;
        }
        match decoder.decode(&mut reader) {
            Ok(inst) => {
                instructions.push(A::stringify_inst(offset, inst));
            }
            Err(e) => {
                if !e.data_exhausted() {
                    // If decoding encountered an error, append a fake "!!! ERROR" instruction
                    instructions.push(DecodedInstruction {
                        offset,
                        decoded_string_per_syntax: A::SYNTAX
                            .iter()
                            .map(|_| format!("!!! ERROR: {}", e))
                            .collect(),
                    });
                }
                break;
            }
        }
    }
    let final_offset = u64::from(yaxpeax_arch::Reader::<A::Address, A::Word>::total_offset(
        &mut reader,
    )) as u32;

    Response {
        start_address: (start_svma - image_base) as u32,
        size: final_offset,
        arch: A::ARCH_NAME.to_string(),
        syntax: A::SYNTAX.iter().map(ToString::to_string).collect(),
        instructions,
    }
}
