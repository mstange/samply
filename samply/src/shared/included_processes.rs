/// Specifies which processes should be included in the converted profile.
#[derive(Debug, Clone)]
pub struct IncludedProcesses {
    /// Names of processes to include. These are actually substrings - if
    /// any of the elements in this Vec is a substring of the process name,
    /// then the process is included.
    pub name_substrings: Vec<String>,
    /// Process IDs to include.
    pub pids: Vec<u32>,
}

impl IncludedProcesses {
    #[allow(unused)] // TODO: Remove once the perf.data importer respects IncludedProcesses
    pub fn should_include(&self, name: Option<&str>, pid: u32) -> bool {
        if self.pids.contains(&pid) {
            return true;
        }

        let Some(name) = name else { return false };
        self.name_substrings
            .iter()
            .any(|substr| name.contains(substr))
    }
}
