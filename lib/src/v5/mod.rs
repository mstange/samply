use crate::{FileAndPathHelper, GetSymbolsError, Result, SymbolTableResult};
use std::collections::HashMap;
use std::ops::Deref;

pub mod request_json {
    use serde::Deserialize;

    #[derive(Deserialize, Debug)]
    #[serde(untagged)]
    pub enum Request {
        WithJobsList { jobs: Vec<Job> },
        JustOneJob(Job),
    }

    impl Request {
        pub fn jobs(&self) -> JobIterator {
            match self {
                Request::WithJobsList { jobs } => JobIterator::WithJobsList(jobs.iter()),
                Request::JustOneJob(job) => JobIterator::JustOneJob(std::iter::once(job)),
            }
        }
    }

    #[derive(Deserialize, Debug)]
    #[serde(rename_all = "camelCase")]
    pub struct Job {
        pub memory_map: Vec<Lib>,
        pub stacks: Vec<Stack>,
    }

    #[derive(Deserialize, Debug, PartialEq, Eq, Hash, Clone)]
    pub struct Lib {
        pub debug_name: String,
        pub breakpad_id: String,
    }

    #[derive(Deserialize, Debug)]
    pub struct Stack(pub Vec<StackFrame>);

    #[derive(Deserialize, Debug)]
    pub struct StackFrame {
        /// index into memory_map
        pub module_index: u32,
        /// lib-relative memory offset
        pub address: u32,
    }

    pub enum JobIterator<'a> {
        WithJobsList(std::slice::Iter<'a, Job>),
        JustOneJob(std::iter::Once<&'a Job>),
    }

    impl<'a> Iterator for JobIterator<'a> {
        type Item = &'a Job;

        fn next(&mut self) -> Option<&'a Job> {
            match self {
                JobIterator::WithJobsList(it) => it.next(),
                JobIterator::JustOneJob(it) => it.next(),
            }
        }
    }
}

mod response_json {
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
        pub module_offset: u64,

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
}

use request_json::Lib;

pub async fn get_api_response(
    request_json_data: &str,
    helper: &impl FileAndPathHelper,
) -> Result<String> {
    let request: request_json::Request = serde_json::from_str(request_json_data)?;
    let mut requested_addresses: HashMap<Lib, Vec<u32>> = HashMap::new();
    for job in request.jobs() {
        for stack in &job.stacks {
            for frame in &stack.0 {
                let lib = job.memory_map.get(frame.module_index as usize).ok_or(
                    GetSymbolsError::ParseRequestErrorContents(
                        "Stack frame module index indexes beyond the memoryMap",
                    ),
                )?;
                requested_addresses
                    .entry((*lib).clone())
                    .or_insert_with(Vec::new)
                    .push(frame.address);
            }
        }
    }
    for (lib, addresses) in requested_addresses.into_iter() {
        let address_results = get_address_results(&lib, addresses, helper).await;
    }
    Ok(String::from(""))
}

struct AddressResult {}

struct SymbolTable {
    symbols: Vec<(u32, String)>,
}

impl SymbolTableResult for SymbolTable {
    fn from_map<T: Deref<Target = str>>(map: HashMap<u32, T>) -> Self {
        let mut symbols: Vec<_> = map
            .into_iter()
            .map(|(address, symbol)| (address, String::from(&*symbol)))
            .collect();
        symbols.sort_by_key(|&(addr, _)| addr);
        symbols.dedup_by_key(|&mut (addr, _)| addr);
        SymbolTable { symbols }
    }
}

async fn get_address_results(
    lib: &Lib,
    addresses: Vec<u32>,
    helper: &impl FileAndPathHelper,
) -> Result<HashMap<u32, AddressResult>> {
    let symbol_table: SymbolTable =
        crate::get_symbol_table_result(&lib.debug_name, &lib.breakpad_id, helper).await?;
    unimplemented!()
}

#[cfg(test)]
mod test {

    use super::request_json::Request;
    use super::response_json;
    use serde_json::Result;

    #[test]
    fn parse_job() -> Result<()> {
        let data = r#"
        {
            "jobs": [
                {
                  "memoryMap": [
                    [
                      "xul.pdb",
                      "44E4EC8C2F41492B9369D6B9A059577C2"
                    ],
                    [
                      "wntdll.pdb",
                      "D74F79EB1F8D4A45ABCD2F476CCABACC2"
                    ]
                  ],
                  "stacks": [
                    [
                      [0, 11723767],
                      [1, 65802]
                    ]
                  ]
                }
            ]
        }"#;

        let r: Request = serde_json::from_str(data)?;
        assert_eq!(r.jobs().count(), 1);
        Ok(())
    }

    #[test]
    fn parse_without_jobs_wrapper() -> Result<()> {
        let data = r#"
        {
            "memoryMap": [
              [
                "xul.pdb",
                "44E4EC8C2F41492B9369D6B9A059577C2"
              ],
              [
                "wntdll.pdb",
                "D74F79EB1F8D4A45ABCD2F476CCABACC2"
              ]
            ],
            "stacks": [
              [
                [0, 11723767],
                [1, 65802]
              ]
            ]
          }
          "#;

        let r: Request = serde_json::from_str(data)?;
        assert_eq!(r.jobs().count(), 1);
        Ok(())
    }

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
