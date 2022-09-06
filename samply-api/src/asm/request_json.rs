use serde::Deserialize;
use serde_hex::{CompactPfx, SerHex};

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub name: Option<String>, // example: "xul.dll"
    pub code_id: Option<String>,
    pub debug_name: Option<String>, // example: "xul.pdb"
    pub debug_id: Option<String>,

    /// The start address where the disassembly should start,
    /// as a "0x"-prefixed hex string, interpreted as a
    /// library-relative offset in bytes.
    #[serde(with = "SerHex::<CompactPfx>")]
    pub start_address: u32,

    /// The length, in bytes, of the machine code that should be disassembled,
    /// as a "0x"-prefixed hex string.
    #[serde(with = "SerHex::<CompactPfx>")]
    pub size: u32,
}

#[cfg(test)]
mod test {

    use super::Request;
    use serde_json::Result;

    #[test]
    fn parse_job() -> Result<()> {
        let data = r#"
        {
          "debugName": "xul.pdb",
          "debugId": "A14CAFD390A3E1884C4C44205044422E1",
          "startAddress": "0x1d04742",
          "size": "0x84"
        }"#;

        let r: Request = serde_json::from_str(data)?;
        assert_eq!(r.start_address, 30426946);
        Ok(())
    }
}
