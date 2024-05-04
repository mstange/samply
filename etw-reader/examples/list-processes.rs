use std::path::Path;

use etw_reader::open_trace;
use etw_reader::parser::{Parser, TryParse};
use etw_reader::schema::SchemaLocator;

fn main() {
    let mut schema_locator = SchemaLocator::new();
    etw_reader::add_custom_schemas(&mut schema_locator);
    open_trace(Path::new(&std::env::args().nth(1).unwrap()), |e| {
        let s = schema_locator.event_schema(e);
        if let Ok(s) = s {
            // DCEnd is used '-Buffering' traces
            if let "MSNT_SystemTrace/Process/Start"
            | "MSNT_SystemTrace/Process/DCStart"
            | "MSNT_SystemTrace/Process/DCEnd" = s.name()
            {
                let mut parser = Parser::create(&s);

                let image_file_name: String = parser.parse("ImageFileName");
                let process_id: u32 = parser.parse("ProcessId");
                let command_line: String = parser.parse("CommandLine");

                println!("{} {} {}", image_file_name, process_id, command_line);
            }
        }
    })
    .unwrap();
}
