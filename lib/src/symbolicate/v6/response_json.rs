use serde::Serialize;

#[derive(Serialize, Debug)]
pub struct Response {
    pub results: Vec<Result>,
}

#[derive(Serialize, Debug)]
pub struct Result {
    pub stacks: Vec<Stack>,
    pub module_status: Vec<Option<ModuleStatus>>,
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

    #[serde(flatten)]
    pub debug_info: Option<DebugInfo>,
}

#[derive(Serialize, Debug)]
pub struct DebugInfo {
    pub inline_stack: Vec<InlineStackFrame>,
}

#[derive(Serialize, Debug)]
pub struct InlineStackFrame {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function_name: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_number: Option<u32>,
}

#[derive(Serialize, Debug)]
pub struct ModuleStatus {
    pub found: bool,
    pub symbol_count: u32,
    pub errors: Vec<Error>,
}

#[derive(Serialize, Debug)]
pub struct Error {
    pub name: String,
    pub message: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<String>,
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

#[cfg(test)]
mod test {
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
                module_status: vec![Some(response_json::ModuleStatus {
                    found: true,
                    symbol_count: 12345,
                    errors: vec![],
                })],
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
      "module_status": [
        {
          "found": true,
          "symbol_count": 12345,
          "errors": []
        }
      ]
    }
  ]
}"#;
        assert_eq!(response, expected);
        Ok(())
    }
}
