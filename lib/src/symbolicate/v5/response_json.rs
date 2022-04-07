use std::collections::HashMap;

use serde::Serialize;

#[derive(Serialize, Debug)]
pub struct Response {
    pub results: Vec<Result>,
}

#[derive(Serialize, Debug)]
pub struct Result {
    pub stacks: Vec<Stack>,
    pub found_modules: HashMap<String, bool>,

    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub module_errors: HashMap<String, Vec<Error>>,
}

#[derive(Serialize, Debug)]
pub struct Stack(pub Vec<StackFrame>);

#[derive(Serialize, Debug)]
pub struct StackFrame {
    /// index of this StackFrame in its parent Stack
    pub frame: u32,

    #[serde(serialize_with = "as_hex_string")]
    pub module_offset: u32,

    pub module: String,

    #[serde(flatten)]
    pub symbol: Option<Symbol>,
}

#[derive(Serialize, Debug)]
pub struct Symbol {
    pub function: String,

    #[serde(serialize_with = "as_hex_string")]
    pub function_offset: u32,

    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "as_optional_hex_string"
    )]
    pub function_size: Option<u32>,

    #[serde(flatten)]
    pub debug_info: Option<DebugInfo>,
}

#[derive(Serialize, Debug)]
pub struct DebugInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,

    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub inlines: Vec<InlineStackFrame>,
}

#[derive(Serialize, Debug)]
pub struct InlineStackFrame {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

#[derive(Serialize, Debug)]
pub struct Error {
    pub name: String,
    pub message: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

impl From<&crate::GetSymbolsError> for Error {
    fn from(err: &crate::GetSymbolsError) -> Self {
        Self {
            name: err.enum_as_string().to_string(),
            message: err.to_string(),
            filename: None,
            line: None,
        }
    }
}

fn as_hex_string<S, T>(field: &T, serializer: S) -> std::result::Result<S::Ok, S::Error>
where
    S: serde::Serializer,
    T: std::fmt::LowerHex,
{
    serializer.collect_str(&format_args!("0x{:x}", field))
}

fn as_optional_hex_string<S, T>(
    field: &Option<T>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error>
where
    S: serde::Serializer,
    T: std::fmt::LowerHex,
{
    match field {
        Some(field) => serializer.collect_str(&format_args!("0x{:x}", field)),
        None => serializer.serialize_none(),
    }
}

#[cfg(test)]
mod test {
    use std::collections::HashMap;

    use super::super::response_json;
    use serde_json::Result;

    #[test]
    fn serialize_correctly() -> Result<()> {
        let response = response_json::Response {
            results: vec![response_json::Result {
                stacks: vec![response_json::Stack(vec![
                    response_json::StackFrame {
                        frame: 0,
                        module_offset: 0xb2e3f7,
                        module: String::from("xul.pdb"),
                        symbol: Some(response_json::Symbol {
                            function: String::from("sctp_send_initiate"),
                            function_offset: 0x4ca,
                            function_size: None,
                            debug_info: None,
                        }),
                    },
                    response_json::StackFrame {
                        frame: 1,
                        module_offset: 0x1010a,
                        module: String::from("wntdll.pdb"),
                        symbol: None,
                    },
                ])],
                found_modules: [(
                    String::from("xul.pdb/44E4EC8C2F41492B9369D6B9A059577C2"),
                    true,
                )]
                .iter()
                .cloned()
                .collect(),
                module_errors: HashMap::new(),
            }],
        };
        let response = serde_json::to_string_pretty(&response)?;
        let expected = r#"{
  "results": [
    {
      "stacks": [
        [
          {
            "frame": 0,
            "module_offset": "0xb2e3f7",
            "module": "xul.pdb",
            "function": "sctp_send_initiate",
            "function_offset": "0x4ca"
          },
          {
            "frame": 1,
            "module_offset": "0x1010a",
            "module": "wntdll.pdb"
          }
        ]
      ],
      "found_modules": {
        "xul.pdb/44E4EC8C2F41492B9369D6B9A059577C2": true
      }
    }
  ]
}"#;
        eprintln!("{}", response);
        assert_eq!(response, expected);
        Ok(())
    }
}
