use etw_reader::{start_trace, parser::{Parser}, print_property, schema::SchemaLocator};


fn main() {

    let mut schema_locator = SchemaLocator::new();
    etw_reader::add_custom_schemas(&mut schema_locator);
    let pattern = std::env::args().nth(2);
    start_trace( 
|e| { 


    let s = schema_locator.event_schema(e);
    if let Ok(s) = s {
            if !s.name().contains("VideoProcessorBltParameters") {
                return;
            }
        println!("pid {} time {}", e.EventHeader.ProcessId, e.EventHeader.TimeStamp);
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