use etw_reader::{
    open_trace,
    parser::{Parser, TryParse},
    print_property,
    schema::{EventSchema, SchemaLocator},
    tdh_types::{Property, TdhInType},
};
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
            let provider = format!("{:?}/{}", e.EventHeader.ProviderId, e.EventHeader.EventDescriptor.Opcode);
            *event_counts.entry(provider).or_insert(0) += 1;
        }
    });
    let mut event_counts: Vec<_> = event_counts.into_iter().collect();
    event_counts.sort_by_key(|x| Reverse(x.1));
    for (k, v) in event_counts {
        println!("{} {}", k, v);
    }
}
