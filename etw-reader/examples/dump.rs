use etw_reader::{open_trace, parser::{Parser, TryParse}, schema::{EventSchema, SchemaLocator}, tdh_types::{Property, TdhInType}};
use windows::{Guid, IntoParam, Param};
use std::path::Path;


fn print_property(parser: &mut Parser, property: &Property) {
    print!("  {} = ", property.name);
    match property.in_type() {
        TdhInType::InTypeUnicodeString => println!("{:?}", TryParse::<String>::try_parse(parser, &property.name)),
        TdhInType::InTypeAnsiString => println!("{:?}", TryParse::<String>::try_parse(parser, &property.name)),
        TdhInType::InTypeUInt32 => println!("{:?}", TryParse::<u32>::try_parse(parser, &property.name)),
        TdhInType::InTypeUInt8 => println!("{:?}", TryParse::<u8>::try_parse(parser, &property.name)),
        TdhInType::InTypePointer => println!("{:?}", TryParse::<u64>::try_parse(parser, &property.name)),
        TdhInType::InTypeInt64 => println!("{:?}", TryParse::<i64>::try_parse(parser, &property.name)),
        TdhInType::InTypeUInt64 => println!("{:?}", TryParse::<u64>::try_parse(parser, &property.name)),
        TdhInType::InTypeGuid => println!("{:?}", TryParse::<Guid>::try_parse(parser, &property.name)),
        _ => println!("Unknown {:?}", property.in_type())
    }
}
fn main() {

    let mut schema_locator = SchemaLocator::new();
    etw_reader::add_custom_schemas(&mut schema_locator);
    open_trace(Path::new(&std::env::args().nth(1).unwrap()), 
|e| { 
    //dbg!(e.EventHeader.TimeStamp);


    let s = schema_locator.event_schema(e);
    if let Ok(s) = s {

        println!("{:?} {} {}-{} {} {}", e.EventHeader.ProviderId, s.name(),  e.EventHeader.EventDescriptor.Opcode, e.EventHeader.EventDescriptor.Id, s.property_count(), e.EventHeader.TimeStamp);

        let mut parser = Parser::create(&s);
        for i in 0..s.property_count() {
            let property = s.property(i);
            //dbg!(&property);
            print_property(&mut parser, &property);
        }
    } else {
        println!("unknown event {:x?}", e.EventHeader.ProviderId);

    }


});

}

