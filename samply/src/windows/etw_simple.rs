#![allow(dead_code)]
#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

use windows::Win32::System::Diagnostics::Etw;

use ferrisetw::{EventRecord, SchemaLocator};
use fxprof_processed_profile::{CpuDelta, Frame, FrameFlags, FrameInfo, Timestamp};
use crate::windows::ProfileContext;

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
    ThreadName: String,
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
                    let schema = schema_locator.event_schema(ev).unwrap();
                    let parser = ferrisetw::parser::Parser::create(ev, &schema);

                    assert_eq!(self.0.UserDataLength % 8, 0);
                    let user_buf = std::slice::from_raw_parts(self.0.UserData as *mut u64, self.0.UserDataLength / 8);
                    //eprintln!("Stackwalk parser buffer size: {}", ev.0.UserDataLength);
                    let raw = StackWalkEvent_StackRaw::from_record(ev, schema_locator);
                    let stack = StackWalkEvent_Stack {
                        EventTimeStamp: raw.EventTimeStamp.unwrap(),
                        StackProcess: raw.StackProcess.unwrap(),
                        StackThread: raw.StackThread.unwrap(),
                        Stack: user_buf.clone(),
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

pub fn trace_callback(ev: &EventRecord, sl: &SchemaLocator, context: &mut ProfileContext) {
    // For the first event we see, use its time as the reference. Maybe there's something
    // on the trace itself we can use.
    // TODO see comment earlier about using QueryTraceProcessingHandle
    if context.timebase_nanos == 0 {
        context.timebase_nanos = ev.raw_timestamp() as u64;
    }

    let Some((ts, event)) = get_tracing_event(ev, sl) else {
        return;
    };

    let timestamp = Timestamp::from_nanos_since_reference(ts - context.timebase_nanos);
    //eprintln!("{} {:?}", ts, event);
    match event {
        TracingEvent::ProcessDCStart(e) | TracingEvent::ProcessStart(e) => {
            let exe = e.ImageFileName.unwrap();
            let pid = e.ProcessId.unwrap();
            let ppid = e.ParentId.unwrap_or(0);

            if context.is_interesting_process(pid, Some(ppid), None) {
                context.add_process(pid, ppid, &exe, timestamp);
            }
        }
        TracingEvent::ProcessStop(e) => {
            let pid = e.ProcessId.unwrap();
            context.remove_process(pid, Some(timestamp));
        }
        TracingEvent::ThreadDCStart(e) | TracingEvent::ThreadStart(e) => {
            let pid = e.ProcessId.unwrap();
            let tid = e.TThreadId.unwrap();

            if context.is_interesting_process(pid, None, None) {
                context.add_thread(pid, tid, timestamp);
                if let Some(thread_name) = e.ThreadName {
                    context.set_thread_name(tid, &thread_name);
                }
            }
        }
        TracingEvent::ThreadStop(e) => {
            let tid = e.TThreadId.unwrap();

            context.remove_thread(tid, Some(timestamp));
        }
        TracingEvent::ImageDCLoad(e) | TracingEvent::ImageLoad(e) => {
            let pid = e.ProcessId.unwrap();
            let base = e.ImageBase.unwrap();
            let size = e.ImageSize.unwrap();
            let filename = e.FileName.unwrap();

            if let Some(process_handle) = context.get_process_handle(pid) {
                let lib_handle = context.get_or_add_lib_simple(&filename);

                context.with_profile(|profile| {
                    profile.add_lib_mapping(process_handle, lib_handle, base, base + size, 0);
                });
            }
        }
        TracingEvent::ImageUnload(e) => {
            let pid = e.ProcessId.unwrap();
            let base = e.ImageBase.unwrap();

            if let Some(process) = context.get_process(pid) {
                context.with_profile(|profile| {
                    profile.remove_lib_mapping(process.handle, base);
                });
            }
        }
        TracingEvent::StackWalk(e) => {
            let _pid = e.StackProcess;
            let tid = e.StackThread;

            if let Some(thread) = context.get_thread(tid) {
                let frames: Vec<FrameInfo> = e
                    .Stack
                    .iter()
                    .take_while(|&&frame| frame != 0)
                    .enumerate()
                    .map(|(i, &frame)| FrameInfo {
                        frame: if i == 0 { Frame::InstructionPointer(frame) } else { Frame::ReturnAddress(frame) },
                        flags: FrameFlags::empty(),
                        category_pair: if frame >= context.kernel_min {
                            context.kernel_category
                        } else {
                            context.default_category
                        },
                    })
                    .collect();

                context.with_profile(|profile| {
                    profile.add_sample(thread.handle, timestamp, frames.into_iter().rev(), CpuDelta::ZERO, 1);
                });
            }
        }
    }

    // Haven't seen extended data in anything yet. Not used by kernel logger I don't think.
    //for edata in ev.extended_data().iter() {
    //    eprintln!("extended data: {:?} {:?}", edata.data_type(), edata.to_extended_data_item());
    //}
}
