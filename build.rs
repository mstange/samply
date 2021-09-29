fn main() {
    windows::build!(
        Windows::Win32::System::Diagnostics::Etw::*,
        Windows::Data::Xml::Dom::*,
        Windows::Win32::Foundation::CloseHandle,
        Windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject},
        Windows::Win32::UI::WindowsAndMessaging::MessageBoxA,
        
        Windows::Win32::System::Diagnostics::Debug::WIN32_ERROR,
        Windows::Win32::Foundation::{
            PSTR, MAX_PATH, SysStringLen, BSTR, FILETIME, PSID
        },
        Windows::Win32::System::SystemServices::{
            VER_GREATER_EQUAL
        },
        Windows::Win32::System::Memory::LocalFree,
        Windows::Win32::System::SystemInformation::{
            OSVERSIONINFOEXA,
            GetSystemTimeAsFileTime,VerifyVersionInfoA, VerSetConditionMask
        },
        Windows::Win32::Security::Authorization::{ConvertSidToStringSidA, },
    );
}