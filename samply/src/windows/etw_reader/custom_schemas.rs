use windows::core::GUID;

use crate::{etw_types::DecodingSource, schema::EventSchema, tdh_types::{Property, PropertyDesc, PrimitiveDesc, PropertyFlags, TdhInType, TdhOutType, PropertyLength}};

struct PropDesc {
    name: &'static str,
    in_type: TdhInType,
    out_type: TdhOutType,
}

pub struct ImageID {}

const ImageID_PROPS: [PropDesc; 5] = [
    PropDesc{ name: "ImageBase", in_type: TdhInType::InTypePointer, out_type: TdhOutType::OutTypeHexInt64},
    PropDesc{ name: "ImageSize", in_type: TdhInType::InTypeUInt32, out_type: TdhOutType::OutTypeUInt32},
    PropDesc{ name: "Unknown", in_type: TdhInType::InTypePointer, out_type: TdhOutType::OutTypeUInt32},
    PropDesc{ name: "TimeDateStamp", in_type: TdhInType::InTypeUInt32, out_type: TdhOutType::OutTypeUInt32},
    PropDesc{ name: "OriginalFileName", in_type: TdhInType::InTypeUnicodeString, out_type: TdhOutType::OutTypeString},
    ];

impl EventSchema for ImageID {
    fn provider_guid(&self) -> GUID {
        GUID::from("b3e675d7-2554-4f18-830b-2762732560de")
    }

    fn event_id(&self) -> u16 {
        0
    }
    
    fn opcode(&self) -> u8 {
        0
    }

    fn event_version(&self) -> u8 {
        2
    }

    fn level(&self) -> u8 { 0 }

    fn decoding_source(&self) -> DecodingSource {
        panic!()
    }

    fn provider_name(&self) -> String {
        "KernelTraceControl".to_owned()
    }

    fn task_name(&self) -> String {
        "ImageID".to_owned()
    }

    fn opcode_name(&self) -> String {
        "".to_string()
    }
    
    fn property_count(&self) -> u32 {
        ImageID_PROPS.len() as u32
    }
    
    fn property(&self, index: u32) -> Property {
        let prop = &ImageID_PROPS[index as usize];
        Property { name: prop.name.to_owned(),
        desc: PropertyDesc::Primitive(PrimitiveDesc{ in_type: prop.in_type,
        out_type: prop.out_type,}),
        length: PropertyLength::Length(0),
        count: 1,
        map_info: None,
        flags: PropertyFlags::empty()}
    }
}


pub struct DbgID {}

const DbgID_PROPS: [PropDesc; 5] = [
    PropDesc{ name: "ImageBase", in_type: TdhInType::InTypeUInt64, out_type: TdhOutType::OutTypeHexInt64},
    PropDesc{ name: "ProcessId", in_type: TdhInType::InTypeUInt32, out_type: TdhOutType::OutTypeUInt32},
    PropDesc{ name: "GuidSig", in_type: TdhInType::InTypeGuid, out_type: TdhOutType::OutTypeGuid},
    PropDesc{ name: "Age", in_type: TdhInType::InTypeUInt32, out_type: TdhOutType::OutTypeUInt32},
    PropDesc{ name: "PdbFileName", in_type: TdhInType::InTypeAnsiString, out_type: TdhOutType::OutTypeString},
    ];

impl EventSchema for DbgID {
    fn provider_guid(&self) -> GUID {
        GUID::from("b3e675d7-2554-4f18-830b-2762732560de")
    }

    fn event_id(&self) -> u16 {
        0
    }

    fn opcode(&self) -> u8 {
        36
    }

    fn event_version(&self) -> u8 {
        2
    }

    fn level(&self) -> u8 { 0 }

    fn decoding_source(&self) -> DecodingSource {
        panic!()
    }

    fn provider_name(&self) -> String {
        "KernelTraceControl".to_owned()
    }

    fn task_name(&self) -> String {
        "ImageID".to_owned()
    }

    fn opcode_name(&self) -> String {
        "DbgID_RSDS".to_string()
    }
    
    fn property_count(&self) -> u32 {
        DbgID_PROPS.len() as u32
    }
    
    fn property(&self, index: u32) -> Property {
        let prop = &DbgID_PROPS[index as usize];
        Property { name: prop.name.to_owned(),
            desc: PropertyDesc::Primitive(PrimitiveDesc{ in_type: prop.in_type,
                out_type: prop.out_type,}),
        count: 1,
        length: PropertyLength::Length(0),
        map_info: None,
        flags: PropertyFlags::empty()}
    }
}

 // Info about EventInfo comes from SymbolTraceEventParser.cs in PerfView
 // It contains an EVENT_DESCRIPTOR
pub struct EventInfo {}
const EventInfo_PROPS: [PropDesc; 9] = [
    PropDesc{ name: "ProviderGuid", in_type: TdhInType::InTypeGuid, out_type: TdhOutType::OutTypeGuid},
    PropDesc{ name: "EventGuid", in_type: TdhInType::InTypeGuid, out_type: TdhOutType::OutTypeGuid},
    PropDesc{ name: "EventDescriptorId", in_type: TdhInType::InTypeUInt16, out_type: TdhOutType::OutTypeUInt16},
    PropDesc{ name: "EventDescriptor.Version", in_type: TdhInType::InTypeUInt8, out_type: TdhOutType::OutTypeUInt8},
    PropDesc{ name: "EventDescriptor.Channel", in_type: TdhInType::InTypeUInt8, out_type: TdhOutType::OutTypeUInt8},
    PropDesc{ name: "EventDescriptor.Level", in_type: TdhInType::InTypeUInt8, out_type: TdhOutType::OutTypeUInt8},
    PropDesc{ name: "EventDescriptor.Opcode", in_type: TdhInType::InTypeUInt8, out_type: TdhOutType::OutTypeUInt8},
    PropDesc{ name: "EventDescriptor.Task", in_type: TdhInType::InTypeUInt16, out_type: TdhOutType::OutTypeUInt16},
    PropDesc{ name: "EventDescriptor.Keyword", in_type: TdhInType::InTypeUInt64, out_type: TdhOutType::OutTypeHexInt64},

    ];
impl EventSchema for EventInfo {
    fn provider_guid(&self) -> GUID {
        GUID::from("bbccf6c1-6cd1-48c4-80ff-839482e37671")
    }

    fn event_id(&self) -> u16 {
        0
    }

    fn opcode(&self) -> u8 {
        32
    }

    fn event_version(&self) -> u8 {
        0
    }

    fn level(&self) -> u8 { 0 }

    fn decoding_source(&self) -> DecodingSource {
        panic!()
    }

    fn provider_name(&self) -> String {
        "KernelTraceControl".to_owned()
    }

    fn task_name(&self) -> String {
        "MetaData".to_owned()
    }

    fn opcode_name(&self) -> String {
        "EventInfo".to_string()
    }
    
    fn property_count(&self) -> u32 {
        EventInfo_PROPS.len() as u32
    }

    fn is_event_metadata(&self) -> bool {
        true
    }
    
    fn property(&self, index: u32) -> Property {
        let prop = &EventInfo_PROPS[index as usize];
        Property { name: prop.name.to_owned(),
            desc: PropertyDesc::Primitive(PrimitiveDesc{ in_type: prop.in_type,
                out_type: prop.out_type,}),
        count: 1,
        length: PropertyLength::Length(0),
        map_info: None,
        flags: PropertyFlags::empty()}
    }
}
// We could override ThreadStop using the same properties to get a ThreadName there too.
pub struct ThreadStart {}

const Thread_PROPS: [PropDesc; 15] = [
    PropDesc{ name: "ProcessId", in_type: TdhInType::InTypeUInt32, out_type: TdhOutType::OutTypeHexInt32},
    PropDesc{ name: "TThreadId", in_type: TdhInType::InTypeUInt32, out_type: TdhOutType::OutTypeHexInt32},
    PropDesc{ name: "StackBase", in_type: TdhInType::InTypePointer, out_type: TdhOutType::OutTypeNull},
    PropDesc{ name: "StackLimit", in_type: TdhInType::InTypePointer, out_type: TdhOutType::OutTypeNull},
    PropDesc{ name: "UserStackBase", in_type: TdhInType::InTypePointer, out_type: TdhOutType::OutTypeNull},
    PropDesc{ name: "UserStackLimit", in_type: TdhInType::InTypePointer, out_type: TdhOutType::OutTypeNull},
    PropDesc{ name: "Affinity", in_type: TdhInType::InTypePointer, out_type: TdhOutType::OutTypeNull},
    PropDesc{ name: "Win32StartAddr", in_type: TdhInType::InTypePointer, out_type: TdhOutType::OutTypeNull},
    PropDesc{ name: "TebBase", in_type: TdhInType::InTypePointer, out_type: TdhOutType::OutTypeNull},
    PropDesc{ name: "SubProcessTag", in_type: TdhInType::InTypeUInt32, out_type: TdhOutType::OutTypeHexInt32},
    PropDesc{ name: "BasePriority", in_type: TdhInType::InTypeUInt8, out_type: TdhOutType::OutTypeNull},
    PropDesc{ name: "PagePriority", in_type: TdhInType::InTypeUInt8, out_type: TdhOutType::OutTypeNull},
    PropDesc{ name: "IoPriority", in_type: TdhInType::InTypeUInt8, out_type: TdhOutType::OutTypeNull},
    PropDesc{ name: "ThreadFlags", in_type: TdhInType::InTypeUInt8, out_type: TdhOutType::OutTypeNull},
    PropDesc{ name: "ThreadName", in_type: TdhInType::InTypeUnicodeString, out_type: TdhOutType::OutTypeString},
    ];



impl EventSchema for ThreadStart {
    fn provider_guid(&self) -> GUID {
        GUID::from("3D6FA8D1-FE05-11D0-9DDA-00C04FD7BA7C")
    }

    fn event_id(&self) -> u16 {
        0
    }

    fn opcode(&self) -> u8 {
        3
    }

    fn event_version(&self) -> u8 {
        3
    }

    fn level(&self) -> u8 { 0 }


    fn decoding_source(&self) -> DecodingSource {
        panic!()
    }

    fn provider_name(&self) -> String {
        "MSNT_SystemTrace".to_owned()
    }

    fn task_name(&self) -> String {
        "Thread".to_owned()
    }

    fn opcode_name(&self) -> String {
        "DCStart".to_string()
    }
    
    fn property_count(&self) -> u32 {
        Thread_PROPS.len() as u32
    }
    
    fn property(&self, index: u32) -> Property {
        let prop = &Thread_PROPS[index as usize];
        Property { name: prop.name.to_owned(),
            desc: PropertyDesc::Primitive(PrimitiveDesc{ in_type: prop.in_type,
                out_type: prop.out_type,}),
        length: PropertyLength::Length(0),
        count:1,
        map_info: None,
        flags: PropertyFlags::empty()}
    }
}

// from umdprovider.h
pub struct D3DUmdLogging_MapAllocation {}

const D3DUmdLogging_PROPS: [PropDesc; 6] = [
    PropDesc{ name: "hD3DAllocation", in_type: TdhInType::InTypeUInt64, out_type: TdhOutType::OutTypeHexInt64},
    PropDesc{ name: "hDxgAllocation", in_type: TdhInType::InTypeUInt64, out_type: TdhOutType::OutTypeHexInt64},
    PropDesc{ name: "Offset", in_type: TdhInType::InTypeUInt64, out_type: TdhOutType::OutTypeUInt64},
    PropDesc{ name: "Size", in_type: TdhInType::InTypeUInt32, out_type: TdhOutType::OutTypeUInt64},
    // XXX: use an enum for these
    PropDesc{ name: "Usage", in_type: TdhInType::InTypeUInt32, out_type: TdhOutType::OutTypeUInt32},
    PropDesc{ name: "Semantic", in_type: TdhInType::InTypeUInt32, out_type: TdhOutType::OutTypeUInt32},
    ];

impl EventSchema for D3DUmdLogging_MapAllocation {
    fn provider_guid(&self) -> GUID {
        GUID::from("A688EE40-D8D9-4736-B6F9-6B74935BA3B1")
    }

    fn event_id(&self) -> u16 { 1 }

    fn event_version(&self) -> u8 { 0 }

    fn opcode(&self) -> u8 { 1 }

    fn level(&self) -> u8 { 0 }

    fn decoding_source(&self) -> DecodingSource {
        panic!()
    }

    fn provider_name(&self) -> String {
        "D3DUmdLogging".to_owned()
    }

    fn task_name(&self) -> String {
        "MapAllocation".to_owned()
    }

    fn opcode_name(&self) -> String {
        "Start".to_string()
    }
    
    fn property_count(&self) -> u32 {
        D3DUmdLogging_PROPS.len() as u32
    }
    
    fn property(&self, index: u32) -> Property {
        let prop = &D3DUmdLogging_PROPS[index as usize];
        Property { name: prop.name.to_owned(),
            desc: PropertyDesc::Primitive(PrimitiveDesc{ in_type: prop.in_type,
                out_type: prop.out_type,}),
        count: 1,
        length: PropertyLength::Length(0),
        map_info: None,
        flags: PropertyFlags::empty()}
    }
}

pub struct D3DUmdLogging_RundownAllocation {}

impl EventSchema for D3DUmdLogging_RundownAllocation {
    fn provider_guid(&self) -> GUID {
        GUID::from("A688EE40-D8D9-4736-B6F9-6B74935BA3B1")
    }

    fn event_id(&self) -> u16 { 2 }

    fn event_version(&self) -> u8 { 0 }

    fn opcode(&self) -> u8 { 3 }

    fn level(&self) -> u8 { 0 }

    fn decoding_source(&self) -> DecodingSource {
        panic!()
    }

    fn provider_name(&self) -> String {
        "D3DUmdLogging".to_owned()
    }

    fn task_name(&self) -> String {
        "MapAllocation".to_owned()
    }

    fn opcode_name(&self) -> String {
        "DC Start".to_string()
    }
    
    fn property_count(&self) -> u32 {
        D3DUmdLogging_PROPS.len() as u32
    }
    
    fn property(&self, index: u32) -> Property {
        let prop = &D3DUmdLogging_PROPS[index as usize];
        Property { name: prop.name.to_owned(),
            desc: PropertyDesc::Primitive(PrimitiveDesc{ in_type: prop.in_type,
                out_type: prop.out_type,}),
        count: 1,
        length: PropertyLength::Length(0),
        map_info: None,
        flags: PropertyFlags::empty()}
    }
}

pub struct D3DUmdLogging_UnmapAllocation {}


impl EventSchema for D3DUmdLogging_UnmapAllocation {
    fn provider_guid(&self) -> GUID {
        GUID::from("A688EE40-D8D9-4736-B6F9-6B74935BA3B1")
    }

    fn event_id(&self) -> u16 { 3 }

    fn event_version(&self) -> u8 { 0 }

    fn opcode(&self) -> u8 { 2 }

    fn level(&self) -> u8 { 0 }

    fn decoding_source(&self) -> DecodingSource {
        panic!()
    }

    fn provider_name(&self) -> String {
        "D3DUmdLogging".to_owned()
    }

    fn task_name(&self) -> String {
        "MapAllocation".to_owned()
    }

    fn opcode_name(&self) -> String {
        "End".to_string()
    }
    
    fn property_count(&self) -> u32 {
        D3DUmdLogging_PROPS.len() as u32
    }
    
    fn property(&self, index: u32) -> Property {
        let prop = &D3DUmdLogging_PROPS[index as usize];
        Property { name: prop.name.to_owned(),
            desc: PropertyDesc::Primitive(PrimitiveDesc{ in_type: prop.in_type,
                out_type: prop.out_type,}),
        count: 1,
        length: PropertyLength::Length(0),
        map_info: None,
        flags: PropertyFlags::empty()}
    }
}

