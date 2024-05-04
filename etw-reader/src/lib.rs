use windows::core::{h, HSTRING, PWSTR};
use windows::Win32::Foundation::{
    GetLastError, ERROR_INSUFFICIENT_BUFFER, ERROR_MORE_DATA, MAX_PATH,
};
use windows::Win32::System::Diagnostics::Etw::{
    EnumerateTraceGuids, EnumerateTraceGuidsEx, TraceGuidQueryInfo, TraceGuidQueryList,
    CONTROLTRACE_HANDLE, EVENT_TRACE_FLAG, TRACE_GUID_INFO, TRACE_GUID_PROPERTIES,
    TRACE_PROVIDER_INSTANCE_INFO,
};

use crate::parser::{Parser, ParserError, TryParse};
use crate::schema::SchemaLocator;
use crate::tdh_types::{PrimitiveDesc, PropertyDesc, TdhInType};
use crate::traits::EncodeUtf16;

#[macro_use]
extern crate memoffset;

use std::borrow::Cow;
use std::collections::HashMap;
use std::hash::BuildHasherDefault;
use std::mem;
use std::path::Path;

use etw_types::EventRecord;
use fxhash::FxHasher;
use tdh_types::{Property, TdhOutType};
use windows::Win32::System::Diagnostics::Etw;

// typedef ULONG64 TRACEHANDLE, *PTRACEHANDLE;
pub(crate) type TraceHandle = u64;
pub const INVALID_TRACE_HANDLE: TraceHandle = u64::MAX;
//, WindowsProgramming};

#[allow(non_upper_case_globals, non_camel_case_types)]
pub mod custom_schemas;
pub mod etw_types;
pub mod parser;
pub mod property;
pub mod schema;
pub mod sddl;
pub mod tdh;
pub mod tdh_types;
pub mod traits;
pub mod utils;
//pub mod trace;
//pub mod provider;

pub use windows::core::GUID;

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

unsafe extern "system" fn trace_callback_thunk(event_record: *mut Etw::EVENT_RECORD) {
    let f: &mut &mut dyn FnMut(&EventRecord) = &mut *((*event_record).UserContext
        as *mut &mut dyn for<'a> std::ops::FnMut(&'a etw_types::EventRecord));
    f(&*(event_record as *const etw_types::EventRecord))
}

pub fn open_trace<F: FnMut(&EventRecord)>(
    path: &Path,
    mut callback: F,
) -> Result<(), std::io::Error> {
    let mut log_file = EventTraceLogfile::default();

    #[cfg(windows)]
    let path = HSTRING::from(path.as_os_str());
    #[cfg(not(windows))]
    let path = HSTRING::from(path.to_string_lossy().to_string());
    log_file.0.LogFileName = PWSTR(path.as_wide().as_ptr() as *mut _);
    log_file.0.Anonymous1.ProcessTraceMode =
        Etw::PROCESS_TRACE_MODE_EVENT_RECORD | Etw::PROCESS_TRACE_MODE_RAW_TIMESTAMP;
    let mut cb: &mut dyn FnMut(&EventRecord) = &mut callback;
    log_file.0.Context = unsafe { std::mem::transmute(&mut cb) };
    log_file.0.Anonymous2.EventRecordCallback = Some(trace_callback_thunk);

    let session_handle = unsafe { Etw::OpenTraceW(&mut *log_file) };
    let result = unsafe { Etw::ProcessTrace(&[session_handle], None, None) };
    result
        .ok()
        .map_err(|e| std::io::Error::from_raw_os_error(e.code().0))
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
    ) {
        self.properties.Wnode.BufferSize = std::mem::size_of::<TraceInfo>() as u32;
        self.properties.Wnode.Guid = GUID::new().unwrap();
        self.properties.Wnode.Flags = Etw::WNODE_FLAG_TRACED_GUID;
        self.properties.Wnode.ClientContext = 1; // QPC clock resolution
        self.properties.FlushTimer = 1;

        self.properties.LogFileMode =
            Etw::EVENT_TRACE_REAL_TIME_MODE | Etw::EVENT_TRACE_NO_PER_PROCESSOR_BUFFERING;

        self.properties.EnableFlags = EVENT_TRACE_FLAG(0);

        //self.properties.LoggerNameOffset = offset_of!(TraceInfo, log_file_name) as u32;
        //self.trace_name[..trace_name.len()].copy_from_slice(trace_name.as_bytes())

        // it doesn't seem like it matters if we fill in trace_name
        self.properties.LoggerNameOffset = offset_of!(TraceInfo, trace_name) as u32;
        self.properties.LogFileNameOffset = offset_of!(TraceInfo, log_file_name) as u32;
        self.trace_name[..trace_name.len() + 1].copy_from_slice(&trace_name.to_utf16())
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

impl Default for Provider {
    fn default() -> Self {
        Self::new()
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

pub fn start_trace<F: FnMut(&EventRecord)>(mut callback: F) {
    // let guid_str = "22fb2cd6-0e7b-422b-a0c7-2fad1fd0e716";
    let guid_str = "DB6F6DDB-AC77-4E88-8253-819DF9BBF140";
    let video_blt_guid = GUID::from(guid_str); //GUID::from("DB6F6DDB-AC77-4E88-8253-819DF9BBF140");

    let session_name = h!("aaaaaa");

    let mut info = TraceInfo::default();
    info.fill(&session_name.to_string());

    let mut handle = CONTROLTRACE_HANDLE { Value: 0 };

    unsafe {
        let status = Etw::ControlTraceW(
            handle,
            session_name,
            &info.properties as *const _ as *mut _,
            Etw::EVENT_TRACE_CONTROL_STOP,
        );
        println!("ControlTrace = {:?}", status);
        let status = Etw::StartTraceW(
            &mut handle,
            session_name,
            &info.properties as *const _ as *mut _,
        );
        println!("StartTrace = {:?} handle {:?}", status, handle);
        info.trace_name = [0; 260];

        let status = Etw::ControlTraceW(
            handle,
            session_name,
            &mut info.properties,
            Etw::EVENT_TRACE_CONTROL_QUERY,
        );
        println!(
            "ControlTrace = {:?} {} {:?} {:?}",
            status, info.properties.BufferSize, info.properties.LoggerThreadId, info.trace_name
        );
    }

    let prov = Provider::new().by_guid(guid_str);
    let parameters = EnableTraceParameters::create(video_blt_guid, prov.trace_flags);
    /*
    let status = unsafe { Etw::EnableTrace(1, 0xffffffff, Etw::TRACE_LEVEL_VERBOSE, &video_blt_guid, handle)};
    println!("EnableTrace = {}", status);
    */
    unsafe {
        let _ = Etw::EnableTraceEx2(
            handle,
            &video_blt_guid,
            1, // Fixme: EVENT_CONTROL_CODE_ENABLE_PROVIDER
            prov.level,
            prov.any,
            prov.all,
            0,
            Some(&*parameters),
        );
    }

    let mut trace = EventTraceLogfile::default();
    trace.0.LoggerName = PWSTR(session_name.as_ptr() as *mut _);
    trace.0.Anonymous1.ProcessTraceMode =
        Etw::PROCESS_TRACE_MODE_REAL_TIME | Etw::PROCESS_TRACE_MODE_EVENT_RECORD;
    let mut cb: &mut dyn FnMut(&EventRecord) = &mut callback;
    trace.0.Context = unsafe { std::mem::transmute(&mut cb) };
    trace.0.Anonymous2.EventRecordCallback = Some(trace_callback_thunk);

    let session_handle = unsafe { Etw::OpenTraceW(&mut *trace) };
    if session_handle.Value == INVALID_TRACE_HANDLE {
        println!(
            "{:?} {:?}",
            unsafe { GetLastError() },
            windows::core::Error::from_win32()
        );

        panic!("Invalid handle");
    }
    println!("OpenTrace {:?}", session_handle);
    let status = unsafe { Etw::ProcessTrace(&[session_handle], None, None) };
    println!("status: {:?}", status);
}

pub fn event_properties_to_string(
    s: &schema::TypedEvent,
    parser: &mut Parser,
    skip_properties: Option<&[&str]>,
) -> String {
    let mut text = String::new();
    for i in 0..s.property_count() {
        let property = s.property(i);
        if let Some(propfilter) = skip_properties {
            if propfilter.iter().any(|&s| s == property.name) {
                continue;
            }
        }

        write_property(&mut text, parser, &property, false);
        text += ", "
    }

    text
}

pub fn write_property(
    output: &mut dyn std::fmt::Write,
    parser: &mut Parser,
    property: &Property,
    write_types: bool,
) {
    if write_types {
        let type_name = if let PropertyDesc::Primitive(prim) = &property.desc {
            format!("{:?}", prim.in_type)
        } else {
            format!("{:?}", property.desc)
        };
        if property.flags.is_empty() {
            write!(output, "  {}: {} = ", property.name, type_name).unwrap();
        } else {
            write!(
                output,
                "  {}({:?}): {} = ",
                property.name, property.flags, type_name
            )
            .unwrap();
        }
    } else {
        write!(output, "  {}= ", property.name).unwrap();
    }
    if let Some(map_info) = &property.map_info {
        let value = match property.desc {
            PropertyDesc::Primitive(PrimitiveDesc {
                in_type: TdhInType::InTypeUInt32,
                ..
            }) => TryParse::<u32>::parse(parser, &property.name),
            PropertyDesc::Primitive(PrimitiveDesc {
                in_type: TdhInType::InTypeUInt16,
                ..
            }) => TryParse::<u16>::parse(parser, &property.name) as u32,
            PropertyDesc::Primitive(PrimitiveDesc {
                in_type: TdhInType::InTypeUInt8,
                ..
            }) => TryParse::<u8>::parse(parser, &property.name) as u32,
            _ => panic!("{:?}", property.desc),
        };
        if map_info.is_bitmap {
            let remaining_bits_str;
            let mut matches: Vec<&str> = Vec::new();
            let mut cleared_value = value;
            for (k, v) in &map_info.map {
                if value & k != 0 {
                    matches.push(v.trim());
                    cleared_value &= !k;
                }
            }
            if cleared_value != 0 {
                remaining_bits_str = format!("{:x}", cleared_value);
                matches.push(&remaining_bits_str);
                //println!("unnamed bits {:x} {:x} {:x?}", value, cleared_value, map_info.map);
            }
            write!(output, "{}", matches.join(" | ")).unwrap();
        } else {
            write!(
                output,
                "{}",
                map_info
                    .map
                    .get(&value)
                    .map(Cow::from)
                    .unwrap_or_else(|| Cow::from(format!("Unknown: {}", value)))
            )
            .unwrap();
        }
    } else {
        let value = match &property.desc {
            PropertyDesc::Primitive(desc) => {
                // XXX: we should be using the out_type here instead of in_type
                match desc.in_type {
                    TdhInType::InTypeUnicodeString => {
                        TryParse::<String>::try_parse(parser, &property.name)
                    }
                    TdhInType::InTypeAnsiString => {
                        TryParse::<String>::try_parse(parser, &property.name)
                    }
                    TdhInType::InTypeBoolean => {
                        TryParse::<bool>::try_parse(parser, &property.name).map(|x| x.to_string())
                    }
                    TdhInType::InTypeHexInt32 => {
                        TryParse::<i32>::try_parse(parser, &property.name).map(|x| x.to_string())
                    }
                    TdhInType::InTypeUInt32 => {
                        TryParse::<u32>::try_parse(parser, &property.name).map(|x| x.to_string())
                    }
                    TdhInType::InTypeUInt16 => {
                        TryParse::<u16>::try_parse(parser, &property.name).map(|x| x.to_string())
                    }
                    TdhInType::InTypeUInt8 => {
                        TryParse::<u8>::try_parse(parser, &property.name).map(|x| x.to_string())
                    }
                    TdhInType::InTypeInt8 => {
                        TryParse::<i8>::try_parse(parser, &property.name).map(|x| x.to_string())
                    }
                    TdhInType::InTypeInt64 => {
                        TryParse::<i64>::try_parse(parser, &property.name).map(|x| x.to_string())
                    }
                    TdhInType::InTypeUInt64 => {
                        let i = TryParse::<u64>::try_parse(parser, &property.name);
                        if desc.out_type == TdhOutType::OutTypeHexInt64 {
                            i.map(|x| format!("0x{:x}", x))
                        } else {
                            i.map(|x| x.to_string())
                        }
                    }
                    TdhInType::InTypeHexInt64 => {
                        let i = TryParse::<i64>::try_parse(parser, &property.name);
                        i.map(|x| format!("0x{:x}", x))
                    }
                    TdhInType::InTypePointer | TdhInType::InTypeSizeT => {
                        TryParse::<u64>::try_parse(parser, &property.name)
                            .map(|x| format!("0x{:x}", x))
                    }
                    TdhInType::InTypeGuid => TryParse::<GUID>::try_parse(parser, &property.name)
                        .map(|x| format!("{:?}", x)),
                    TdhInType::InTypeInt32 => {
                        TryParse::<i32>::try_parse(parser, &property.name).map(|x| x.to_string())
                    }
                    TdhInType::InTypeFloat => {
                        TryParse::<f32>::try_parse(parser, &property.name).map(|x| x.to_string())
                    }
                    _ => Ok(format!("Unknown {:?} -> {:?}", desc.in_type, desc.out_type)),
                }
            }
            PropertyDesc::Struct(desc) => Ok(format!(
                "unhandled struct {} {}",
                desc.start_index, desc.num_members
            )),
        };
        let value = match value {
            Ok(value) => value,
            Err(ParserError::InvalidType) => format!("invalid type {:?}", property.desc),
            Err(ParserError::LengthMismatch) => format!(
                "Err(LengthMismatch) type: {:?}, flags: {:?}, buf: {}",
                property.desc,
                property.flags,
                parser.buffer.len()
            ),
            Err(e) => format!("Err({:?}) type: {:?}", e, property.desc),
        };
        write!(output, "{}", value).unwrap();
    }
}

pub fn print_property(parser: &mut Parser, property: &Property, write_types: bool) {
    let mut result = String::new();
    write_property(&mut result, parser, property, write_types);
    println!("{}", result);
}

pub fn add_custom_schemas(locator: &mut SchemaLocator) {
    locator.add_custom_schema(Box::new(custom_schemas::ImageID {}));
    locator.add_custom_schema(Box::new(custom_schemas::DbgID {}));
    locator.add_custom_schema(Box::new(custom_schemas::EventInfo {}));
    locator.add_custom_schema(Box::new(custom_schemas::ThreadStart {}));
    locator.add_custom_schema(Box::new(custom_schemas::D3DUmdLogging_MapAllocation {}));
    locator.add_custom_schema(Box::new(custom_schemas::D3DUmdLogging_RundownAllocation {}));
    locator.add_custom_schema(Box::new(custom_schemas::D3DUmdLogging_UnmapAllocation {}));
}

pub fn enumerate_trace_guids() {
    let mut count = 1;
    loop {
        let mut guids: Vec<TRACE_GUID_PROPERTIES> =
            vec![unsafe { std::mem::zeroed() }; count as usize];
        let mut ptrs: Vec<*mut TRACE_GUID_PROPERTIES> = Vec::new();
        for guid in &mut guids {
            ptrs.push(guid)
        }

        let result = unsafe { EnumerateTraceGuids(ptrs.as_mut_slice(), &mut count) };
        match result.ok() {
            Ok(()) => {
                for guid in guids[..count as usize].iter() {
                    println!("{:?}", guid.Guid);
                }
                break;
            }
            Err(e) => {
                if e.code() != ERROR_MORE_DATA.to_hresult() {
                    break;
                }
            }
        }
    }
}

pub fn enumerate_trace_guids_ex(print_instances: bool) {
    let mut required_size: u32 = 0;

    loop {
        let mut guids: Vec<GUID> =
            vec![GUID::zeroed(); required_size as usize / mem::size_of::<GUID>()];

        let size = (guids.len() * mem::size_of::<GUID>()) as u32;
        println!("get {}", required_size);

        let result = unsafe {
            EnumerateTraceGuidsEx(
                TraceGuidQueryList,
                None,
                0,
                Some(guids.as_mut_ptr() as *mut _),
                size,
                &mut required_size as *mut _,
            )
        };
        match result.ok() {
            Ok(()) => {
                for guid in guids.iter() {
                    println!("{:?}", guid);
                    let info = get_provider_info(guid);
                    let instance_count =
                        unsafe { *(info.as_ptr() as *const TRACE_GUID_INFO) }.InstanceCount;
                    let mut instance_ptr: *const TRACE_PROVIDER_INSTANCE_INFO = unsafe {
                        info.as_ptr().add(mem::size_of::<TRACE_GUID_INFO>())
                            as *const TRACE_PROVIDER_INSTANCE_INFO
                    };

                    for _ in 0..instance_count {
                        let instance = unsafe { &*instance_ptr };
                        if print_instances {
                            println!(
                                "enable_count {}, pid {}, flags {}",
                                instance.EnableCount, instance.Pid, instance.Flags,
                            )
                        }
                        instance_ptr = unsafe {
                            (instance_ptr as *const u8).add(instance.NextOffset as usize)
                                as *const TRACE_PROVIDER_INSTANCE_INFO
                        };
                    }
                }
                break;
            }
            Err(e) => {
                if e.code() != ERROR_INSUFFICIENT_BUFFER.to_hresult() {
                    println!("some other error");
                    break;
                }
            }
        }
    }
}

pub fn get_provider_info(guid: &GUID) -> Vec<u8> {
    let mut required_size: u32 = 0;

    loop {
        let mut info: Vec<u8> = vec![0; required_size as usize];

        let size = info.len() as u32;

        let result = unsafe {
            EnumerateTraceGuidsEx(
                TraceGuidQueryInfo,
                Some(guid as *const GUID as *const _),
                mem::size_of::<GUID>() as u32,
                Some(info.as_mut_ptr() as *mut _),
                size,
                &mut required_size as *mut _,
            )
        };
        match result.ok() {
            Ok(()) => {
                return info;
            }
            Err(e) => {
                if e.code() != ERROR_INSUFFICIENT_BUFFER.to_hresult() {
                    panic!("{:?}", e);
                }
            }
        }
    }
}
