use etw_reader::{enumerate_trace_guids_ex, tdh};

pub fn main() {
    tdh::list_etw_providers();
    enumerate_trace_guids_ex(false);
}
