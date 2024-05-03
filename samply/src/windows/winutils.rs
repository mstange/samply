use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::mem::size_of;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::ptr::null_mut;

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, GetLastError, FALSE, HANDLE, LUID, MAX_PATH};
use windows::Win32::Security::{
    AdjustTokenPrivileges, GetTokenInformation, LookupPrivilegeValueW, TokenElevation,
    SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES, TOKEN_ELEVATION, TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows::Win32::Storage::FileSystem::QueryDosDeviceW;
use windows::Win32::System::ProcessStatus::{EnumDeviceDrivers, GetDeviceDriverFileNameW};
use windows::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

pub fn is_elevated() -> bool {
    unsafe {
        let mut handle: HANDLE = Default::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut handle).ok();

        let mut elevation = TOKEN_ELEVATION::default();
        let mut size = std::mem::size_of::<TOKEN_ELEVATION>() as u32;
        GetTokenInformation(
            handle,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut std::ffi::c_void),
            size,
            &mut size,
        )
        .ok();

        elevation.TokenIsElevated != 0
    }
}

pub fn enable_debug_privilege() {
    if !is_elevated() {
        // TODO elevate with "runas" verb to pop up UAC dialog.
        eprintln!(
            "You must run samply as an Administrator so that it can enable SeDebugPrivilege. \
            Try using 'sudo' on recent Windows."
        );
        std::process::exit(1);
    }

    unsafe {
        let mut h_token: HANDLE = Default::default();
        let mut tp: TOKEN_PRIVILEGES = std::mem::zeroed();

        if OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
            &mut h_token,
        )
        .is_err()
        {
            panic!("OpenProcessToken failed. Error: {:?}", GetLastError());
        }

        let mut luid: LUID = std::mem::zeroed();
        let name = OsString::from("SeDebugPrivilege")
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        if LookupPrivilegeValueW(PCWSTR::null(), PCWSTR(&name[0]), &mut luid).is_err() {
            panic!("LookupPrivilegeValue failed. Error: {:?}", GetLastError());
        }

        tp.PrivilegeCount = 1;
        tp.Privileges[0].Luid = luid;
        tp.Privileges[0].Attributes = SE_PRIVILEGE_ENABLED;

        if AdjustTokenPrivileges(
            h_token,
            FALSE,
            Some(&tp),
            std::mem::size_of::<TOKEN_PRIVILEGES>() as u32,
            None,
            None,
        )
        .is_err()
        {
            panic!("AdjustTokenPrivileges failed. Error: {:?}", GetLastError());
        }

        if !GetLastError().is_ok() {
            eprintln!(
                "AdjustTokenPrivileges succeeded, but the error result is failure. Likely \
                the token does not have the specified privilege, which means you are not running \
                as Administrator. GetLastError: {:?}",
                GetLastError()
            );
            std::process::exit(1);
        }

        CloseHandle(h_token).ok();
    }
}

pub fn from_zero_terminated_wstr(wstr: &[u16]) -> String {
    let path_os_str = OsString::from_wide(&wstr[..wstr.iter().position(|&c| c == 0).unwrap()]);
    path_os_str.to_string_lossy().into()
}

// This is a hack to convert a path like \Device\HarddiskVolume4\Windows\System32\ntoskrnl.exe
// into C:\Windows\System32\ntoskrnl.exe . This turns out to be rocket science, and the Rtl
// method that's supposed to do it (RtlNtPathNameToDosPathName) doesn't seem to actually work.
pub fn get_dos_device_mappings() -> HashMap<String, String> {
    let mut mappings = HashMap::new();
    let mut buffer = vec![0u16; 512];

    // Iterate through possible drive letters
    for drive_letter in b'A'..=b'Z' {
        let drive_str = format!("{}:", drive_letter as char);

        unsafe {
            let wide_drive_str: Vec<u16> = OsStr::new(&drive_str)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            let result =
                QueryDosDeviceW(PWSTR(wide_drive_str.as_ptr() as *mut _), Some(&mut buffer));

            if result != 0 {
                // Success, process the returned multi-sz string
                let end = buffer.iter().position(|&c| c == 0).unwrap();
                let device_path = OsString::from_wide(&buffer[..end]);
                mappings.insert(device_path.to_string_lossy().to_string(), drive_str.clone());
            }
        }
    }

    unsafe {
        GetSystemDirectoryW(Some(&mut buffer));
        // Annoyingly, this return path includes System32, and the NT SystemRoot is to the Windows
        // directory, so we need to strip this.
        let end = buffer.iter().position(|&c| c == 0).unwrap();
        let slash = buffer[..end]
            .iter()
            .rposition(|&c| c == '\\' as u16)
            .unwrap();
        buffer[slash] = 0;

        mappings.insert(
            "\\SystemRoot".to_string(),
            from_zero_terminated_wstr(&buffer),
        );
    }

    mappings
}

// Iterator returning pathname + start/end range for every kernel driver. These are global in every process
// in the same location.
pub fn iter_kernel_drivers() -> impl Iterator<Item = (String, u64, u64)> {
    unsafe {
        // Starting in Windows 11 Version 24H2, EnumDeviceDrivers will require SeDebugPrivilege to return valid ImageBase values.
        // Sigh.
        let mut cb_needed = 0;
        EnumDeviceDrivers(null_mut(), 0, &mut cb_needed).ok();

        let mut drivers = vec![null_mut(); cb_needed as usize / size_of::<usize>()];
        EnumDeviceDrivers(
            drivers.as_mut_ptr(),
            (drivers.len() * size_of::<usize>()) as u32,
            &mut cb_needed,
        )
        .ok();

        drivers.sort();

        let count = cb_needed as usize / size_of::<usize>();

        let mut name_buffer = vec![0u16; MAX_PATH as usize];
        let mut i = 0;
        std::iter::from_fn(move || {
            while i < count {
                let driver_addr = drivers[i];
                i += 1;

                if GetDeviceDriverFileNameW(driver_addr, &mut name_buffer) == 0 {
                    // for some reason this failed; try getting the next one
                    continue;
                }

                let path = from_zero_terminated_wstr(&name_buffer);

                //println!("Driver Path: {}", path);
                //println!("Kernel Base Address: {:?}", driver_addr);

                let start_avma = driver_addr as u64;
                // note that i was already incremented above, so this is the next address
                let end_avma = if i < count {
                    drivers[i] as u64
                } else {
                    0xffff_ffff_ffff_ffff
                };

                return Some((path, start_avma, end_avma));
            }

            None
        })
    }
}
