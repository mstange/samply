use serde::ser::SerializeSeq;
use serde_derive::Serialize;

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    /// The start address where the return assembly code is located,
    /// as a "0x"-prefixed hex string, interpreted as a
    /// library-relative offset in bytes.
    #[serde(serialize_with = "crate::hex::as_hex_string")]
    pub start_address: u32,

    /// The length, in bytes, of the disassembled machine code.
    #[serde(serialize_with = "crate::hex::as_hex_string")]
    pub size: u32,

    /// The CPU architecture targeted by this binary, e.g. "i686", "x86_64", "arm", "aarch64"
    pub arch: String,

    /// A single-element Vec with the disassembly syntax used in the `instructions`,
    /// e.g. `["Intel"]` for x86.
    ///
    /// This is a Vec because I'd like to use `["Intel", "C Style"]` in the future,
    /// with each instruction being `[<offset>, <intel-diassembly>, <c-style-disassembly>]`.
    pub syntax: Vec<String>,

    /// The disassembled instructions.
    pub instructions: Vec<DecodedInstruction>,
}

#[derive(Debug)]
pub struct DecodedInstruction {
    /// Byte offset from start_address.
    pub offset: u32,

    /// The decoded instruction as a string, one for each syntax (e.g. Intel and then C-Style).
    pub decoded_string_per_syntax: Vec<String>,
}

impl serde::Serialize for DecodedInstruction {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // Output as a single array of the form `[8, "decoded instruction"]`.
        // If syntax is `["Intel", "C style"]`, serialize as `[8, "decoded using Intel syntax", "decoded using C style syntax"]`.
        let mut seq = serializer.serialize_seq(None)?;

        // First element is the offset to the start address.
        seq.serialize_element(&self.offset)?;

        // Flatten all string into the outer array.
        for decoded_string in &self.decoded_string_per_syntax {
            seq.serialize_element(decoded_string)?;
        }

        // In the future we may append more elements here, for extra per-instruction information.
        // For example `{ "jumpTarget": "0x1390" }`.
        // Or even `{ "jumpTarget": "0x2468", "destSymbol": { "name": "MyFunction()", "address": "0x2468", "size": "0x38" } }`

        seq.end()
    }
}

#[cfg(test)]
mod test {
    use serde_json::Result;

    use super::{DecodedInstruction, Response};

    #[test]
    fn serialize_correctly() -> Result<()> {
        let response = Response {
            start_address: 0x1234,
            size: 0x3,
            arch: "x86_64".to_string(),
            syntax: vec!["Intel".to_string()],
            instructions: vec![
                DecodedInstruction {
                    offset: 0,
                    decoded_string_per_syntax: vec!["push rbp".to_string()],
                },
                DecodedInstruction {
                    offset: 1,
                    decoded_string_per_syntax: vec!["mov rbp, rsp".to_string()],
                },
            ],
        };
        let response = serde_json::to_string_pretty(&response)?;
        let expected = r#"{
  "startAddress": "0x1234",
  "size": "0x3",
  "arch": "x86_64",
  "syntax": [
    "Intel"
  ],
  "instructions": [
    [
      0,
      "push rbp"
    ],
    [
      1,
      "mov rbp, rsp"
    ]
  ]
}"#;
        // eprintln!("{}", response);
        assert_eq!(response, expected);
        Ok(())
    }
}
