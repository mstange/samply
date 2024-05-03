use serde_derive::Deserialize;

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

#[cfg(test)]
mod test {

    use serde_json::Result;

    use super::super::request_json::Request;

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
}
