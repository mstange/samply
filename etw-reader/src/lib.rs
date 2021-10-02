mod bindings {
    windows::include_bindings!();
}

extern crate num_traits;

#[macro_use]
extern crate bitflags;

#[macro_use]
extern crate num_derive;


use crate::{bindings::Windows::Win32::Foundation::PWSTR, parser::{Parser, TryParse}, schema::{EventSchema, SchemaLocator}, tdh_types::TdhInType};
use schema::TypedEvent;
use windows::{IntoParam, Param};
use std::path::Path;
use crate::bindings::Windows::Win32::System::Diagnostics::Etw;
//, WindowsProgramming};

pub mod etw_types;
pub mod tdh;
pub mod tdh_types;
pub mod utils;
pub mod parser;
pub mod property;
pub mod schema;
pub mod sddl;
pub mod traits;
pub mod custom_schemas;

pub use windows::Guid;
#[repr(C)]
#[derive(Clone)]
pub struct EventTraceLogfile(Etw::EVENT_TRACE_LOGFILEW);

impl Default for EventTraceLogfile {
    fn default() -> Self {
        unsafe { std::mem::zeroed::<EventTraceLogfile>() }
    }
}

impl std::ops::Deref for EventTraceLogfile {
    type Target = Etw::EVENT_TRACE_LOGFILEW;

    fn deref(&self) -> &self::Etw::EVENT_TRACE_LOGFILEW {
        &self.0
    }
}
impl std::ops::DerefMut for EventTraceLogfile {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}


unsafe fn trace_callback_thunk(event_record: *mut Etw::EVENT_RECORD) {
    let f: &mut &mut dyn FnMut(&Etw::EVENT_RECORD) = std::mem::transmute((*event_record).UserContext);
    f(&*event_record)
}

pub fn open_trace<F: FnMut(&Etw::EVENT_RECORD)>(path: &Path, mut callback: F)  {
    let mut log_file = EventTraceLogfile::default();

    #[cfg(windows)]
    let path: Param<PWSTR> = path.as_os_str().into_param();
    #[cfg(not(windows))]
    let path: Param<PWSTR> = panic!();
    log_file.0.LogFileName = unsafe { path.abi() };
    log_file.0.Anonymous1.ProcessTraceMode = Etw::PROCESS_TRACE_MODE_EVENT_RECORD | Etw::PROCESS_TRACE_MODE_RAW_TIMESTAMP;
    let mut cb: &mut dyn FnMut(&Etw::EVENT_RECORD) = &mut callback;
    log_file.0.Context = unsafe { std::mem::transmute(&mut cb) };
    log_file.0.Anonymous2.EventRecordCallback = trace_callback_thunk as *mut _;

    //log_file

    let session_handle = unsafe { Etw::OpenTraceW(&mut *log_file) };
    unsafe { Etw::ProcessTrace(&session_handle, 1, std::ptr::null_mut(), std::ptr::null_mut()) };

}

use std::sync::Arc;

pub fn add_custom_schemas(locator: &mut SchemaLocator) {
    locator.add_custom_schema(Arc::new(custom_schemas::ImageID{}));
    locator.add_custom_schema(Arc::new(custom_schemas::DbgID{}));
    locator.add_custom_schema(Arc::new(custom_schemas::ThreadStart{}));
}


