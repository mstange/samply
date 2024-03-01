use etw_reader::{enumerate_trace_guids, tdh};

pub fn main() {
    tdh::list_etw_providers();
    enumerate_trace_guids();
}