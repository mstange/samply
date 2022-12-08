use samply_symbols::{
    debugid::{CodeId, DebugId},
    object::{self, Architecture, CompressionFormat, Object},
    relative_address_base, FileAndPathHelper, SymbolManager,
};
use serde_json::json;
use yaxpeax_arch::{Arch, DecodeError, U8Reader};

use crate::asm::response_json::DecodedInstruction;

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

    #[error("Unexpected compression of text section")]
    UnexpectedCompression,

    #[error("Could not read the requested address range from the section (might be out of bounds or the section might not have any bytes in the file)")]
    ByteRangeNotInSection,

    #[error("Unrecognized architecture {0:?}")]
    UnrecognizedArch(Architecture),
}

#[derive(Clone, Debug, Default)]
struct Query {
    start_address: u32,
    size: u32,
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
            ..
        } = request;

        let debug_id = debug_id
            .as_deref()
            .and_then(|debug_id| DebugId::from_breakpad(debug_id).ok());
        let code_id = code_id.clone().map(CodeId::new);

        let binary_image = self
            .symbol_manager
            .load_binary(
                debug_name.as_deref(),
                debug_id,
                name.as_deref(),
                code_id.as_ref(),
            )
            .await
            .map_err(AsmError::LoadBinaryError)?;
        let object = binary_image.make_object();

        let query = Query {
            start_address: (*start_address).into(),
            size: (*size).into(),
        };

        do_stuff_with_object(&object, &query)
    }
}

fn do_stuff_with_object<'data: 'file, 'file>(
    object: &'file impl Object<'data, 'file>,
    query: &Query,
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
        Architecture::Aarch64 => query.start_address & !0b11,
        Architecture::Arm => query.start_address & !0b1,
        _ => query.start_address,
    };

    // Translate start_address from a "relative address" into an
    // SVMA ("stated virtual memory address").
    let image_base = relative_address_base(object);
    let start_address = image_base + u64::from(relative_start_address);

    // Find the section which contains our start_address.
    use object::ObjectSection;
    let (section, section_address_range) = object
        .sections()
        .find_map(|section| {
            let section_start_addr = section.address();
            let section_end_addr = section_start_addr.checked_add(section.size())?;
            let address_range = section_start_addr..section_end_addr;
            if !address_range.contains(&start_address) {
                return None;
            }

            Some((section, address_range))
        })
        .ok_or(AsmError::AddressNotFound)?;

    let file_range = section.compressed_file_range()?;
    if file_range.format != CompressionFormat::None {
        return Err(AsmError::UnexpectedCompression);
    }

    // Pad out the number of bytes we read a little, to allow for reading one
    // more instruction.
    // We've been asked to decode the instructions whose instruction addresses
    // are in the range start_address .. (start_address + size). If the end of
    // this range points into the middle of an instruction, we still want to
    // decode the entire instruction last, so we need all of its bytes.
    // We have another check later to make sure we don't return instructions whose
    // address is beyond the requested range.
    const MAX_INSTR_LEN: u64 = 15; // TODO: Get the correct max length for this arch
    let max_read_len = section_address_range.end - start_address;
    let read_len = (u64::from(query.size) + MAX_INSTR_LEN).min(max_read_len);

    // Now read the instruction bytes from the file.
    let bytes = section
        .data_range(start_address, read_len)?
        .ok_or(AsmError::ByteRangeNotInSection)?;

    let reader = yaxpeax_arch::U8Reader::new(bytes);
    let (instructions, len) = decode_arch(reader, architecture, query.size)?;
    Ok(response_json::Response {
        start_address: relative_start_address.into(),
        size: len.into(),
        instructions,
    })
}

fn decode_arch(
    reader: U8Reader,
    arch: Architecture,
    decode_len: u32,
) -> Result<(Vec<DecodedInstruction>, u32), AsmError> {
    Ok(match arch {
        Architecture::I386 => decode::<yaxpeax_x86::protected_mode::Arch>(reader, decode_len),
        Architecture::X86_64 => decode::<yaxpeax_x86::amd64::Arch>(reader, decode_len),
        Architecture::Aarch64 => decode::<yaxpeax_arm::armv8::a64::ARMv8>(reader, decode_len),
        Architecture::Arm => decode::<yaxpeax_arm::armv7::ARMv7>(reader, decode_len),
        _ => return Err(AsmError::UnrecognizedArch(arch)),
    })
}

trait InstructionDecoding: Arch {
    fn make_decoder() -> Self::Decoder;
    fn stringify_inst(inst: Self::Instruction) -> String;
}

impl InstructionDecoding for yaxpeax_x86::amd64::Arch {
    fn make_decoder() -> Self::Decoder {
        yaxpeax_x86::amd64::InstDecoder::default()
    }

    fn stringify_inst(inst: Self::Instruction) -> String {
        inst.to_string()
    }
}

impl InstructionDecoding for yaxpeax_x86::protected_mode::Arch {
    fn make_decoder() -> Self::Decoder {
        yaxpeax_x86::protected_mode::InstDecoder::default()
    }

    fn stringify_inst(inst: Self::Instruction) -> String {
        inst.to_string()
    }
}

impl InstructionDecoding for yaxpeax_arm::armv8::a64::ARMv8 {
    fn make_decoder() -> Self::Decoder {
        yaxpeax_arm::armv8::a64::InstDecoder::default()
    }

    fn stringify_inst(inst: Self::Instruction) -> String {
        inst.to_string()
    }
}

impl InstructionDecoding for yaxpeax_arm::armv7::ARMv7 {
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

    fn stringify_inst(inst: Self::Instruction) -> String {
        inst.to_string()
    }
}

fn decode<'a, A: InstructionDecoding>(
    mut reader: U8Reader<'a>,
    decode_len: u32,
) -> (Vec<DecodedInstruction>, u32)
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
                let decoded_string = A::stringify_inst(inst);
                instructions.push(DecodedInstruction {
                    offset,
                    decoded_string,
                });
            }
            Err(e) => {
                if !e.data_exhausted() {
                    // If decoding encountered an error, append a fake "!!! ERROR" instruction
                    instructions.push(DecodedInstruction {
                        offset,
                        decoded_string: format!("!!! ERROR: {}", e),
                    });
                }
                break;
            }
        }
    }
    let final_offset = u64::from(yaxpeax_arch::Reader::<A::Address, A::Word>::total_offset(
        &mut reader,
    )) as u32;

    (instructions, final_offset)
}
