use etw_log::{open_trace, parser::{Parser, TryParse}, schema::{EventSchema, SchemaLocator}, tdh_types::{Property, TdhInType}};
use windows::{Guid, IntoParam, Param};
use etw_log::tdh;
use std::{path::Path, sync::Arc};


fn print_property(parser: &mut Parser, property: &Property) {
    print!("{} = ", property.name);
    match property.in_type() {
        TdhInType::InTypeUnicodeString => println!("{:?}", TryParse::<String>::try_parse(parser, &property.name)),
        TdhInType::InTypeAnsiString => println!("{:?}", TryParse::<String>::try_parse(parser, &property.name)),
        TdhInType::InTypeUInt32 => println!("{:?}", TryParse::<u32>::try_parse(parser, &property.name)),
        TdhInType::InTypeUInt8 => println!("{:?}", TryParse::<u8>::try_parse(parser, &property.name)),
        TdhInType::InTypePointer => println!("{:?}", TryParse::<usize>::try_parse(parser, &property.name)),
        TdhInType::InTypeInt64 => println!("{:?}", TryParse::<i64>::try_parse(parser, &property.name)),
        TdhInType::InTypeGuid => println!("{:?}", TryParse::<Guid>::try_parse(parser, &property.name)),
        _ => println!("Unknown {:?}", property.in_type())
    }
}
fn main() {

    let mut schema_locator = SchemaLocator::new();
    let mut log_file = open_trace(Path::new("D:\\Captures\\23-09-2021_17-21-32_thread-switch-bench.etl"), 
|e| { 
    //dbg!(e.EventHeader.TimeStamp);

    let s = etw_log::schema_from_custom(e.clone());
    if let Some(s) = s {
        println!("{}/{}/{}",  s.provider_name(), s.task_name(), s.opcode_name());

        let mut parser = Parser::create(&s);
        for i in 0..s.property_count() {
            let property = s.property(i);
            print_property(&mut parser, &property);
        }
    } else {
  
        let s = tdh::schema_from_tdh(e.clone());  
        if let Ok(s) = s {

            println!("{:?} {}/{}/{} {}-{} {} {}", e.EventHeader.ProviderId, s.provider_name(), s.task_name(), s.opcode_name(),  e.EventHeader.EventDescriptor.Opcode, e.EventHeader.EventDescriptor.Id, s.property_count(), e.UserDataLength);

            let s = schema_locator.event_schema(e.clone()).unwrap();

            let mut parser = Parser::create(&s);
            for i in 0..s.property_count() {
                let property = s.property(i);
                //dbg!(&property);
                print_property(&mut parser, &property);
            }
        } else {
            eprintln!("unknown event {:x?}", e.EventHeader.ProviderId);

        }
    }

});

}

