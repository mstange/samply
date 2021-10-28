extern crate num_traits;

#[macro_use]
extern crate bitflags;

#[macro_use]
extern crate num_derive;

use windows::Win32::Foundation::PWSTR;
use crate::{parser::{Parser, ParserError, TryParse}, schema::SchemaLocator, tdh_types::{PropertyDesc, PrimitiveDesc, TdhInType}};
use etw_types::EventRecord;
use tdh_types::Property;
use windows::runtime::{IntoParam, Param};
use std::{collections::HashMap, hash::BuildHasherDefault, path::Path};
use windows::Win32::System::Diagnostics::Etw;
use fxhash::FxHasher;
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

pub use windows::runtime::GUID;

pub type FastHashMap<K, V> = HashMap<K, V, BuildHasherDefault<FxHasher>>;
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
    let f: &mut &mut dyn FnMut(&EventRecord) = std::mem::transmute((*event_record).UserContext);
    f(std::mem::transmute(event_record))
}

pub fn open_trace<F: FnMut(&EventRecord)>(path: &Path, mut callback: F)  {
    let mut log_file = EventTraceLogfile::default();

    #[cfg(windows)]
    let path: Param<PWSTR> = path.as_os_str().into_param();
    #[cfg(not(windows))]
    let path: Param<PWSTR> = panic!();
    log_file.0.LogFileName = unsafe { path.abi() };
    log_file.0.Anonymous1.ProcessTraceMode = Etw::PROCESS_TRACE_MODE_EVENT_RECORD | Etw::PROCESS_TRACE_MODE_RAW_TIMESTAMP;
    let mut cb: &mut dyn FnMut(&EventRecord) = &mut callback;
    log_file.0.Context = unsafe { std::mem::transmute(&mut cb) };
    log_file.0.Anonymous2.EventRecordCallback = trace_callback_thunk as *mut _;

    let session_handle = unsafe { Etw::OpenTraceW(&mut *log_file) };
    unsafe { Etw::ProcessTrace(&session_handle, 1, std::ptr::null_mut(), std::ptr::null_mut()) };

}

pub fn print_property(parser: &mut Parser, property: &Property) {
    print!("  {}= ", property.name);
    if let Some(map_info) = &property.map_info {
        let mut value = match property.desc {
            PropertyDesc::Primitive(PrimitiveDesc{ in_type: TdhInType::InTypeUInt32, ..}) => TryParse::<u32>::parse(parser, &property.name),
            _ => panic!()
        };
        if map_info.is_bitmap {
            let mut remaining_bits_str = String::new();
            let mut matches: Vec<&str> = Vec::new();
            let mut cleared_value = value;
            for (k, v) in &map_info.map {
                if value & k != 0 {
                    matches.push(v.trim());
                    cleared_value &= !k;
                }
            }
            if cleared_value != 0 {
                remaining_bits_str = cleared_value.to_string();
                matches.push(&remaining_bits_str);
                println!("unnamed bits {} {} {:?}", value, cleared_value, map_info.map);
            }
            println!("{}", matches.join(" | "));
        } else {
            println!("{}", map_info.map[&value]);
        }
    } else {
        let value = match &property.desc {
            PropertyDesc::Primitive(desc) => {
                match desc.in_type {
                    TdhInType::InTypeUnicodeString => TryParse::<String>::try_parse(parser, &property.name),
                    TdhInType::InTypeAnsiString => TryParse::<String>::try_parse(parser, &property.name),
                    TdhInType::InTypeUInt32 => TryParse::<u32>::try_parse(parser, &property.name).map(|x| x.to_string()),
                    TdhInType::InTypeUInt16 => TryParse::<u16>::try_parse(parser, &property.name).map(|x| x.to_string()),
                    TdhInType::InTypeUInt8 => TryParse::<u8>::try_parse(parser, &property.name).map(|x| x.to_string()),
                    TdhInType::InTypeInt64 => TryParse::<i64>::try_parse(parser, &property.name).map(|x| x.to_string()),
                    TdhInType::InTypeUInt64 => TryParse::<u64>::try_parse(parser, &property.name).map(|x| x.to_string()),
                    TdhInType::InTypePointer => TryParse::<u64>::try_parse(parser, &property.name).map(|x| x.to_string()),
                    TdhInType::InTypeGuid => TryParse::<GUID>::try_parse(parser, &property.name).map(|x| format!("{:?}", x)),
                    _ => Ok(format!("Unknown {:?}", desc.in_type))
                }
            }
            PropertyDesc::Struct(desc) => Ok(format!("unhandled struct {} {}", desc.start_index, desc.num_members)),
        };
        let value = match value {
            Ok(value) => value,
            Err(ParserError::InvalidType) => format!("invalid type {:?}", property.desc),
            Err(ParserError::LengthMismatch) => format!("Err(LengthMismatch) type: {:?}, flags: {:?}, buf: {}", property.desc, property.flags, parser.buffer.len()),
            Err(e) => format!("Err({:?}) type: {:?}", e, property.desc)
        };
        println!("{}", value)
    }
}

pub fn add_custom_schemas(locator: &mut SchemaLocator) {
    locator.add_custom_schema(Box::new(custom_schemas::ImageID{}));
    locator.add_custom_schema(Box::new(custom_schemas::DbgID{}));
    locator.add_custom_schema(Box::new(custom_schemas::ThreadStart{}));
}


