use crate::bindings::Windows::Win32::{
    Security,
    Foundation::{PSTR},
    System::Memory::LocalFree,

};
//use crate::traits::*;
use std::str::Utf8Error;

/// SDDL native error
#[derive(Debug)]
pub enum SddlNativeError {
    /// Represents an error parsing the SID into a String
    SidParseError(Utf8Error),
    /// Represents an standard IO Error
    IoError(std::io::Error),
}

//impl LastOsError<SddlNativeError> for SddlNativeError {}

impl From<std::io::Error> for SddlNativeError {
    fn from(err: std::io::Error) -> Self {
        SddlNativeError::IoError(err)
    }
}

impl From<Utf8Error> for SddlNativeError {
    fn from(err: Utf8Error) -> Self {
        SddlNativeError::SidParseError(err)
    }
}

pub(crate) type SddlResult<T> = Result<T, SddlNativeError>;

pub fn convert_sid_to_string(sid: isize) -> SddlResult<String> {
    /*
    let mut tmp = PSTR::NULL;
    unsafe {
        if !Security::ConvertSidToStringSidA(Security::PSID(sid), &mut tmp).as_bool() {
            return Err(SddlNativeError::IoError(std::io::Error::last_os_error()));
        }

        let sid_string = std::ffi::CStr::from_ptr(tmp.0 as *mut _)
            .to_str()?
            .to_owned();

        LocalFree(tmp.0 as isize);

        Ok(sid_string)
    }*/
    panic!()
}
/*
#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_convert_string_to_sid() {
        let sid: Vec<u8> = vec![1, 2, 0, 0, 0, 0, 0, 5, 0x20, 0, 0, 0, 0x20, 2, 0, 0];
        if let Ok(string_sid) = convert_sid_to_string(sid.as_ptr() as isize) {
            assert_eq!(string_sid, "S-1-5-32-544");
        }
    }
}*/
