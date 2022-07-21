// LLVM_CONFIG_PATH=~/.mozbuild/clang/bin/llvm-config bindgen -o thread_act.rs --no-layout-tests --no-derive-copy --whitelist-function 'thread_.*' /Library/Developer/CommandLineTools/SDKs/MacOSX10.15.sdk/System/Library/Frameworks/Kernel.framework/Versions/A/Headers/mach/thread_act.h

#![allow(non_camel_case_types)]
#![allow(non_upper_case_globals)]

use super::thread_info::{policy_t, thread_info_t};
use mach::boolean::boolean_t;
use mach::exception_types::{
    exception_behavior_array_t, exception_behavior_t, exception_flavor_array_t,
    exception_mask_array_t, exception_mask_t,
};
use mach::kern_return::kern_return_t;
use mach::mach_types::{exception_handler_array_t, thread_act_t};
use mach::message::mach_msg_type_number_t;
use mach::port::mach_port_t;
use mach::thread_status::{thread_state_flavor_t, thread_state_t};
use mach::vm_types::{integer_t, natural_t};

pub type mach_voucher_t = mach_port_t;
pub type ipc_voucher_t = mach_voucher_t;
pub type mach_voucher_selector_t = u32;
pub type policy_base_t = *mut integer_t;
pub type policy_limit_t = *mut integer_t;
pub type thread_flavor_t = natural_t;
pub type thread_policy_flavor_t = natural_t;
pub type thread_policy_t = *mut integer_t;
pub type thread_inspect_t = mach_port_t;
pub type processor_set_t = mach_port_t;
pub type processor_set_name_t = processor_set_t;
extern "C" {
    pub fn thread_terminate(target_act: thread_act_t) -> kern_return_t;
}
extern "C" {
    pub fn thread_get_state(
        target_act: thread_act_t,
        flavor: thread_state_flavor_t,
        old_state: thread_state_t,
        old_stateCnt: *mut mach_msg_type_number_t,
    ) -> kern_return_t;
}
extern "C" {
    pub fn thread_set_state(
        target_act: thread_act_t,
        flavor: thread_state_flavor_t,
        new_state: thread_state_t,
        new_stateCnt: mach_msg_type_number_t,
    ) -> kern_return_t;
}
extern "C" {
    pub fn thread_suspend(target_act: thread_act_t) -> kern_return_t;
}
extern "C" {
    pub fn thread_resume(target_act: thread_act_t) -> kern_return_t;
}
extern "C" {
    pub fn thread_abort(target_act: thread_act_t) -> kern_return_t;
}
extern "C" {
    pub fn thread_abort_safely(target_act: thread_act_t) -> kern_return_t;
}
extern "C" {
    pub fn thread_depress_abort(thread: thread_act_t) -> kern_return_t;
}
extern "C" {
    pub fn thread_get_special_port(
        thr_act: thread_act_t,
        which_port: ::std::os::raw::c_int,
        special_port: *mut mach_port_t,
    ) -> kern_return_t;
}
extern "C" {
    pub fn thread_set_special_port(
        thr_act: thread_act_t,
        which_port: ::std::os::raw::c_int,
        special_port: mach_port_t,
    ) -> kern_return_t;
}
extern "C" {
    pub fn thread_info(
        target_act: thread_inspect_t,
        flavor: thread_flavor_t,
        thread_info_out: thread_info_t,
        thread_info_outCnt: *mut mach_msg_type_number_t,
    ) -> kern_return_t;
}
extern "C" {
    pub fn thread_set_exception_ports(
        thread: thread_act_t,
        exception_mask: exception_mask_t,
        new_port: mach_port_t,
        behavior: exception_behavior_t,
        new_flavor: thread_state_flavor_t,
    ) -> kern_return_t;
}
extern "C" {
    pub fn thread_get_exception_ports(
        thread: thread_inspect_t,
        exception_mask: exception_mask_t,
        masks: exception_mask_array_t,
        masksCnt: *mut mach_msg_type_number_t,
        old_handlers: exception_handler_array_t,
        old_behaviors: exception_behavior_array_t,
        old_flavors: exception_flavor_array_t,
    ) -> kern_return_t;
}
extern "C" {
    pub fn thread_swap_exception_ports(
        thread: thread_act_t,
        exception_mask: exception_mask_t,
        new_port: mach_port_t,
        behavior: exception_behavior_t,
        new_flavor: thread_state_flavor_t,
        masks: exception_mask_array_t,
        masksCnt: *mut mach_msg_type_number_t,
        old_handlers: exception_handler_array_t,
        old_behaviors: exception_behavior_array_t,
        old_flavors: exception_flavor_array_t,
    ) -> kern_return_t;
}
extern "C" {
    pub fn thread_policy(
        thr_act: thread_act_t,
        policy: policy_t,
        base: policy_base_t,
        baseCnt: mach_msg_type_number_t,
        set_limit: boolean_t,
    ) -> kern_return_t;
}
extern "C" {
    pub fn thread_policy_set(
        thread: thread_act_t,
        flavor: thread_policy_flavor_t,
        policy_info: thread_policy_t,
        policy_infoCnt: mach_msg_type_number_t,
    ) -> kern_return_t;
}
extern "C" {
    pub fn thread_policy_get(
        thread: thread_inspect_t,
        flavor: thread_policy_flavor_t,
        policy_info: thread_policy_t,
        policy_infoCnt: *mut mach_msg_type_number_t,
        get_default: *mut boolean_t,
    ) -> kern_return_t;
}
extern "C" {
    pub fn thread_sample(thread: thread_act_t, reply: mach_port_t) -> kern_return_t;
}
extern "C" {
    pub fn thread_assign(thread: thread_act_t, new_set: processor_set_t) -> kern_return_t;
}
extern "C" {
    pub fn thread_assign_default(thread: thread_act_t) -> kern_return_t;
}
extern "C" {
    pub fn thread_get_assignment(
        thread: thread_act_t,
        assigned_set: *mut processor_set_name_t,
    ) -> kern_return_t;
}
extern "C" {
    pub fn thread_set_policy(
        thr_act: thread_act_t,
        pset: processor_set_t,
        policy: policy_t,
        base: policy_base_t,
        baseCnt: mach_msg_type_number_t,
        limit: policy_limit_t,
        limitCnt: mach_msg_type_number_t,
    ) -> kern_return_t;
}
extern "C" {
    pub fn thread_get_mach_voucher(
        thr_act: thread_act_t,
        which: mach_voucher_selector_t,
        voucher: *mut ipc_voucher_t,
    ) -> kern_return_t;
}
extern "C" {
    pub fn thread_set_mach_voucher(thr_act: thread_act_t, voucher: ipc_voucher_t) -> kern_return_t;
}
extern "C" {
    pub fn thread_swap_mach_voucher(
        thr_act: thread_act_t,
        new_voucher: ipc_voucher_t,
        old_voucher: *mut ipc_voucher_t,
    ) -> kern_return_t;
}
