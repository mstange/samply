#![allow(dead_code)]
#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

use windows::Win32::System::Diagnostics::Etw;

use ferrisetw::{EventRecord, SchemaLocator};

// generate schema property lookups based on the name and type of each field
macro_rules! create_etw_schema_type {
    ($name:ident { $($field:ident: $type:ty),*, }) => {
        #[derive(Default, Debug)]
        pub struct $name {
            $(
                pub $field: Option<$type>,
            )*
        }
        impl $name {
            pub fn from_record(record: &EventRecord, schema_locator: &SchemaLocator) -> Self {
                let schema = schema_locator.event_schema(record).unwrap();
                let parser = ferrisetw::parser::Parser::create(record, &schema);

                let mut result = Self::default();
                $(
                    result.$field = parser.try_parse(stringify!($field)).ok();
                )*
                result
            }
        }
    };
}

const Event_Start: u8 = 1;
const Event_Stop: u8 = 2;
// DC = Data Collection. These events are emitted (typically only DCStart) to indicate threads
// and processes that are already alive when the kernel collection starts.
const Event_DCStart: u8 = 3;
const Event_DCStop: u8 = 4;

const Event_Process_Terminate: u8 = 11;
const Event_Process_ContextSwitch: u8 = 36;
const Event_Process_Defunct: u8 = 39;
const Event_Process_WorkerThread: u8 = 57;
const Event_Process_ThreadSetName: u8 = 72;

const Event_Image_Load: u8 = 10;
const Event_Image_Unload: u8 = 2;

create_etw_schema_type!(ImageEvent {
    ImageBase: u64,
    ImageSize: u64,
    ProcessId: u32,
    FileName: String,
});

create_etw_schema_type!(ProcessEvent {
    ProcessId: u32,
    ParentId: u32,
    ImageFileName: String,
    CommandLine: String,
});

create_etw_schema_type!(ThreadEvent {
    ProcessId: u32,
    TThreadId: u32, // not a typo, at least not in this code!
    ParentProcessID: u32,
    ImageName: String,
});

create_etw_schema_type!(ThreadSetNameEvent {
    ProcessId: u32,
    ThreadId: u32,
    ThreadName: String,
});

const StackWalkGuid: ::windows::core::GUID =
    ::windows::core::GUID::from_u128(0xdef2fe46_7bd6_4b80_bd94_f57fe20d0ce3);

const Event_StackWalk_Stack: u8 = 32;
const Event_StackWalk_KeyCreate: u8 = 34;
const Event_StackWalk_KeyDelete: u8 = 35;
const Event_StackWalk_KeyRundown: u8 = 36;
const Event_StackWalk_StackKeyKernel: u8 = 37;
const Event_StackWalk_StackKeyUser: u8 = 38;

// 32=Event_StackWalk_Stack. This is a very silly event. There are other events for much bigger stacks, but I haven't profiled
// anything that generates them yet.
create_etw_schema_type!(StackWalkEvent_StackRaw {
    EventTimeStamp: u64,
    StackProcess: u32,
    StackThread: u32,
    Stack1: u64,
    Stack2: u64,
    Stack3: u64,
    Stack4: u64,
    Stack5: u64,
    Stack6: u64,
    Stack7: u64,
    Stack8: u64,
    Stack9: u64,
    Stack10: u64,
    Stack11: u64,
    Stack12: u64,
    Stack13: u64,
    Stack14: u64,
    Stack15: u64,
    Stack16: u64,
    Stack17: u64,
    Stack18: u64,
    Stack19: u64,
    Stack20: u64,
    Stack21: u64,
    Stack22: u64,
    Stack23: u64,
    Stack24: u64,
    Stack25: u64,
    Stack26: u64,
    Stack27: u64,
    Stack28: u64,
    Stack29: u64,
    Stack30: u64,
    Stack31: u64,
    Stack32: u64,
});

#[derive(Default, Debug)]
pub struct StackWalkEvent_Stack {
    pub EventTimeStamp: u64,
    pub StackProcess: u32,
    pub StackThread: u32,
    pub Stack: [u64; 32],
}

// I see opcode 15, service information (ServiceName, ProcessName, ProcessId)
const SystemConfigGuid: ::windows::core::GUID =
    ::windows::core::GUID::from_u128(0x01853a65_418f_4f36_aefc_dc0f1d2fd235);

// PerfInfoGuid, 51=SysClEnter
create_etw_schema_type!(PerfInfoEvent_SysCallEnter {
    SysCallAddress: u64, // I think u64? is u32 in mof
});

// PerfInfoGuid, 52=SysClExit
create_etw_schema_type!(PerfInfoEvent_SysCallExit {
    SysCallNtStatus: u32, // I think u64? is u32 in mof
});

#[derive(Debug)]
pub enum TracingEvent {
    ProcessStart(ProcessEvent),
    ProcessStop(ProcessEvent),
    ProcessDCStart(ProcessEvent),
    ThreadStart(ThreadEvent),
    ThreadStop(ThreadEvent),
    ThreadDCStart(ThreadEvent),
    ImageLoad(ImageEvent),
    ImageUnload(ImageEvent),
    ImageDCLoad(ImageEvent),
    StackWalk(StackWalkEvent_Stack),
}

const ObjectManagerGuid: ::windows::core::GUID =
    ::windows::core::GUID::from_u128(0x89497f50_effe_4440_8cf2_ce6b1cdcaca7);
// 32=CreateHandle, 33=CloseHandle, 34=DuplicateHandle, 36=TypeDCStart, 37=TypeDCEnd, 38=HandleDCStart, 39=HandleDCEnd,
// 48=CreateObject, 49=DeleteObject
// the .mof file has 32-bit handles/objects but that's not correct, not sure what the right is

pub fn get_tracing_event(
    ev: &EventRecord,
    schema_locator: &SchemaLocator,
) -> Option<(u64, TracingEvent)> {
    let provider = ::windows::core::GUID::from(ev.provider_id().to_u128());
    let opcode = ev.opcode();
    let event = match provider {
        Etw::ProcessGuid => {
            match opcode {
                Event_Start => TracingEvent::ProcessStart(ProcessEvent::from_record(ev, schema_locator)),
                Event_Stop => TracingEvent::ProcessStop(ProcessEvent::from_record(ev, schema_locator)),
                Event_DCStart => TracingEvent::ProcessStart(ProcessEvent::from_record(ev, schema_locator)),
                _ => return None,
            }
        },
        Etw::ThreadGuid => {
            match opcode {
                Event_Start => TracingEvent::ThreadStart(ThreadEvent::from_record(ev, schema_locator)),
                Event_Stop => TracingEvent::ThreadStop(ThreadEvent::from_record(ev, schema_locator)),
                Event_DCStart => TracingEvent::ThreadDCStart(ThreadEvent::from_record(ev, schema_locator)),
                _ => return None,
            }
        },
        Etw::ImageLoadGuid => {
            match opcode {
                Event_Image_Load => TracingEvent::ImageLoad(ImageEvent::from_record(ev, schema_locator)),
                Event_Image_Unload => TracingEvent::ImageUnload(ImageEvent::from_record(ev, schema_locator)),
                Event_DCStart=> TracingEvent::ImageDCLoad(ImageEvent::from_record(ev, schema_locator)),
                _ => return None,
            }
        },
        StackWalkGuid => {
            match opcode {
                Event_StackWalk_Stack => {
                    let raw = StackWalkEvent_StackRaw::from_record(ev, schema_locator);
                    let stack = StackWalkEvent_Stack {
                        EventTimeStamp: raw.EventTimeStamp.unwrap(),
                        StackProcess: raw.StackProcess.unwrap(),
                        StackThread: raw.StackThread.unwrap(),
                        Stack: [
                            raw.Stack1.unwrap_or_default(), raw.Stack2.unwrap_or_default(), raw.Stack3.unwrap_or_default(), raw.Stack4.unwrap_or_default(), raw.Stack5.unwrap_or_default(), raw.Stack6.unwrap_or_default(), raw.Stack7.unwrap_or_default(), raw.Stack8.unwrap_or_default(),
                            raw.Stack9.unwrap_or_default(), raw.Stack10.unwrap_or_default(), raw.Stack11.unwrap_or_default(), raw.Stack12.unwrap_or_default(), raw.Stack13.unwrap_or_default(), raw.Stack14.unwrap_or_default(), raw.Stack15.unwrap_or_default(), raw.Stack16.unwrap_or_default(),
                            raw.Stack17.unwrap_or_default(), raw.Stack18.unwrap_or_default(), raw.Stack19.unwrap_or_default(), raw.Stack20.unwrap_or_default(), raw.Stack21.unwrap_or_default(), raw.Stack22.unwrap_or_default(), raw.Stack23.unwrap_or_default(), raw.Stack24.unwrap_or_default(),
                            raw.Stack25.unwrap_or_default(), raw.Stack26.unwrap_or_default(), raw.Stack27.unwrap_or_default(), raw.Stack28.unwrap_or_default(), raw.Stack29.unwrap_or_default(), raw.Stack30.unwrap_or_default(), raw.Stack31.unwrap_or_default(), raw.Stack32.unwrap_or_default(),
                        ],
                    };
                    TracingEvent::StackWalk(stack)
                },
                _ => {
                    eprintln!("Unhandled stackwalk opcode: {}", opcode);
                    return None;
                }
            }
        },
        Etw::PerfInfoGuid |
        SystemConfigGuid |
        Etw::EventTraceGuid | // we could pull out start/end time from EventTraceGuid
        Etw::PageFaultGuid | // we could pull out VM alloc/frees here
        ObjectManagerGuid
        => {
            return None;
        },
        _ => {
            eprintln!("Unknown provider: {:?} opcode: {}", provider, opcode);
            return None;
        }
    };

    Some((ev.raw_timestamp() as u64, event))
}
