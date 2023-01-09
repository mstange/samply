//! Basic TDH types
//!
//! The `tdh_type` module provides an abstraction over the basic TDH types, this module act as a
//! helper for the parser to determine which IN and OUT type are expected from a property within an
//! event
//!
//! This is a bit extra but is basically a redefinition of the In an Out TDH types following the
//! rust naming convention, it can also come in handy when implementing the [TryParse] trait for a type
//! to determine how to handle a [Property] based on this values
//!
//! [TryParse]: crate::parser::TryParse
//! [Property]: crate::native::tdh_types::Property
use windows::Win32::System::Diagnostics::Etw;


use crate::etw_types::EventPropertyInfo;
use num_traits::FromPrimitive;

#[derive(Debug, Clone, Default)]
pub struct PropertyMapInfo {
    pub is_bitmap: bool,
    pub map: crate::FastHashMap<u32, String>
}
#[derive(Debug, Clone)]
pub struct PrimitiveDesc {
    pub in_type: TdhInType,
    pub out_type: TdhOutType,
}

#[derive(Debug, Clone, Default)]
pub struct StructDesc {
    pub start_index: u16,
    pub num_members: u16,
}

#[derive(Debug, Clone)]
pub enum PropertyDesc {
    Primitive(PrimitiveDesc),
    Struct(StructDesc),
}

/// Attributes of a property
#[derive(Debug, Clone)]
pub struct Property {
    /// Name of the Property
    pub name: String,
    /// Represent the [PropertyFlags]
    pub flags: PropertyFlags,
    pub length: u16,
    pub desc: PropertyDesc,
    pub map_info: Option<PropertyMapInfo>,
    pub count: u16,
}

#[doc(hidden)]
impl Property {
    pub fn new(name: String, property: &EventPropertyInfo, map_info: Option<PropertyMapInfo>) -> Self {
        if property.Flags.0 & Etw::PropertyStruct.0 != 0 {
            unsafe {
                let start_index = property.Anonymous1.structType.StructStartIndex;
                let num_members = property.Anonymous1.structType.NumOfStructMembers;
                
                Property {
                    name,
                    flags: PropertyFlags::from(property.Flags),
                    length: property.Anonymous3.length,
                    desc: PropertyDesc::Struct(StructDesc{start_index, num_members}),
                    map_info,
                    count: property.Anonymous2.count,
                }
            }
        } else {
            unsafe {
                let out_type = FromPrimitive::from_u16(property.Anonymous1.nonStructType.OutType)
                    .unwrap_or(TdhOutType::OutTypeNull);
                let in_type = FromPrimitive::from_u16(property.Anonymous1.nonStructType.InType)
                    .unwrap_or_else(|| panic!("{:?}", property.Anonymous1.nonStructType.InType));
                
                Property {
                    name,
                    flags: PropertyFlags::from(property.Flags),
                    length: property.Anonymous3.length,
                    desc: PropertyDesc::Primitive(PrimitiveDesc{in_type, out_type}),
                    map_info,
                    count: property.Anonymous2.count,
                }
            }
        }
    }

    pub fn len(&self) -> usize {
        self.length.clone() as usize
    }
}

/// Represent a TDH_IN_TYPE
#[repr(u16)]
#[derive(Debug, Clone, Copy, FromPrimitive, ToPrimitive, PartialEq)]
pub enum TdhInType {
    // Deprecated values are not defined
    InTypeNull,
    InTypeUnicodeString,
    InTypeAnsiString,
    InTypeInt8,    // Field size is 1 byte
    InTypeUInt8,   // Field size is 1 byte
    InTypeInt16,   // Field size is 2 bytes
    InTypeUInt16,  // Field size is 2 bytes
    InTypeInt32,   // Field size is 4 bytes
    InTypeUInt32,  // Field size is 4 bytes
    InTypeInt64,   // Field size is 8 bytes
    InTypeUInt64,  // Field size is 8 bytes
    InTypeFloat,   // Field size is 4 bytes
    InTypeDouble,  // Field size is 8 bytes
    InTypeBoolean, // Field size is 4 bytes
    InTypeBinary,  // Depends on the OutType
    InTypeGuid,
    InTypePointer,
    InTypeFileTime,   // Field size is 8 bytes
    InTypeSystemTime, // Field size is 16 bytes
    InTypeSid,        // Field size determined by the first few bytes of the field
    InTypeHexInt32,
    InTypeHexInt64,
    InTypeCountedString = 300,
    InTypeCountedAnsiString,
    InTypeReverseCountedString,
    InTypeReverseCountedAnsiString,
    InTypeNonNullTerminatedString,
    InTypeNonNullTerminatedAnsiString,
    InTypeUnicodeChar,
    InTypeAnsiChar,
    InTypeSizeT,
    InTypeHexdump,
    InTypeWBEMSID,
}



/// Represent a TDH_OUT_TYPE
#[repr(u16)]
#[derive(Debug, Clone, Copy, FromPrimitive, ToPrimitive, PartialEq)]
pub enum TdhOutType {
    OutTypeNull,
    OutTypeString,
    OutTypeDateTime,
    OutTypeInt8,    // Field size is 1 byte
    OutTypeUInt8,   // Field size is 1 byte
    OutTypeInt16,   // Field size is 2 bytes
    OutTypeUInt16,  // Field size is 2 bytes
    OutTypeInt32,   // Field size is 4 bytes
    OutTypeUInt32,  // Field size is 4 bytes
    OutTypeInt64,   // Field size is 8 bytes
    OutTypeUInt64,  // Field size is 8 bytes
    OutTypeFloat,   // Field size is 4 bytes
    OutTypeDouble,  // Field size is 8 bytes
    OutTypeBoolean, // Field size is 4 bytes
    OutTypeGuid,
    OutTypeHexBinary,
    OutTypeHexInt8,
    OutTypeHexInt16,
    OutTypeHexInt32,
    OutTypeHexInt64,
    OutTypePid,
    OutTypeTid,
    OutTypePort,
    OutTypeIpv4,
    OutTypeIpv6,
    OutTypeWin32Error = 30,
    OutTypeNtStatus = 31,
    OutTypeHResult = 32,
    OutTypeJson = 34,
    OutTypeUtf8 = 35,
    OutTypePkcs7 = 36,
    OutTypeCodePointer = 37,
    OutTypeDatetimeUtc = 38,
}

impl Default for TdhOutType {
    fn default() -> TdhOutType {
        TdhOutType::OutTypeNull
    }
}

bitflags! {
    /// Represents the Property flags
    ///
    /// See: [Property Flags enum](https://docs.microsoft.com/en-us/windows/win32/api/tdh/ne-tdh-property_flags)
    #[derive(Default)]
    pub struct PropertyFlags: u32 {
        const PROPERTY_STRUCT = 0x1;
        const PROPERTY_PARAM_LENGTH = 0x2;
        const PROPERTY_PARAM_COUNT = 0x4;
        const PROPERTY_WBEMXML_FRAGMENT = 0x8;
        const PROPERTY_PARAM_FIXED_LENGTH = 0x10;
        const PROPERTY_PARAM_FIXED_COUNT = 0x20;
        const PROPERTY_HAS_TAGS = 0x40;
        const PROPERTY_HAS_CUSTOM_SCHEMA = 0x80;
    }
}

impl From<Etw::PROPERTY_FLAGS> for PropertyFlags {
    fn from(flags: Etw::PROPERTY_FLAGS) -> Self {
        // Should be a safe cast
        PropertyFlags::from_bits_truncate(flags.0 as u32)
    }
}
