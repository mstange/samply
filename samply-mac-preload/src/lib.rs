#![no_main]
#![no_std]

use libc::{c_char, c_int, mode_t, FILE};

use core::ffi::CStr;

mod mach_ipc;
mod mach_sys;

use mach_ipc::{channel, mach_task_self, OsIpcChannel, OsIpcSender};

extern "C" {
    fn open(path: *const c_char, flags: c_int, mode: mode_t) -> c_int;
    fn fopen(filename: *const c_char, mode: *const c_char) -> *mut FILE;
}

static CHANNEL_SENDER: spin::Mutex<Option<OsIpcSender>> = spin::Mutex::new(None);

#[cfg(not(test))]
#[panic_handler]
fn panic(_panic: &core::panic::PanicInfo<'_>) -> ! {
    unsafe { libc::abort() }
}

// Run our code as early as possible, by pretending to be a global constructor.
// This code was taken from https://github.com/neon-bindings/neon/blob/2277e943a619579c144c1da543874f4a7ec39879/src/lib.rs#L40-L44
#[used]
#[allow(non_upper_case_globals)]
#[cfg_attr(target_os = "macos", link_section = "__DATA,__mod_init_func")]
static __SETUP_SAMPLY_CONNECTION: unsafe extern "C" fn() = {
    unsafe extern "C" fn __load_samply_lib() {
        let _ = set_up_samply_connection();
    }
    __load_samply_lib
};

fn set_up_samply_connection() -> Option<()> {
    let (tx0, rx0) = channel().ok()?;
    // Safety:
    // - b"SAMPLY_BOOTSTRAP_SERVER_NAME\0" is a nul-terminated c string
    // - This is the only code running, nobody else is calling getenv or setenv on other threads
    let tx1 = unsafe {
        let name = libc::getenv(b"SAMPLY_BOOTSTRAP_SERVER_NAME\0".as_ptr() as *const libc::c_char)
            as *const libc::c_char;
        if name.is_null() {
            return None;
        }
        OsIpcSender::connect(name).ok()?
    };
    // We have a connection to the parent.

    // Send our task to the parent. Then the parent can control us completely.
    let p = mach_task_self();
    let c = OsIpcChannel::RawPort(p);
    let pid = unsafe { libc::getpid() };
    let mut message_bytes = [0; 11];
    message_bytes[0..7].copy_from_slice(b"My task");
    message_bytes[7..11].copy_from_slice(&pid.to_le_bytes());
    tx1.send(&message_bytes, [OsIpcChannel::Sender(tx0), c])
        .ok()?;
    *CHANNEL_SENDER.lock() = Some(tx1);
    // Wait for the parent to tell us to proceed, in case it wants to do any more setup with our task.
    let mut recv_buf = [0; 256];
    let result = rx0.recv(&mut recv_buf).ok()?;
    assert_eq!(b"Proceed", &result);
    Some(())
}

// Override the `open` function, in order to be able to observe the file
// paths of opened files.
//
// We use this to detect jitdump files.
#[no_mangle]
extern "C" fn samply_hooked_open(path: *const c_char, flags: c_int, mode: mode_t) -> c_int {
    // unsafe {
    //     libc::printf(b"open(%s, %d, %u)\n\0".as_ptr() as *const i8, path, flags, mode as c_uint);
    // }

    if let Ok(path) = unsafe { CStr::from_ptr(path) }.to_str() {
        detect_and_send_jitdump_path(path);
        detect_and_send_marker_file_path(path);
    }

    // Call the original. Do this at the end, so that this is compiled as a tail call.
    //
    // WARNING: What we are doing here is even sketchier than it seems. The `open` function
    // is variadic: It can be called with or without the mode parameter. I have not found
    // the right way to forward those variadic args properly. So by using a tail call, we
    // can hope that the compiled code leaves the arguments completely untouched and just
    // jumps to the called function, and everything should work out fine in terms of the
    // call ABI.
    unsafe { open(path, flags, mode) }
}

// Override fopen for the same reason.
#[no_mangle]
extern "C" fn samply_hooked_fopen(path: *const c_char, mode: *const c_char) -> *mut FILE {
    // unsafe {
    //     libc::printf(b"fopen(%s, %s\n\0".as_ptr() as *const i8, path, mode);
    // }

    if let Ok(path) = unsafe { CStr::from_ptr(path) }.to_str() {
        detect_and_send_jitdump_path(path);
        detect_and_send_marker_file_path(path);
    }

    // Call the original.
    unsafe { fopen(path, mode) }
}

fn detect_and_send_jitdump_path(path: &str) {
    if path.len() > 256 - 12 || !path.ends_with(".dump") || !path.contains("/jit-") {
        return;
    }

    let channel_sender = CHANNEL_SENDER.lock();
    let Some(sender) = channel_sender.as_ref() else { return };
    let pid = unsafe { libc::getpid() };
    let mut message_bytes = [0; 256];
    message_bytes[0..7].copy_from_slice(b"Jitdump");
    message_bytes[7..11].copy_from_slice(&pid.to_le_bytes());
    message_bytes[11] = path.len() as u8;
    message_bytes[12..][..path.len()].copy_from_slice(path.as_bytes());
    let _ = sender.send(&message_bytes, []);
}

fn detect_and_send_marker_file_path(path: &str) {
    if path.len() > 256 - 12 || !path.ends_with(".txt") || !path.contains("/marker-") {
        return;
    }

    let channel_sender = CHANNEL_SENDER.lock();
    let Some(sender) = channel_sender.as_ref() else { return };
    let pid = unsafe { libc::getpid() };
    let mut message_bytes = [0; 256];
    message_bytes[0..7].copy_from_slice(b"MarkerF");
    message_bytes[7..11].copy_from_slice(&pid.to_le_bytes());
    message_bytes[11] = path.len() as u8;
    message_bytes[12..][..path.len()].copy_from_slice(path.as_bytes());
    let _ = sender.send(&message_bytes, []);
}

#[allow(non_camel_case_types)]
pub struct InterposeEntry {
    _new: *const (),
    _old: *const (),
}

#[used]
#[allow(dead_code)]
#[allow(non_upper_case_globals)]
#[link_section = "__DATA,__interpose"]
pub static mut _interpose_open: InterposeEntry = InterposeEntry {
    _new: samply_hooked_open as *const (),
    _old: open as *const (),
};

#[used]
#[allow(dead_code)]
#[allow(non_upper_case_globals)]
#[link_section = "__DATA,__interpose"]
pub static mut _interpose_fopen: InterposeEntry = InterposeEntry {
    _new: samply_hooked_fopen as *const (),
    _old: fopen as *const (),
};
