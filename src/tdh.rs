use crate::bindings::Windows::Win32::System::Diagnostics::Etw;
use crate::bindings::Windows::Win32::System::Diagnostics::Debug::ERROR_INSUFFICIENT_BUFFER;
use crate::etw_types::*;
use crate::tdh_types::*;
use crate::traits::*;


use crate::utils;
#[derive(Debug)]
pub enum TdhNativeError {
    /// Represents an standard IO Error
    IoError(std::io::Error),
}


impl From<std::io::Error> for TdhNativeError {
    fn from(err: std::io::Error) -> Self {
        TdhNativeError::IoError(err)
    }
}

pub(crate) type TdhNativeResult<T> = Result<T, TdhNativeError>;

pub fn schema_from_tdh(event: Etw::EVENT_RECORD) -> TdhNativeResult<TraceEventInfoRaw> {
    let mut buffer_size = 0;
    unsafe {
        if Etw::TdhGetEventInformation(
            &event,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut buffer_size,
        ) != ERROR_INSUFFICIENT_BUFFER.0
        {
            return Err(TdhNativeError::IoError(std::io::Error::last_os_error()));
        }

        let mut buffer = TraceEventInfoRaw::alloc(buffer_size);
        if Etw::TdhGetEventInformation(
            &event,
            0,
            std::ptr::null_mut(),
            buffer.info_as_ptr() as *mut _,
            &mut buffer_size,
        ) != 0
        {
            return Err(TdhNativeError::IoError(std::io::Error::last_os_error()));
        }

        Ok(buffer)
    }
}

pub(crate) fn property_size(mut event: EventRecord, name: &str) -> TdhNativeResult<u32> {
    let mut property_size = 0;

    let mut desc = Etw::PROPERTY_DATA_DESCRIPTOR::default();
    desc.ArrayIndex = u32::MAX;
    desc.PropertyName = name.as_utf16().as_ptr() as u64;

    unsafe {
        let status = Etw::TdhGetPropertySize(
            &mut event,
            0,
            std::ptr::null_mut(),
            1,
            &mut desc,
            &mut property_size,
        );
        if status != 0 {
            return Err(TdhNativeError::IoError(std::io::Error::from_raw_os_error(
                status as i32,
            )));
        }
    }

    Ok(property_size)
}


#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}



