use windows::Guid;

use crate::{etw_types::DecodingSource, schema::EventSchema, tdh_types::{Property, PropertyFlags, TdhInType, TdhOutType}};



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
    fn provider_guid(&self) -> Guid {
        Guid::from("b3e675d7-2554-4f18-830b-2762732560de")
    }

    fn event_id(&self) -> u16 {
        0
    }

    fn event_version(&self) -> u8 {
        0
    }

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
            in_type: prop.in_type,
        out_type: prop.out_type,
        length: 0,
        flags: PropertyFlags::empty()}
    }
}


pub struct DbgID {}

const DbgID_PROPS: [PropDesc; 5] = [
    PropDesc{ name: "ImageBase", in_type: TdhInType::InTypeInt64, out_type: TdhOutType::OutTypeHexInt64},
    PropDesc{ name: "ProcessId", in_type: TdhInType::InTypeUInt32, out_type: TdhOutType::OutTypeUInt32},
    PropDesc{ name: "GuidSig", in_type: TdhInType::InTypeGuid, out_type: TdhOutType::OutTypeGuid},
    PropDesc{ name: "Age", in_type: TdhInType::InTypeUInt32, out_type: TdhOutType::OutTypeUInt32},
    PropDesc{ name: "PdbFileName", in_type: TdhInType::InTypeAnsiString, out_type: TdhOutType::OutTypeString},
    ];

impl EventSchema for DbgID {
    fn provider_guid(&self) -> Guid {
        Guid::from("b3e675d7-2554-4f18-830b-2762732560de")
    }

    fn event_id(&self) -> u16 {
        36
    }

    fn event_version(&self) -> u8 {
        0
    }

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
            in_type: prop.in_type,
        out_type: prop.out_type,
        length: 0,
        flags: PropertyFlags::empty()}
    }
}