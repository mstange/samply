mod bindings {
    windows::include_bindings!();
}

extern crate num_traits;

#[macro_use]
extern crate bitflags;

#[macro_use]
extern crate num_derive;


use crate::{bindings::Windows::Win32::Foundation::PWSTR, parser::{Parser, TryParse}, schema::{EventSchema, SchemaLocator}, tdh_types::TdhInType};
use schema::Schema;
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
pub mod kernel_trace_control;

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


    let path: Param<PWSTR> = path.as_os_str().into_param();
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

pub fn schema_from_custom(event: Etw::EVENT_RECORD) -> Option<Schema> {
    /*let img = kernel_trace_control::ImageID{};
    if event.EventHeader.ProviderId == img.provider_guid() && event.EventHeader.EventDescriptor.Opcode as u16 == img.event_id() {
        return Some(Schema::new(event.clone(), Arc::new(img)));
    }*/
    let dbg = kernel_trace_control::DbgID{};
    if event.EventHeader.ProviderId == dbg.provider_guid() && event.EventHeader.EventDescriptor.Opcode as u16 == dbg.event_id() {
        return Some(Schema::new(event.clone(), Arc::new(dbg)));
    }
    return None;
}
/* 
fn main() {

    let mut schema_locator = SchemaLocator::new();
    let mut log_file = open_trace(Path::new("D:\\Captures\\23-09-2021_17-21-32_thread-switch-bench.etl"), 
|e| { 
    let s = tdh::schema_from_tdh(e.clone());    
    if let Ok(s) = s {
        if !(s.opcode_name().starts_with("DC") && s.task_name() == "Thread") {return}
        if e.EventHeader.ProcessId != 33712 { return }
        //if !(s.opcode_name().starts_with("DCStop") && s.provider_name() == "MSNT_SystemTrace") {return}
        //if !(s.opcode_name().starts_with("DCStop")) {return}
        println!("{}/{}/{} {} {}", s.provider_name(), s.task_name(), s.opcode_name(), s.property_count(), e.UserDataLength);

        let schema = schema_locator.event_schema(e.clone()).unwrap();
        let mut parser = Parser::create(&schema);
        for i in 0..s.property_count() {
            let property = s.property(i);
            print!("{:?} = ", property.name);
            match property.in_type() {
                TdhInType::InTypeUnicodeString => println!("{:?}", TryParse::<String>::try_parse(&mut parser, &property.name)),
                TdhInType::InTypeUInt32 => println!("{:?}", TryParse::<u32>::try_parse(&mut parser, &property.name)),
                TdhInType::InTypeUInt8 => println!("{:?}", TryParse::<u8>::try_parse(&mut parser, &property.name)),
                TdhInType::InTypePointer => println!("{:?}", TryParse::<usize>::try_parse(&mut parser, &property.name)),

                _ => println!("Unknown {:?}", property.in_type())
            }
        }
        println!("Name: {}", utils::parse_null_utf16_string(parser.buffer.as_slice()));

    } else {
    //println!("event {:x?} {}", e.EventHeader.ProviderId.data1, s.is_ok());
}

});

    println!("Hello, world!");
}*/

