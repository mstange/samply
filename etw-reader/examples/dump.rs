use etw_reader::{open_trace, parser::{Parser, TryParse}, print_property, schema::{EventSchema, SchemaLocator}, tdh_types::{Property, TdhInType}};
use windows::{Guid, IntoParam, Param};
use std::path::Path;



fn main() {

    let mut schema_locator = SchemaLocator::new();
    etw_reader::add_custom_schemas(&mut schema_locator);
    let pattern = std::env::args().nth(2);
    open_trace(Path::new(&std::env::args().nth(1).unwrap()), 
|e| { 
    //dbg!(e.EventHeader.TimeStamp);


    let s = schema_locator.event_schema(e);
    if let Ok(s) = s {
        if let Some(pattern) = &pattern {
            if !s.name().contains(pattern) {
                return;
            }
        }
        println!("{:?} {} {}-{} {} {}", e.EventHeader.ProviderId, s.name(),  e.EventHeader.EventDescriptor.Opcode, e.EventHeader.EventDescriptor.Id, s.property_count(), e.EventHeader.TimeStamp);

        let mut parser = Parser::create(&s);
        for i in 0..s.property_count() {
            let property = s.property(i);
            //dbg!(&property);
            print_property(&mut parser, &property);
        }
    } else {
        if pattern.is_none() {
            println!("unknown event {:x?}:{}", e.EventHeader.ProviderId, e.EventHeader.EventDescriptor.Opcode);
        }
    }


});

}

