use serde::Serialize;
use serde_tuple::*;

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

    /// The disassembled instructions.
    pub instructions: Vec<DecodedInstruction>,
}

#[derive(Serialize_tuple, Debug)]
pub struct DecodedInstruction {
    /// Byte offset from start_address.
    pub offset: u32,

    /// The decoded instruction as a string.
    pub decoded_string: String,
}

#[cfg(test)]
mod test {
    use super::{DecodedInstruction, Response};
    use serde_json::Result;

    #[test]
    fn serialize_correctly() -> Result<()> {
        let response = Response {
            start_address: 0x1234,
            size: 0x3,
            instructions: vec![
                DecodedInstruction {
                    offset: 0,
                    decoded_string: "push rbp".to_string(),
                },
                DecodedInstruction {
                    offset: 1,
                    decoded_string: "mov rbp, rsp".to_string(),
                },
            ],
        };
        let response = serde_json::to_string_pretty(&response)?;
        let expected = r#"{
  "startAddress": "0x1234",
  "size": "0x3",
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
        eprintln!("{}", response);
        assert_eq!(response, expected);
        Ok(())
    }
}
