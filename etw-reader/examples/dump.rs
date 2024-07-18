use std::collections::HashMap;
use std::path::Path;

use etw_reader::parser::{Parser, TryParse};
use etw_reader::schema::SchemaLocator;
use etw_reader::{open_trace, print_property, GUID};
use windows::Win32::System::Diagnostics::Etw::{
    self, EtwProviderTraitDecodeGuid, EtwProviderTraitTypeGroup,
};

fn main() {
    let mut schema_locator = SchemaLocator::new();
    etw_reader::add_custom_schemas(&mut schema_locator);
    let pattern = std::env::args().nth(2);
    let mut processes = HashMap::new();
    open_trace(Path::new(&std::env::args().nth(1).unwrap()), |e| {
        //dbg!(e.EventHeader.TimeStamp);

        let s = schema_locator.event_schema(e);
        if let Ok(s) = s {
            if let "MSNT_SystemTrace/Process/Start"
            | "MSNT_SystemTrace/Process/DCStart"
            | "MSNT_SystemTrace/Process/DCEnd" = s.name()
            {
                let mut parser = Parser::create(&s);

                let image_file_name: String = parser.parse("ImageFileName");
                let process_id: u32 = parser.parse("ProcessId");
                processes.insert(process_id, image_file_name);
            }

            if let Some(pattern) = &pattern {
                if !s.name().contains(pattern) {
                    return;
                }
            }
            println!(
                "{:?} {} {} {}-{} {} {}, processor {}",
                e.EventHeader.ProviderId,
                s.name(),
                s.provider_name(),
                e.EventHeader.EventDescriptor.Opcode,
                e.EventHeader.EventDescriptor.Id,
                s.property_count(),
                e.EventHeader.TimeStamp,
                unsafe { e.BufferContext.Anonymous.ProcessorIndex },
            );
            println!(
                "pid: {} {:?}",
                s.process_id(),
                processes.get(&s.process_id())
            );
            if e.EventHeader.ActivityId != GUID::zeroed() {
                println!("ActivityId: {:?}", e.EventHeader.ActivityId);
            }
            if e.ExtendedDataCount > 0 {
                let items = unsafe {
                    std::slice::from_raw_parts(e.ExtendedData, e.ExtendedDataCount as usize)
                };
                for i in items {
                    match i.ExtType as u32 {
                        Etw::EVENT_HEADER_EXT_TYPE_EVENT_SCHEMA_TL => {
                            println!("extended: SCHEMA_TL");
                            let data: &[u8] = unsafe {
                                std::slice::from_raw_parts(
                                    i.DataPtr as *const u8,
                                    i.DataSize as usize,
                                )
                            };

                            // from TraceLoggingProvider.h
                            let size =
                                u16::from_ne_bytes(<[u8; 2]>::try_from(&data[0..2]).unwrap());
                            println!("  size: {}", size);
                            let mut extension_size = 1;
                            while data[2 + extension_size] & 0x80 != 0 {
                                extension_size += 1;
                            }
                            println!("  extension: {:?}", &data[2..2 + extension_size]);
                            let name_start = 2 + extension_size;
                            let mut name_end = name_start;
                            while data[name_end] != 0 {
                                name_end += 1;
                            }
                            let name =
                                String::from_utf8(data[name_start..name_end].to_owned()).unwrap();
                            println!("  name: {}", name);

                            let mut field_start = name_end + 1;

                            while field_start < data.len() {
                                let field_name_start = field_start;
                                let mut field_name_end = field_name_start;
                                while data[field_name_end] != 0 {
                                    field_name_end += 1;
                                }
                                let field_name = String::from_utf8(
                                    data[field_name_start..field_name_end].to_owned(),
                                )
                                .unwrap();
                                println!("  field_name: {}", field_name);
                                let mut field_pos = field_name_end + 1;
                                let field_in_type = data[field_pos];
                                dbg!(field_in_type);
                                field_pos += 1;
                                if field_in_type & 128 == 128 {
                                    let field_out_type = data[field_pos];
                                    field_pos += 1;
                                    dbg!(field_out_type);
                                    if field_out_type & 128 == 128 {
                                        // field extension
                                        println!("  field extension");
                                        while data[field_pos] & 0x80 != 0 {
                                            field_pos += 1;
                                        }
                                        field_pos += 1;
                                    }
                                }
                                let c_count = 32;
                                let v_count = 64;
                                let custom = v_count | c_count;
                                let count_mask = v_count | c_count;
                                if field_in_type & count_mask == c_count {
                                    // value count
                                    field_pos += 2
                                }
                                if field_in_type & count_mask == custom {
                                    let type_info_size = u16::from_ne_bytes(
                                        <[u8; 2]>::try_from(&data[field_pos..field_pos + 2])
                                            .unwrap(),
                                    );
                                    field_pos += 2;
                                    field_pos += type_info_size as usize;
                                }
                                field_start = field_pos;
                            }
                        }
                        Etw::EVENT_HEADER_EXT_TYPE_PROV_TRAITS => {
                            println!("extended: PROV_TRAITS");
                            let data: &[u8] = unsafe {
                                std::slice::from_raw_parts(
                                    i.DataPtr as *const u8,
                                    i.DataSize as usize,
                                )
                            };
                            // ProviderMetadata
                            let size =
                                u16::from_ne_bytes(<[u8; 2]>::try_from(&data[0..2]).unwrap());
                            println!("  size: {}", size);
                            let name_start = 2;
                            let mut name_end = name_start;
                            while data[name_end] != 0 {
                                name_end += 1;
                            }
                            let name =
                                String::from_utf8(data[name_start..name_end].to_owned()).unwrap();
                            println!("  name: {}", name);
                            let mut metadata_start = name_end + 1;
                            // ProviderMetadataChunk
                            while metadata_start < data.len() {
                                let metadata_size = u16::from_ne_bytes(
                                    <[u8; 2]>::try_from(&data[metadata_start..metadata_start + 2])
                                        .unwrap(),
                                );
                                let metadata_type = data[metadata_start + 2];
                                println!("  metadata_size: {}", metadata_size);
                                if metadata_type as i32 == EtwProviderTraitTypeGroup.0 {
                                    println!("  EtwProviderTraitTypeGroup");
                                    // read GUID
                                    let guid = &data[(metadata_start + 3)..];
                                    let guid = GUID::from_values(
                                        u32::from_ne_bytes((&guid[0..4]).try_into().unwrap()),
                                        u16::from_ne_bytes((&guid[4..6]).try_into().unwrap()),
                                        u16::from_ne_bytes((&guid[6..8]).try_into().unwrap()),
                                        [
                                            guid[8], guid[9], guid[10], guid[11], guid[12],
                                            guid[13], guid[14], guid[15],
                                        ],
                                    );
                                    println!("  GUID {:?}", guid);
                                } else if metadata_type as i32 == EtwProviderTraitDecodeGuid.0 {
                                    println!("  EtwProviderTraitDecodeGuid");
                                } else {
                                    println!("  Unexpected {}", metadata_type);
                                }
                                metadata_start += metadata_size as usize;
                            }
                        }
                        _ => {
                            println!("extended: {:?}", i);
                        }
                    }
                }
            }
            let formatted_message = s.event_message();
            if let Some(message) = formatted_message {
                println!("message: {}", message);
            }
            let mut parser = Parser::create(&s);
            for i in 0..s.property_count() {
                let property = s.property(i);
                //dbg!(&property);
                print_property(&mut parser, &property, true);
            }
        } else if pattern.is_none() {
            println!(
                "unknown event {:x?}:{} size: {}",
                e.EventHeader.ProviderId, e.EventHeader.EventDescriptor.Opcode, e.UserDataLength
            );
        }
    })
    .unwrap();
}
