use serde_derive::Deserialize;

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    /// The debugName of the library whose symbol information contains
    /// a reference to the requested file.
    pub debug_name: String,

    /// The debugId / "breakpadId" of the library whose symbol information
    /// contains a reference to the requested file.
    pub debug_id: String,

    /// An address, as a "0x"-prefixed hex string, interpreted as a
    /// library-relative offset in bytes.
    /// This address is symbolicated, and any of the files referenced in
    /// the symbolication results is eligible to be requested.
    #[serde(deserialize_with = "crate::hex::from_prefixed_hex_str")]
    pub module_offset: u32,

    /// The full path of the requested file, must match exactly what
    /// /symbolicate/v5 returned in its response json for the given
    /// address.
    pub file: String,
}

#[cfg(test)]
mod test {
    use serde_json::Result;

    use super::Request;

    #[test]
    fn parse_job() -> Result<()> {
        let data = r#"
        {
          "debugName": "xul.pdb",
          "debugId": "A14CAFD390A3E1884C4C44205044422E1",
          "moduleOffset": "0x1d04742",
          "file": "/builds/worker/workspace/obj-build/x86_64-pc-windows-msvc/release/build/cssparser-066ad33a2406b35b/out/tokenizer.rs"
        }"#;

        let r: Request = serde_json::from_str(data)?;
        println!("{r:?}");
        assert_eq!(r.module_offset, 30426946);
        Ok(())
    }
}
