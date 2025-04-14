// LLVM_CONFIG_PATH=~/.mozbuild/clang/bin/llvm-config bindgen -o thread_act.rs --no-layout-tests --no-derive-copy --whitelist-function 'thread_.*' /Library/Developer/CommandLineTools/SDKs/MacOSX10.15.sdk/System/Library/Frameworks/Kernel.framework/Versions/A/Headers/mach/thread_act.h

#![allow(non_camel_case_types)]
#![allow(non_upper_case_globals)]

use mach2::kern_return::kern_return_t;
use mach2::message::mach_msg_type_number_t;
use mach2::port::mach_port_t;
use mach2::vm_types::natural_t;

use super::thread_info::thread_info_t;

pub type thread_flavor_t = natural_t;
pub type thread_inspect_t = mach_port_t;
extern "C" {
    pub fn thread_info(
        target_act: thread_inspect_t,
        flavor: thread_flavor_t,
        thread_info_out: thread_info_t,
        thread_info_outCnt: *mut mach_msg_type_number_t,
    ) -> kern_return_t;
}
