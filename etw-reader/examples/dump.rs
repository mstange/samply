use etw_reader::{open_trace, parser::{Parser}, print_property, schema::SchemaLocator};
use windows::Win32::System::Diagnostics::Etw;
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
        println!("{:?} {} {} {}-{} {} {}", e.EventHeader.ProviderId, s.name(), s.provider_name(), e.EventHeader.EventDescriptor.Opcode, e.EventHeader.EventDescriptor.Id, s.property_count(), e.EventHeader.TimeStamp);
        println!("pid: {}", s.process_id());
        if e.ExtendedDataCount > 0 {
            let items = unsafe { std::slice::from_raw_parts(e.ExtendedData, e.ExtendedDataCount as usize) };
            for i in items {
                match i.ExtType as u32 {
                    Etw::EVENT_HEADER_EXT_TYPE_EVENT_SCHEMA_TL => {
                        println!("extended: SCHEMA_TL");
                    }
                    Etw::EVENT_HEADER_EXT_TYPE_PROV_TRAITS => {
                        println!("extended: PROV_TRAITS");
                    }
                    _ => {
                        println!("extended: {:?}", i);
                    }

                }
            }
        }
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

