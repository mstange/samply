use std::cmp::Reverse;
use std::collections::HashMap;
use std::path::Path;

use etw_reader::schema::SchemaLocator;
use etw_reader::{open_trace, GUID};

fn main() {
    let mut schema_locator = SchemaLocator::new();
    etw_reader::add_custom_schemas(&mut schema_locator);
    let mut event_counts = HashMap::new();
    open_trace(Path::new(&std::env::args().nth(1).unwrap()), |e| {
        let s = schema_locator.event_schema(e);
        if let Ok(s) = s {
            let name = format!(
                "{} {:?}/{}/{}",
                s.name(),
                e.EventHeader.ProviderId,
                e.EventHeader.EventDescriptor.Id,
                e.EventHeader.EventDescriptor.Opcode
            );
            if let Some(count) = event_counts.get_mut(&name) {
                *count += 1;
            } else {
                event_counts.insert(name, 1);
            }
        } else {
            let provider_name = match e.EventHeader.ProviderId {
                GUID {
                    data1: 0x9B79EE91,
                    data2: 0xB5FD,
                    data3: 0x41C0,
                    data4: [0xA2, 0x43, 0x42, 0x48, 0xE2, 0x66, 0xE9, 0xD0],
                } => "SysConfig ",
                GUID {
                    data1: 0xB3E675D7,
                    data2: 0x2554,
                    data3: 0x4F18,
                    data4: [0x83, 0x0B, 0x27, 0x62, 0x73, 0x25, 0x60, 0xDE],
                } => "KernelTraceControl ",
                GUID {
                    data1: 0xED54DFF8,
                    data2: 0xC409,
                    data3: 0x4CF6,
                    data4: [0xBF, 0x83, 0x05, 0xE1, 0xE6, 0x1A, 0x09, 0xC4],
                } => "WinSat ",
                // see https://docs.microsoft.com/en-us/windows-hardware/drivers/ddi/umdprovider/nf-umdprovider-umdetwregister
                GUID {
                    data1: 0xa688ee40,
                    data2: 0xd8d9,
                    data3: 0x4736,
                    data4: [0xb6, 0xf9, 0x6b, 0x74, 0x93, 0x5b, 0xa3, 0xb1],
                } => "D3DUmdLogging ",
                GUID {
                    data1: 0x3d6fa8d3,
                    data2: 0xfe05,
                    data3: 0x11d0,
                    data4: [0x9d, 0xda, 0x00, 0xc0, 0x4f, 0xd7, 0xba, 0x7c],
                } => "PageFault_V2 ",
                _ => "",
            };

            let provider = format!(
                "{}{:?}/{}-{}/{}",
                provider_name,
                e.EventHeader.ProviderId,
                e.EventHeader.EventDescriptor.Id,
                e.EventHeader.EventDescriptor.Version,
                e.EventHeader.EventDescriptor.Task
            );
            *event_counts.entry(provider).or_insert(0) += 1;
        }
    });
    let mut event_counts: Vec<_> = event_counts.into_iter().collect();
    // event_counts.sort_by_key(|x| x.0.clone()); //alphabetical
    event_counts.sort_by_key(|x| Reverse(x.1));
    for (k, v) in event_counts {
        println!("{} {}", k, v);
    }
}
