use etw_reader::{GUID, open_trace, schema::{SchemaLocator}};
use std::{cmp::Reverse, collections::HashMap, path::Path};

fn main() {
    let mut schema_locator = SchemaLocator::new();
    etw_reader::add_custom_schemas(&mut schema_locator);
    let mut event_counts = HashMap::new();
    open_trace(Path::new(&std::env::args().nth(1).unwrap()), |e| {
        let s = schema_locator.event_schema(e);
        if let Ok(s) = s {
            if let Some(count) = event_counts.get_mut(s.name()) {
                *count += 1;
            } else {
                event_counts.insert(s.name().to_owned(), 1);
            }
        } else {
            let provider_name = 
            match e.EventHeader.ProviderId {
                GUID{ data1: 0x9B79EE91, data2: 0xB5FD, data3: 0x41C0, data4: [0xA2, 0x43, 0x42, 0x48, 0xE2, 0x66, 0xE9, 0xD0]} => "SysConfig ",
                GUID{ data1: 0xB3E675D7, data2: 0x2554, data3: 0x4F18, data4: [0x83, 0x0B, 0x27, 0x62, 0x73, 0x25, 0x60, 0xDE]} => "KernelTraceControl ",
                GUID{ data1: 0xED54DFF8, data2: 0xC409, data3: 0x4CF6, data4: [0xBF, 0x83, 0x05, 0xE1, 0xE6, 0x1A, 0x09, 0xC4]} => "WinSat ",

                _ => ""
            };

            let provider = format!("{}{:?}/{}", provider_name,  e.EventHeader.ProviderId, e.EventHeader.EventDescriptor.Opcode);
            *event_counts.entry(provider).or_insert(0) += 1;
        }
    });
    let mut event_counts: Vec<_> = event_counts.into_iter().collect();
    event_counts.sort_by_key(|x| Reverse(x.1));
    for (k, v) in event_counts {
        println!("{} {}", k, v);
    }
}
