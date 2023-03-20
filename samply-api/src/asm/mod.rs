use std::str::FromStr;

use samply_symbols::{
    debugid::DebugId, object, CodeByteReadingError, CodeId, FileAndPathHelper,
    FileAndPathHelperError, LibraryInfo, SymbolManager,
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
    UnrecognizedArch(String),

    #[error("Could not read the requested address range from the file: {0}")]
    FileIO(#[from] FileAndPathHelperError),
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

        // Align the start address, for architectures with instruction alignment.
        // For example, on ARM, you might be looking for the instructions of a
        // function whose function symbol has address 0x2001. But this address is
        // really two pieces of information: 0x2000 is the address of the function's
        // first instruction (ARM instructions are two-byte aligned), and the 0x1 bit
        // is the "thumb" bit, meaning that the instructions need to be decoded
        // with the thumb decoder.
        let architecture = binary_image.arch();
        let relative_start_address = match architecture {
            Some("arm64" | "arm64e") => start_address & !0b11,
            Some("arm") => start_address & !0b1,
            _ => *start_address,
        };

        // Pad out the number of bytes we read a little, to allow for reading one
        // more instruction.
        // We've been asked to decode the instructions whose instruction addresses
        // are in the range relative_start_address .. (relative_start_address + disassembly_len).
        // If the end of
        // this range points into the middle of an instruction, we still want to
        // decode the entire instruction, so we need all of its bytes.
        // We have another check later to make sure we don't return instructions whose
        // address is beyond the requested range.
        const MAX_INSTR_LEN: u32 = 15; // TODO: Get the correct max length for this arch

        // Now read the instruction bytes from the file.
        let bytes = binary_image
            .read_bytes_at_relative_address(relative_start_address, disassembly_len + MAX_INSTR_LEN)
            .map_err(|e| match e {
                CodeByteReadingError::AddressNotFound => AsmError::AddressNotFound,
                CodeByteReadingError::ObjectParseError(e) => AsmError::ObjectParseError(e),
                CodeByteReadingError::ByteRangeNotInSection => AsmError::ByteRangeNotInSection,
                CodeByteReadingError::FileIO(e) => AsmError::FileIO(e),
            })?;

        let reader = yaxpeax_arch::U8Reader::new(bytes);
        decode_arch(
            reader,
            architecture,
            relative_start_address,
            disassembly_len,
        )
    }

    async fn get_function_end_address(
        &self,
        library_info: &LibraryInfo,
        address_within_function: u32,
    ) -> Option<u32> {
        let symbol_map_res = self.symbol_manager.load_symbol_map(library_info).await;
        let symbol = symbol_map_res
            .ok()?
            .lookup_relative_address(address_within_function)?
            .symbol;
        symbol.address.checked_add(symbol.size?)
    }
}

fn decode_arch(
    reader: U8Reader,
    arch: Option<&str>,
    relative_start_address: u32,
    decode_len: u32,
) -> Result<Response, AsmError> {
    Ok(match arch {
        Some("x86") => {
            decode::<yaxpeax_x86::protected_mode::Arch>(reader, relative_start_address, decode_len)
        }
        Some("x86_64" | "x86_64h") => {
            decode::<yaxpeax_x86::amd64::Arch>(reader, relative_start_address, decode_len)
        }
        Some("arm64" | "arm64e") => {
            decode::<yaxpeax_arm::armv8::a64::ARMv8>(reader, relative_start_address, decode_len)
        }
        Some("arm") => {
            decode::<yaxpeax_arm::armv7::ARMv7>(reader, relative_start_address, decode_len)
        }
        _ => {
            return Err(AsmError::UnrecognizedArch(
                arch.map_or_else(|| "unknown".to_string(), |a| a.to_string()),
            ))
        }
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
    relative_start_address: u32,
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
                            .map(|_| format!("!!! ERROR: {e}"))
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
        start_address: relative_start_address,
        size: final_offset,
        arch: A::ARCH_NAME.to_string(),
        syntax: A::SYNTAX.iter().map(ToString::to_string).collect(),
        instructions,
    }
}
