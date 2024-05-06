use std::ffi::CStr;

use windows::core::PCSTR;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{FindWindowA, ShowWindow, SW_HIDE};

/// Find the window associated with the console and hide it
pub fn hide_console() {
    unsafe {
        let console_window_name = CStr::from_bytes_with_nul(b"ConsoleWindowClass\0").unwrap();
        let console_window = FindWindowA(
            PCSTR::from_raw(console_window_name.as_ptr() as *const u8),
            PCSTR::null(),
        );
        if console_window != HWND(0) {
            let _ = ShowWindow(console_window, SW_HIDE);
        }
    }
}
