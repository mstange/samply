use serde::Serialize;
use std::collections::HashMap;

#[derive(Serialize, Debug)]
pub struct Response {
    pub results: Vec<Result>,
}

#[derive(Serialize, Debug)]
pub struct Result {
    pub stacks: Vec<Stack>,
    pub found_modules: HashMap<String, bool>,
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
        assert_eq!(response, expected);
        Ok(())
    }
}
