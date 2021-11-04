extern crate num_traits;

#[macro_use]
extern crate bitflags;

#[macro_use]
extern crate num_derive;

use windows::Win32::Foundation::{GetLastError, MAX_PATH, PWSTR};
use crate::{parser::{Parser, ParserError, TryParse}, schema::SchemaLocator, tdh_types::{PropertyDesc, PrimitiveDesc, TdhInType}, traits::EncodeUtf16};

#[macro_use]
extern crate memoffset;

use etw_types::EventRecord;
use tdh_types::Property;
use windows::runtime::{IntoParam, Param};
use std::{borrow::Cow, collections::HashMap, hash::BuildHasherDefault, path::Path};
use windows::Win32::System::Diagnostics::Etw;
use fxhash::FxHasher;

// typedef ULONG64 TRACEHANDLE, *PTRACEHANDLE;
pub(crate) type TraceHandle = u64;
pub const INVALID_TRACE_HANDLE: TraceHandle = u64::MAX;
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
//pub mod trace;
//pub mod provider;

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

/// Complete Trace Properties struct
///
/// The [EventTraceProperties] struct contains the information about a tracing session, this struct
/// also needs two buffers right after it to hold the log file name and the session name. This struct
/// provides the full definition of the properties plus the the allocation for both names
///
/// See: [EVENT_TRACE_PROPERTIES](https://docs.microsoft.com/en-us/windows/win32/api/evntrace/ns-evntrace-event_trace_properties)
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TraceInfo {
    pub properties: Etw::EVENT_TRACE_PROPERTIES,
    // it's not clear that these need to be u16 if using with the Unicode versions of the functions ETW functions
    trace_name: [u16; MAX_PATH as usize],
    log_file_name: [u16; MAX_PATH as usize],
}

impl Default for TraceInfo {
    fn default() -> Self {
        let properties = Etw::EVENT_TRACE_PROPERTIES::default();
        TraceInfo {
            properties,
            trace_name: [0; 260],
            log_file_name: [0; 260],
        }
    }
}

impl TraceInfo {
    pub(crate) fn fill(
        &mut self,
        trace_name: &str,
        //trace_properties: &TraceProperties,
    ) 
    {
        self.properties.Wnode.BufferSize = std::mem::size_of::<TraceInfo>() as u32;
        self.properties.Wnode.Guid = GUID::new().unwrap();
        self.properties.Wnode.Flags = Etw::WNODE_FLAG_TRACED_GUID;
        self.properties.Wnode.ClientContext = 1; // QPC clock resolution
        self.properties.FlushTimer = 1;


        self.properties.LogFileMode =
        Etw::EVENT_TRACE_REAL_TIME_MODE | Etw::EVENT_TRACE_NO_PER_PROCESSOR_BUFFERING;

        self.properties.EnableFlags.0 = 0;

        //self.properties.LoggerNameOffset = offset_of!(TraceInfo, log_file_name) as u32;
        //self.trace_name[..trace_name.len()].copy_from_slice(trace_name.as_bytes())

        // it doesn't seem like it matters if we fill in trace_name
        self.properties.LoggerNameOffset = offset_of!(TraceInfo, trace_name) as u32;
        self.properties.LogFileNameOffset = offset_of!(TraceInfo, log_file_name) as u32;
        self.trace_name[..trace_name.len() + 1].copy_from_slice(&trace_name.as_utf16())
    }
}


#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct EnableTraceParameters(Etw::ENABLE_TRACE_PARAMETERS);

impl EnableTraceParameters {
    pub fn create(guid: GUID, trace_flags: u32) -> Self {
        let mut params = EnableTraceParameters::default();
        params.0.ControlFlags = 0;
        params.0.Version = Etw::ENABLE_TRACE_PARAMETERS_VERSION_2;
        params.0.SourceId = guid;
        params.0.EnableProperty = trace_flags;

        // TODO: Add Filters option
        params.0.EnableFilterDesc = std::ptr::null_mut();
        params.0.FilterDescCount = 0;

        params
    }
}

impl std::ops::Deref for EnableTraceParameters {
    type Target = Etw::ENABLE_TRACE_PARAMETERS;

    fn deref(&self) -> &self::Etw::ENABLE_TRACE_PARAMETERS {
        &self.0
    }
}

impl std::ops::DerefMut for EnableTraceParameters {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Provider {
    /// Use the `new` function to create a Provider builder
    ///
    /// This function will create a by-default provider which can be tweaked afterwards
    ///
    /// # Example
    /// ```rust
    /// let my_provider = Provider::new();
    /// ```
    pub fn new() -> Self {
        Provider {
            guid: None,
            any: 0,
            all: 0,
            level: 5,
            trace_flags: 0,
            flags: 0,
        }
    }

    pub fn by_guid(mut self, guid: &str) -> Self {
        self.guid = Some(GUID::from(guid));
        self
    }
}


pub struct Provider {
    /// Option that represents a Provider GUID
    pub guid: Option<GUID>,
    /// Provider Any keyword
    pub any: u64,
    /// Provider All keyword
    pub all: u64,
    /// Provider level flag
    pub level: u8,
    /// Provider trace flags
    pub trace_flags: u32,
    /// Provider kernel flags, only apply to KernelProvider
    pub flags: u32, // Only applies to KernelProviders
    // perfinfo

    // filters: RwLock<Vec<F>>,
}

pub fn start_trace<F: FnMut(&EventRecord)>(mut callback: F)  {
    let guid_str = "22fb2cd6-0e7b-422b-a0c7-2fad1fd0e716";
    let guid_str = "DB6F6DDB-AC77-4E88-8253-819DF9BBF140";
    let mut video_blt_guid = GUID::from(guid_str);//GUID::from("DB6F6DDB-AC77-4E88-8253-819DF9BBF140");

    let session_name = "aaaaaa".to_owned();

    let mut info = TraceInfo::default();
    info.fill(&session_name);

    let session_name_pwstr: Param<PWSTR> = session_name.into_param();

    let mut handle = 0;

    unsafe {
        let status = Etw::ControlTraceW(0, session_name_pwstr.abi(), &info.properties as *const _ as *mut _, Etw::EVENT_TRACE_CONTROL_STOP);
        println!("ControlTrace = {}", status);
        let status = Etw::StartTraceW(&mut handle, session_name_pwstr.abi(), &info.properties as *const _ as *mut _);
        println!("StartTrace = {} handle {}", status, handle);
        info.trace_name = [0; 260];

        let status = Etw::ControlTraceW(handle, session_name_pwstr.abi(), &mut info.properties, Etw::EVENT_TRACE_CONTROL_QUERY);
        println!("ControlTrace = {} {} {:?} {:?}", status, info.properties.BufferSize, info.properties.LoggerThreadId, info.trace_name);
    }

    let prov = Provider::new().by_guid(guid_str);
    let mut parameters = EnableTraceParameters::create(video_blt_guid, prov.trace_flags);
    /*
    let status = unsafe { Etw::EnableTrace(1, 0xffffffff, Etw::TRACE_LEVEL_VERBOSE, &video_blt_guid, handle)};
    println!("EnableTrace = {}", status);
    */
    unsafe { Etw::EnableTraceEx2(
        handle,
        &mut video_blt_guid,
        1, // Fixme: EVENT_CONTROL_CODE_ENABLE_PROVIDER
        prov.level,
        prov.any,
        prov.all,
        0,
        &mut *parameters,
    ); }
    
    let mut trace = EventTraceLogfile::default();
    trace.0.LoggerName = unsafe { session_name_pwstr.abi() };
    trace.0.Anonymous1.ProcessTraceMode = Etw::PROCESS_TRACE_MODE_REAL_TIME | Etw::PROCESS_TRACE_MODE_EVENT_RECORD;
    let mut cb: &mut dyn FnMut(&EventRecord) = &mut callback;
    trace.0.Context = unsafe { std::mem::transmute(&mut cb) };
    trace.0.Anonymous2.EventRecordCallback = trace_callback_thunk as *mut _;

    let session_handle = unsafe { Etw::OpenTraceW(&mut *trace) };
    if session_handle == INVALID_TRACE_HANDLE {
        println!("{} {:?}", unsafe { GetLastError().0 }, windows::runtime::Error::from_win32());

        panic!("Invalid handle");
    }
    println!("OpenTrace {}", session_handle);
    let status = unsafe { Etw::ProcessTrace(&session_handle, 1, std::ptr::null_mut(), std::ptr::null_mut()) };
    println!("status: {}", status);
}

pub fn print_property(parser: &mut Parser, property: &Property) {
    print!("  {}= ", property.name);
    if let Some(map_info) = &property.map_info {
        let value = match property.desc {
            PropertyDesc::Primitive(PrimitiveDesc{ in_type: TdhInType::InTypeUInt32, ..}) => TryParse::<u32>::parse(parser, &property.name),
            PropertyDesc::Primitive(PrimitiveDesc{ in_type: TdhInType::InTypeUInt16, ..}) => TryParse::<u16>::parse(parser, &property.name) as u32,
            PropertyDesc::Primitive(PrimitiveDesc{ in_type: TdhInType::InTypeUInt8, ..}) => TryParse::<u8>::parse(parser, &property.name) as u32,
            _ => panic!("{:?}", property.desc)
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
            println!("{}", map_info.map.get(&value).map(|x| Cow::from(x)).unwrap_or_else(|| Cow::from(format!("Unknown: {}", value))));
        }
    } else {
        let value = match &property.desc {
            PropertyDesc::Primitive(desc) => {
                match desc.in_type {
                    TdhInType::InTypeUnicodeString => TryParse::<String>::try_parse(parser, &property.name),
                    TdhInType::InTypeAnsiString => TryParse::<String>::try_parse(parser, &property.name),
                    TdhInType::InTypeBoolean => TryParse::<bool>::try_parse(parser, &property.name).map(|x| x.to_string()),
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


