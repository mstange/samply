// Copyright 2015 The Servo Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use crate::mach_sys::{
    self, kern_return_t, mach_msg_body_t, mach_msg_header_t, mach_msg_ool_descriptor_t,
    mach_msg_port_descriptor_t, mach_msg_return_t, mach_msg_timeout_t, mach_msg_type_name_t,
    mach_port_deallocate, mach_port_limits_t, mach_port_msgcount_t, mach_port_right_t,
    mach_task_self_,
};

pub use crate::mach_sys::mach_port_t;

use core::cell::Cell;
use core::fmt::Debug;
use core::marker::PhantomData;
use core::mem;
use core::ptr;
use core::slice;
use core::usize;
use libc::{self, c_char, size_t};

pub const MACH_PORT_NULL: mach_port_t = 0;

const BOOTSTRAP_SUCCESS: kern_return_t = 0;
const KERN_NOT_IN_SET: kern_return_t = 12;
const KERN_INVALID_NAME: kern_return_t = 15;
const KERN_INVALID_RIGHT: kern_return_t = 17;
const KERN_INVALID_VALUE: kern_return_t = 18;
const KERN_UREFS_OVERFLOW: kern_return_t = 19;
const KERN_INVALID_CAPABILITY: kern_return_t = 20;
const KERN_SUCCESS: kern_return_t = 0;
const KERN_NO_SPACE: kern_return_t = 3;
const MACH_MSGH_BITS_COMPLEX: u32 = 0x80000000;
const MACH_MSG_IPC_KERNEL: kern_return_t = 0x00000800;
const MACH_MSG_IPC_SPACE: kern_return_t = 0x00002000;
const MACH_MSG_PORT_DESCRIPTOR: u32 = 0;
const MACH_MSG_SUCCESS: kern_return_t = 0;
const MACH_MSG_TIMEOUT_NONE: mach_msg_timeout_t = 0;
const MACH_MSG_TYPE_COPY_SEND: u32 = 19;
const MACH_MSG_TYPE_MAKE_SEND: u32 = 20;
const MACH_MSG_TYPE_MAKE_SEND_ONCE: u32 = 21;
const MACH_MSG_TYPE_MOVE_SEND: u32 = 17;
const MACH_MSG_TYPE_PORT_SEND: u32 = MACH_MSG_TYPE_MOVE_SEND;
const MACH_MSG_VM_KERNEL: kern_return_t = 0x00000400;
const MACH_MSG_VM_SPACE: kern_return_t = 0x00001000;
const MACH_NOTIFY_FIRST: i32 = 64;
const MACH_NOTIFY_NO_SENDERS: i32 = MACH_NOTIFY_FIRST + 6;
const MACH_PORT_LIMITS_INFO: i32 = 1;
const MACH_PORT_QLIMIT_LARGE: mach_port_msgcount_t = 1024;
const MACH_PORT_QLIMIT_MAX: mach_port_msgcount_t = MACH_PORT_QLIMIT_LARGE;
const MACH_PORT_RIGHT_RECEIVE: mach_port_right_t = 1;
const MACH_PORT_RIGHT_SEND: mach_port_right_t = 0;
const MACH_RCV_BODY_ERROR: kern_return_t = 0x1000400c;
const MACH_RCV_HEADER_ERROR: kern_return_t = 0x1000400b;
const MACH_RCV_INTERRUPTED: kern_return_t = 0x10004005;
const MACH_RCV_INVALID_DATA: kern_return_t = 0x10004008;
const MACH_RCV_INVALID_NAME: kern_return_t = 0x10004002;
const MACH_RCV_INVALID_NOTIFY: kern_return_t = 0x10004007;
const MACH_RCV_INVALID_TRAILER: kern_return_t = 0x1000400f;
const MACH_RCV_INVALID_TYPE: kern_return_t = 0x1000400d;
const MACH_RCV_IN_PROGRESS: kern_return_t = 0x10004001;
const MACH_RCV_IN_PROGRESS_TIMED: kern_return_t = 0x10004011;
const MACH_RCV_IN_SET: kern_return_t = 0x1000400a;
const MACH_RCV_LARGE: i32 = 4;
const MACH_RCV_MSG: i32 = 2;
const MACH_RCV_PORT_CHANGED: kern_return_t = 0x10004006;
const MACH_RCV_PORT_DIED: kern_return_t = 0x10004009;
const MACH_RCV_SCATTER_SMALL: kern_return_t = 0x1000400e;
const MACH_RCV_TIMED_OUT: kern_return_t = 0x10004003;
const MACH_RCV_TOO_LARGE: kern_return_t = 0x10004004;
const MACH_SEND_INTERRUPTED: kern_return_t = 0x10000007;
const MACH_SEND_INVALID_DATA: kern_return_t = 0x10000002;
const MACH_SEND_INVALID_DEST: kern_return_t = 0x10000003;
const MACH_SEND_INVALID_HEADER: kern_return_t = 0x10000010;
const MACH_SEND_INVALID_MEMORY: kern_return_t = 0x1000000c;
const MACH_SEND_INVALID_NOTIFY: kern_return_t = 0x1000000b;
const MACH_SEND_INVALID_REPLY: kern_return_t = 0x10000009;
const MACH_SEND_INVALID_RIGHT: kern_return_t = 0x1000000a;
const MACH_SEND_INVALID_RT_OOL_SIZE: kern_return_t = 0x10000015;
const MACH_SEND_INVALID_TRAILER: kern_return_t = 0x10000011;
const MACH_SEND_INVALID_TYPE: kern_return_t = 0x1000000f;
const MACH_SEND_INVALID_VOUCHER: kern_return_t = 0x10000005;
const MACH_SEND_IN_PROGRESS: kern_return_t = 0x10000001;
const MACH_SEND_MSG: i32 = 1;
const MACH_SEND_MSG_TOO_SMALL: kern_return_t = 0x10000008;
const MACH_SEND_NO_BUFFER: kern_return_t = 0x1000000d;
const MACH_SEND_TIMED_OUT: kern_return_t = 0x10000004;
const MACH_SEND_TOO_LARGE: kern_return_t = 0x1000000e;
const TASK_BOOTSTRAP_PORT: i32 = 4;

#[allow(non_camel_case_types)]
type name_t = *const c_char;

pub fn channel() -> Result<(OsIpcSender, OsIpcReceiver), MachError> {
    let receiver = OsIpcReceiver::new()?;
    let sender = receiver.sender()?;
    receiver.request_no_senders_notification()?;
    Ok((sender, receiver))
}

#[derive(PartialEq, Eq, Debug)]
pub struct OsIpcReceiver {
    port: Cell<mach_port_t>,
}

impl Drop for OsIpcReceiver {
    fn drop(&mut self) {
        let port = self.port.get();
        if port != MACH_PORT_NULL {
            mach_port_mod_release(port, MACH_PORT_RIGHT_RECEIVE).unwrap();
        }
    }
}

fn mach_port_allocate(right: mach_port_right_t) -> Result<mach_port_t, KernelError> {
    let mut port: mach_port_t = 0;
    let os_result = unsafe { mach_sys::mach_port_allocate(mach_task_self(), right, &mut port) };
    if os_result == KERN_SUCCESS {
        return Ok(port);
    }
    Err(os_result.into())
}

fn mach_port_mod_addref(port: mach_port_t, right: mach_port_right_t) -> Result<(), KernelError> {
    let err = unsafe { mach_sys::mach_port_mod_refs(mach_task_self(), port, right, 1) };
    if err == KERN_SUCCESS {
        return Ok(());
    }
    Err(err.into())
}

fn mach_port_mod_release(port: mach_port_t, right: mach_port_right_t) -> Result<(), KernelError> {
    let err = unsafe { mach_sys::mach_port_mod_refs(mach_task_self(), port, right, -1) };
    if err == KERN_SUCCESS {
        return Ok(());
    }
    Err(err.into())
}

fn mach_port_extract_right(
    port: mach_port_t,
    message_type: mach_msg_type_name_t,
) -> Result<(mach_port_t, mach_msg_type_name_t), KernelError> {
    let (mut right, mut acquired_right) = (0, 0);
    let error = unsafe {
        mach_sys::mach_port_extract_right(
            mach_task_self(),
            port,
            message_type,
            &mut right,
            &mut acquired_right,
        )
    };
    if error == KERN_SUCCESS {
        return Ok((right, acquired_right));
    }
    Err(error.into())
}

impl OsIpcReceiver {
    fn new() -> Result<OsIpcReceiver, MachError> {
        let port = mach_port_allocate(MACH_PORT_RIGHT_RECEIVE)?;
        let limits = mach_port_limits_t {
            mpl_qlimit: MACH_PORT_QLIMIT_MAX,
        };
        let os_result = unsafe {
            mach_sys::mach_port_set_attributes(
                mach_task_self(),
                port,
                MACH_PORT_LIMITS_INFO,
                &limits as *const mach_sys::mach_port_limits as *mut i32,
                1,
            )
        };
        if os_result == KERN_SUCCESS {
            Ok(OsIpcReceiver::from_name(port))
        } else {
            Err(KernelError::from(os_result).into())
        }
    }

    fn from_name(port: mach_port_t) -> OsIpcReceiver {
        OsIpcReceiver {
            port: Cell::new(port),
        }
    }

    fn sender(&self) -> Result<OsIpcSender, MachError> {
        let port = self.port.get();
        debug_assert!(port != MACH_PORT_NULL);
        let (right, acquired_right) = mach_port_extract_right(port, MACH_MSG_TYPE_MAKE_SEND)?;
        debug_assert!(acquired_right == MACH_MSG_TYPE_PORT_SEND);
        Ok(OsIpcSender::from_name(right))
    }

    fn request_no_senders_notification(&self) -> Result<(), MachError> {
        let port = self.port.get();
        debug_assert!(port != MACH_PORT_NULL);
        unsafe {
            let os_result = mach_sys::mach_port_request_notification(
                mach_task_self(),
                port,
                MACH_NOTIFY_NO_SENDERS,
                0,
                port,
                MACH_MSG_TYPE_MAKE_SEND_ONCE as u32,
                &mut 0,
            );
            if os_result != KERN_SUCCESS {
                return Err(KernelError::from(os_result).into());
            }
        }
        Ok(())
    }

    pub fn recv<'a>(&self, buf: &'a mut [u8]) -> Result<&'a [u8], MachError> {
        let port = self.port.get();

        debug_assert!(port != MACH_PORT_NULL);
        unsafe {
            setup_receive_buffer(buf, port);
            let message = &mut buf[0] as *mut _ as *mut Message;
            let flags = MACH_RCV_MSG | MACH_RCV_LARGE;
            let timeout = MACH_MSG_TIMEOUT_NONE;
            match mach_sys::mach_msg(
                message as *mut _,
                flags,
                0,
                buf.len() as u32,
                port,
                timeout,
                MACH_PORT_NULL,
            ) {
                MACH_MSG_SUCCESS => {}
                os_result => return Err(MachError::from(os_result)),
            }

            if (*message).header.msgh_id == MACH_NOTIFY_NO_SENDERS {
                return Err(MachError::from(MACH_NOTIFY_NO_SENDERS));
            }

            let mut port_descriptor = message.offset(1) as *mut mach_msg_port_descriptor_t;
            let mut descriptors_remaining = (*message).body.msgh_descriptor_count;
            while descriptors_remaining > 0 {
                if (*port_descriptor).type_() != MACH_MSG_PORT_DESCRIPTOR {
                    break;
                }
                let _ = (*port_descriptor).name;
                port_descriptor = port_descriptor.offset(1);
                descriptors_remaining -= 1;
            }

            let shared_memory_descriptor = port_descriptor as *mut mach_msg_ool_descriptor_t;
            if descriptors_remaining > 0 {
                return Err(MachError::RcvInvalidType);
            }

            let has_inline_data_ptr = shared_memory_descriptor as *mut bool;
            let has_inline_data = *has_inline_data_ptr;
            let payload = if has_inline_data {
                let padding_start = has_inline_data_ptr.offset(1) as *mut u8;
                let padding_count = Message::payload_padding(padding_start as usize);
                let payload_size_ptr = padding_start.add(padding_count) as *mut usize;
                let payload_size = *payload_size_ptr;
                let max_payload_size = message as usize + ((*message).header.msgh_size as usize)
                    - (shared_memory_descriptor as usize);
                assert!(payload_size <= max_payload_size);
                let payload_ptr = payload_size_ptr.offset(1) as *mut u8;
                slice::from_raw_parts(payload_ptr, payload_size)
            } else {
                return Err(MachError::RcvBodyError);
            };

            Ok(payload)
        }
    }
}

#[derive(PartialEq, Eq, Debug)]
pub struct OsIpcSender {
    port: mach_port_t,
    // Make sure this is `!Sync`, to match `crossbeam_channel::Sender`; and to discourage sharing
    // references.
    //
    // (Rather, senders should just be cloned, as they are shared internally anyway --
    // another layer of sharing only adds unnecessary overhead...)
    nosync_marker: PhantomData<Cell<()>>,
}

impl Drop for OsIpcSender {
    fn drop(&mut self) {
        if self.port == MACH_PORT_NULL {
            return;
        }
        // mach_port_deallocate and mach_port_mod_refs are very similar, except that
        // mach_port_mod_refs returns an error when there are no receivers for the port,
        // causing the sender port to never be deallocated. mach_port_deallocate handles
        // this case correctly and is therefore important to avoid dangling port leaks.
        let err = unsafe { mach_port_deallocate(mach_task_self(), self.port) };
        if err != KERN_SUCCESS {
            panic!("mach_port_deallocate({}) failed: {:?}", self.port, err);
        }
    }
}

impl Clone for OsIpcSender {
    fn clone(&self) -> OsIpcSender {
        let mut cloned_port = self.port;
        if cloned_port != MACH_PORT_NULL {
            match mach_port_mod_addref(cloned_port, MACH_PORT_RIGHT_SEND) {
                Ok(()) => (),
                Err(KernelError::InvalidRight) => cloned_port = MACH_PORT_NULL,
                Err(error) => panic!("mach_port_mod_refs(1, {}) failed: {:?}", cloned_port, error),
            }
        }
        OsIpcSender {
            port: cloned_port,
            nosync_marker: PhantomData,
        }
    }
}

impl OsIpcSender {
    fn from_name(port: mach_port_t) -> OsIpcSender {
        OsIpcSender {
            port,
            nosync_marker: PhantomData,
        }
    }

    /// Safety: name_c must point at a nul-terminated C string.
    pub unsafe fn connect(name_c: *const c_char) -> Result<OsIpcSender, MachError> {
        let mut bootstrap_port = 0;
        let os_result = mach_sys::task_get_special_port(
            mach_task_self(),
            TASK_BOOTSTRAP_PORT,
            &mut bootstrap_port,
        );
        if os_result != KERN_SUCCESS {
            return Err(KernelError::from(os_result).into());
        }

        let mut port = 0;
        let os_result = bootstrap_look_up(bootstrap_port, name_c, &mut port);
        if os_result == BOOTSTRAP_SUCCESS {
            Ok(OsIpcSender::from_name(port))
        } else {
            Err(MachError::from(os_result))
        }
    }

    pub fn send<const N: usize>(
        &self,
        data: &[u8],
        ports: [OsIpcChannel; N],
    ) -> Result<(), MachError> {
        unsafe {
            let size = Message::size_of(data, ports.len(), 0);
            let message = libc::malloc(size as size_t) as *mut Message;
            (*message).header.msgh_bits = (MACH_MSG_TYPE_COPY_SEND) | MACH_MSGH_BITS_COMPLEX;
            (*message).header.msgh_size = size as u32;
            (*message).header.msgh_local_port = MACH_PORT_NULL;
            (*message).header.msgh_remote_port = self.port;
            (*message).header.msgh_id = 0;
            (*message).body.msgh_descriptor_count = ports.len() as u32;

            let mut port_descriptor_dest = message.offset(1) as *mut mach_msg_port_descriptor_t;
            for outgoing_port in &ports {
                (*port_descriptor_dest).name = outgoing_port.port();
                (*port_descriptor_dest).pad1 = 0;

                let disposition = match *outgoing_port {
                    OsIpcChannel::Sender(_) => MACH_MSG_TYPE_MOVE_SEND,
                    OsIpcChannel::RawPort(_) => MACH_MSG_TYPE_COPY_SEND,
                };
                (*port_descriptor_dest).set_disposition(disposition);

                (*port_descriptor_dest).set_type(MACH_MSG_PORT_DESCRIPTOR);
                port_descriptor_dest = port_descriptor_dest.offset(1);
            }

            let shared_memory_descriptor_dest =
                port_descriptor_dest as *mut mach_msg_ool_descriptor_t;
            let is_inline_dest = shared_memory_descriptor_dest as *mut bool;
            *is_inline_dest = true;

            if true {
                // Zero out the last word for paranoia's sake.
                *((message as *mut u8).offset(size as isize - 4) as *mut u32) = 0;

                let data_size = data.len();
                let padding_start = is_inline_dest.offset(1) as *mut u8;
                let padding_count = Message::payload_padding(padding_start as usize);
                // Zero out padding
                padding_start.write_bytes(0, padding_count);
                let data_size_dest = padding_start.add(padding_count) as *mut usize;
                *data_size_dest = data_size;

                let data_dest = data_size_dest.offset(1) as *mut u8;
                ptr::copy_nonoverlapping(data.as_ptr(), data_dest, data_size);
            }

            let os_result = mach_sys::mach_msg(
                message as *mut _,
                MACH_SEND_MSG,
                (*message).header.msgh_size,
                0,
                MACH_PORT_NULL,
                MACH_MSG_TIMEOUT_NONE,
                MACH_PORT_NULL,
            );
            libc::free(message as *mut _);
            if os_result != MACH_MSG_SUCCESS {
                return Err(MachError::from(os_result));
            }
            for outgoing_port in ports {
                mem::forget(outgoing_port);
            }
            Ok(())
        }
    }
}

pub enum OsIpcChannel {
    Sender(OsIpcSender),
    RawPort(mach_port_t),
}

impl OsIpcChannel {
    fn port(&self) -> mach_port_t {
        match *self {
            OsIpcChannel::Sender(ref sender) => sender.port,
            OsIpcChannel::RawPort(ref port) => *port,
        }
    }
}

#[derive(PartialEq, Eq, Debug)]
pub struct OsOpaqueIpcChannel {
    port: mach_port_t,
}

impl Drop for OsOpaqueIpcChannel {
    fn drop(&mut self) {
        // Make sure we don't leak!
        debug_assert!(self.port == MACH_PORT_NULL);
    }
}

unsafe fn setup_receive_buffer(buffer: &mut [u8], port_name: mach_port_t) {
    let message = &buffer[0] as *const u8 as *mut mach_msg_header_t;
    (*message).msgh_local_port = port_name;
    (*message).msgh_size = buffer.len().try_into().unwrap_or(u32::MAX)
}

pub fn mach_task_self() -> mach_port_t {
    unsafe { mach_task_self_ }
}

#[repr(C)]
struct Message {
    header: mach_msg_header_t,
    body: mach_msg_body_t,
}

impl Message {
    fn payload_padding(unaligned: usize) -> usize {
        ((unaligned + 7) & !7) - unaligned // 8 byte alignment
    }

    fn size_of(data: &[u8], port_length: usize, shared_memory_length: usize) -> usize {
        let mut size = mem::size_of::<Message>()
            + mem::size_of::<mach_msg_port_descriptor_t>() * port_length
            + mem::size_of::<mach_msg_ool_descriptor_t>() * shared_memory_length
            + mem::size_of::<bool>();

        // rustc panics in debug mode for unaligned accesses.
        // so include padding to start payload at 8-byte aligned address
        size += Self::payload_padding(size);
        size += mem::size_of::<usize>() + data.len();

        // Round up to the next 4 bytes; mach_msg_send returns an error for unaligned sizes.
        if (size & 0x3) != 0 {
            size = (size & !0x3) + 4;
        }

        size
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KernelError {
    Success,
    NoSpace,
    InvalidName,
    InvalidRight,
    InvalidValue,
    InvalidCapability,
    UrefsOverflow,
    NotInSet,
    Unknown(kern_return_t),
}

impl From<kern_return_t> for KernelError {
    fn from(code: kern_return_t) -> KernelError {
        match code {
            KERN_SUCCESS => KernelError::Success,
            KERN_NO_SPACE => KernelError::NoSpace,
            KERN_INVALID_NAME => KernelError::InvalidName,
            KERN_INVALID_RIGHT => KernelError::InvalidRight,
            KERN_INVALID_VALUE => KernelError::InvalidValue,
            KERN_INVALID_CAPABILITY => KernelError::InvalidCapability,
            KERN_UREFS_OVERFLOW => KernelError::UrefsOverflow,
            KERN_NOT_IN_SET => KernelError::NotInSet,
            code => KernelError::Unknown(code),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MachError {
    Success,
    Kernel(KernelError),
    IpcSpace,
    VmSpace,
    IpcKernel,
    VmKernel,
    RcvInProgress,
    RcvInvalidName,
    RcvTimedOut,
    RcvTooLarge,
    RcvInterrupted,
    RcvPortChanged,
    RcvInvalidNotify,
    RcvInvalidData,
    RcvPortDied,
    RcvInSet,
    RcvHeaderError,
    RcvBodyError,
    RcvInvalidType,
    RcvScatterSmall,
    RcvInvalidTrailer,
    RcvInProgressTimed,
    NotifyNoSenders,
    SendInterrupted,
    SendInvalidData,
    SendInvalidDest,
    SendInvalidHeader,
    SendInvalidMemory,
    SendInvalidNotify,
    SendInvalidReply,
    SendInvalidRight,
    SendInvalidRtOolSize,
    SendInvalidTrailer,
    SendInvalidType,
    SendInvalidVoucher,
    SendInProgress,
    SendMsgTooSmall,
    SendNoBuffer,
    SendTimedOut,
    SendTooLarge,
    Unknown(mach_msg_return_t),
}

impl From<mach_msg_return_t> for MachError {
    fn from(code: mach_msg_return_t) -> MachError {
        match code {
            MACH_MSG_SUCCESS => MachError::Success,
            MACH_MSG_IPC_KERNEL => MachError::IpcKernel,
            MACH_MSG_IPC_SPACE => MachError::IpcSpace,
            MACH_MSG_VM_KERNEL => MachError::VmKernel,
            MACH_MSG_VM_SPACE => MachError::VmSpace,
            MACH_RCV_BODY_ERROR => MachError::RcvBodyError,
            MACH_RCV_HEADER_ERROR => MachError::RcvHeaderError,
            MACH_RCV_INTERRUPTED => MachError::RcvInterrupted,
            MACH_RCV_INVALID_DATA => MachError::RcvInvalidData,
            MACH_RCV_INVALID_NAME => MachError::RcvInvalidName,
            MACH_RCV_INVALID_NOTIFY => MachError::RcvInvalidNotify,
            MACH_RCV_INVALID_TRAILER => MachError::RcvInvalidTrailer,
            MACH_RCV_INVALID_TYPE => MachError::RcvInvalidType,
            MACH_RCV_IN_PROGRESS => MachError::RcvInProgress,
            MACH_RCV_IN_PROGRESS_TIMED => MachError::RcvInProgressTimed,
            MACH_RCV_IN_SET => MachError::RcvInSet,
            MACH_RCV_PORT_CHANGED => MachError::RcvPortChanged,
            MACH_RCV_PORT_DIED => MachError::RcvPortDied,
            MACH_RCV_SCATTER_SMALL => MachError::RcvScatterSmall,
            MACH_RCV_TIMED_OUT => MachError::RcvTimedOut,
            MACH_RCV_TOO_LARGE => MachError::RcvTooLarge,
            MACH_NOTIFY_NO_SENDERS => MachError::NotifyNoSenders,
            MACH_SEND_INTERRUPTED => MachError::SendInterrupted,
            MACH_SEND_INVALID_DATA => MachError::SendInvalidData,
            MACH_SEND_INVALID_DEST => MachError::SendInvalidDest,
            MACH_SEND_INVALID_HEADER => MachError::SendInvalidHeader,
            MACH_SEND_INVALID_MEMORY => MachError::SendInvalidMemory,
            MACH_SEND_INVALID_NOTIFY => MachError::SendInvalidNotify,
            MACH_SEND_INVALID_REPLY => MachError::SendInvalidReply,
            MACH_SEND_INVALID_RIGHT => MachError::SendInvalidRight,
            MACH_SEND_INVALID_RT_OOL_SIZE => MachError::SendInvalidRtOolSize,
            MACH_SEND_INVALID_TRAILER => MachError::SendInvalidTrailer,
            MACH_SEND_INVALID_TYPE => MachError::SendInvalidType,
            MACH_SEND_INVALID_VOUCHER => MachError::SendInvalidVoucher,
            MACH_SEND_IN_PROGRESS => MachError::SendInProgress,
            MACH_SEND_MSG_TOO_SMALL => MachError::SendMsgTooSmall,
            MACH_SEND_NO_BUFFER => MachError::SendNoBuffer,
            MACH_SEND_TIMED_OUT => MachError::SendTimedOut,
            MACH_SEND_TOO_LARGE => MachError::SendTooLarge,
            code => MachError::Unknown(code),
        }
    }
}

impl From<KernelError> for MachError {
    fn from(kernel_error: KernelError) -> MachError {
        MachError::Kernel(kernel_error)
    }
}

extern "C" {
    fn bootstrap_look_up(
        bp: mach_port_t,
        service_name: name_t,
        sp: *mut mach_port_t,
    ) -> kern_return_t;
}
