// LLVM_CONFIG_PATH=~/.mozbuild/clang/bin/llvm-config bindgen -o thread_info.rs --no-layout-tests --no-derive-copy --whitelist-type thread_basic_info --whitelist-type thread_identifier_info_data_t --whitelist-type 'thread_.*' --whitelist-type io_stat_info_t --whitelist-var 'TH(READ)?_.*' --whitelist-var MAXTHREADNAMESIZE /Library/Developer/CommandLineTools/SDKs/MacOSX10.15.sdk/System/Library/Frameworks/Kernel.framework/Versions/A/Headers/mach/thread_info.h

#![allow(non_camel_case_types)]
#![allow(non_upper_case_globals)]

use std::mem::size_of;

use mach2::message::mach_msg_type_number_t;
use mach2::vm_types::{integer_t, natural_t};

pub const THREAD_BASIC_INFO: u32 = 3;
pub const THREAD_IDENTIFIER_INFO: u32 = 4;
pub const THREAD_EXTENDED_INFO: u32 = 5;
pub type policy_t = ::std::os::raw::c_int;
#[repr(C)]
#[derive(Debug, Default)]
pub struct time_value {
    pub seconds: integer_t,
    pub microseconds: integer_t,
}
pub type time_value_t = time_value;
pub type thread_info_t = *mut integer_t;
#[repr(C)]
#[derive(Debug, Default)]
pub struct thread_basic_info {
    pub user_time: time_value_t,
    pub system_time: time_value_t,
    pub cpu_usage: integer_t,
    pub policy: policy_t,
    pub run_state: integer_t,
    pub flags: integer_t,
    pub suspend_count: integer_t,
    pub sleep_time: integer_t,
}
pub type thread_basic_info_data_t = thread_basic_info;
#[repr(C)]
#[derive(Debug, Default)]
pub struct thread_identifier_info {
    pub thread_id: u64,
    pub thread_handle: u64,
    pub dispatch_qaddr: u64,
}
pub type thread_identifier_info_data_t = thread_identifier_info;
#[repr(C)]
pub struct thread_extended_info {
    pub pth_user_time: u64,
    pub pth_system_time: u64,
    pub pth_cpu_usage: i32,
    pub pth_policy: i32,
    pub pth_run_state: i32,
    pub pth_flags: i32,
    pub pth_sleep_time: i32,
    pub pth_curpri: i32,
    pub pth_priority: i32,
    pub pth_maxpriority: i32,
    pub pth_name: [::std::os::raw::c_char; 64usize],
}
pub type thread_extended_info_data_t = thread_extended_info;
#[repr(C)]
#[derive(Debug, Default)]
pub struct io_stat_entry {
    pub count: u64,
    pub size: u64,
}
#[repr(C)]
#[derive(Debug, Default)]
pub struct io_stat_info {
    pub disk_reads: io_stat_entry,
    pub io_priority: [io_stat_entry; 4usize],
    pub paging: io_stat_entry,
    pub metadata: io_stat_entry,
    pub total_io: io_stat_entry,
}

pub const THREAD_BASIC_INFO_COUNT: mach_msg_type_number_t =
    (size_of::<thread_basic_info_data_t>() / size_of::<natural_t>()) as _;
pub const THREAD_IDENTIFIER_INFO_COUNT: mach_msg_type_number_t =
    (size_of::<thread_identifier_info_data_t>() / size_of::<natural_t>()) as _;
pub const THREAD_EXTENDED_INFO_COUNT: mach_msg_type_number_t =
    (size_of::<thread_extended_info_data_t>() / size_of::<natural_t>()) as _;
