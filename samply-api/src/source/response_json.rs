use serde::Serialize;

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    /// If present, ISO string representation of the date and time at which the
    /// symbols file was last modified.
    pub symbols_last_modified: Option<String>,

    /// If present, ISO string representation of the date and time at which the
    /// source file was last modified.
    pub source_last_modified: Option<String>,

    /// The path of the requested file.
    pub file: String,

    /// The full source code of the requested file.
    pub source: String,
}

#[cfg(test)]
mod test {
    use serde_json::Result;

    #[test]
    fn serialize_correctly() -> Result<()> {
        let response = super::Response {
            symbols_last_modified: None,
            source_last_modified: None,
            file: "/Users/mstange/code/mozilla/modules/zlib/src/deflate.c".to_string(),
            source: r#"/* deflate.c -- compress data using the deflation algorithm
* Copyright (C) 1995-2017 Jean-loup Gailly and Mark Adler
* For conditions of distribution and use, see copyright notice in zlib.h
*/
"#
            .to_string(),
        };
        let response = serde_json::to_string_pretty(&response)?;
        let expected = r#"{
  "symbolsLastModified": null,
  "sourceLastModified": null,
  "file": "/Users/mstange/code/mozilla/modules/zlib/src/deflate.c",
  "source": "/* deflate.c -- compress data using the deflation algorithm\n* Copyright (C) 1995-2017 Jean-loup Gailly and Mark Adler\n* For conditions of distribution and use, see copyright notice in zlib.h\n*/\n"
}"#;
        eprintln!("{response}");
        assert_eq!(response, expected);
        Ok(())
    }
}
