use std::convert::TryInto;
use std::path::Path;
use std::time::Instant;

use debugid::DebugId;
use etw_reader::parser::{Address, Parser, TryParse};
use etw_reader::schema::SchemaLocator;
use etw_reader::{
    add_custom_schemas, event_properties_to_string, open_trace, print_property, GUID,
};
use fxprof_processed_profile::debugid;
use uuid::Uuid;

use super::profile_context::ProfileContext;
use crate::windows::coreclr;
use crate::windows::profile_context::KnownCategory;

pub fn profile_pid_from_etl_file(context: &mut ProfileContext, etl_file: &Path) {
    let is_arm64 = context.is_arm64();

    let mut schema_locator = SchemaLocator::new();
    add_custom_schemas(&mut schema_locator);

    let processing_start_timestamp = Instant::now();

    let demand_zero_faults = false; //pargs.contains("--demand-zero-faults");

    let mut core_clr_context = coreclr::CoreClrContext::new(context.creation_props());

    let result = open_trace(etl_file, |e| {
        let Ok(s) = schema_locator.event_schema(e) else {
            return;
        };

        let mut parser = Parser::create(&s);
        let timestamp_raw = e.EventHeader.TimeStamp as u64;

        //eprintln!("{}", s.name());
        match s.name() {
            "MSNT_SystemTrace/EventTrace/Header" => {
                let _timer_resolution: u32 = parser.parse("TimerResolution");
                let perf_freq: u64 = parser.parse("PerfFreq");
                let clock_type: u32 = parser.parse("ReservedFlags");
                let events_lost: u32 = parser.parse("EventsLost");
                if events_lost != 0 {
                    log::warn!("{} events lost", events_lost);
                }

                context.handle_header(timestamp_raw, perf_freq, clock_type);

                if log::log_enabled!(log::Level::Info) {
                    for i in 0..s.property_count() {
                        let property = s.property(i);
                        print_property(&mut parser, &property, false);
                    }
                }
            }
            "MSNT_SystemTrace/PerfInfo/CollectionStart" => {
                let interval_raw: u32 = parser.parse("NewInterval");
                context.handle_collection_start(interval_raw);
            }
            "MSNT_SystemTrace/Thread/SetName" => {
                let pid: u32 = parser.parse("ProcessId");
                let tid: u32 = parser.parse("ThreadId");
                let thread_name: String = parser.parse("ThreadName");
                context.handle_thread_set_name(timestamp_raw, pid, tid, thread_name);
            }
            "MSNT_SystemTrace/Thread/DCStart" => {
                let tid: u32 = parser.parse("TThreadId");
                let pid: u32 = parser.parse("ProcessId");
                let thread_name: Option<String> = parser.try_parse("ThreadName").ok();
                context.handle_thread_dcstart(timestamp_raw, tid, pid, thread_name)
            }
            "MSNT_SystemTrace/Thread/Start" => {
                let tid: u32 = parser.parse("TThreadId");
                let pid: u32 = parser.parse("ProcessId");
                let thread_name: Option<String> = parser.try_parse("ThreadName").ok();
                context.handle_thread_start(timestamp_raw, tid, pid, thread_name);
            }
            "MSNT_SystemTrace/Thread/End" => {
                let tid: u32 = parser.parse("TThreadId");
                let pid: u32 = parser.parse("ProcessId");
                context.handle_thread_end(timestamp_raw, pid, tid);
            }
            "MSNT_SystemTrace/Thread/DCEnd" => {
                let tid: u32 = parser.parse("TThreadId");
                context.handle_thread_dcend(timestamp_raw, tid);
            }
            "MSNT_SystemTrace/Process/DCStart" => {
                // note: the event's e.EventHeader.process_id here is the parent (i.e. the process that spawned
                // a new one. The process_id in ProcessId is the new process id.
                // XXXmstange then what about "ParentId"? Is it the same as e.EventHeader.process_id?
                let pid: u32 = parser.parse("ProcessId");
                let parent_pid: u32 = parser.parse("ParentId");
                let image_file_name: String = parser.parse("ImageFileName");
                context.handle_process_dcstart(timestamp_raw, pid, parent_pid, image_file_name);
            }
            "MSNT_SystemTrace/Process/Start" => {
                // note: the event's e.EventHeader.process_id here is the parent (i.e. the process that spawned
                // a new one. The process_id in ProcessId is the new process id.
                // XXXmstange then what about "ParentId"? Is it the same as e.EventHeader.process_id?
                let pid: u32 = parser.parse("ProcessId");
                let parent_pid: u32 = parser.parse("ParentId");
                let image_file_name: String = parser.parse("ImageFileName");
                context.handle_process_start(timestamp_raw, pid, parent_pid, image_file_name);
            }
            "MSNT_SystemTrace/Process/End" => {
                let pid: u32 = parser.parse("ProcessId");
                context.handle_process_end(timestamp_raw, pid);
            }
            "MSNT_SystemTrace/Process/DCEnd" => {
                let pid: u32 = parser.parse("ProcessId");
                context.handle_process_dcend(timestamp_raw, pid);
            }
            "MSNT_SystemTrace/Process/Terminate" => {
                // nothing, but we don't want a marker for it
            }
            "MSNT_SystemTrace/StackWalk/Stack" => {
                let tid: u32 = parser.parse("StackThread");
                let pid: u32 = parser.parse("StackProcess");
                // The EventTimeStamp here indicates thea ccurate time the stack was collected, and
                // not the time the ETW event was emitted (which is in the header). Use it instead.
                let referenced_timestamp_raw: u64 = parser.parse("EventTimeStamp");
                let stack_len = parser.buffer.len() / 8;
                let stack_address_iter = parser
                    .buffer
                    .chunks_exact(8)
                    .map(|a| u64::from_ne_bytes(a.try_into().unwrap()));
                if is_arm64 {
                    context.handle_stack_arm64(
                        referenced_timestamp_raw,
                        pid,
                        tid,
                        stack_address_iter,
                    );
                } else {
                    context.handle_stack_x86(
                        referenced_timestamp_raw,
                        pid,
                        tid,
                        stack_len,
                        stack_address_iter,
                    );
                }
            }
            "MSNT_SystemTrace/PerfInfo/SampleProf" => {
                let tid: u32 = parser.parse("ThreadId");
                context.handle_sample(timestamp_raw, tid);
            }
            "MSNT_SystemTrace/PageFault/DemandZeroFault" => {
                if !demand_zero_faults {
                    return;
                }

                let tid: u32 = s.thread_id();
                context.handle_sample(timestamp_raw, tid);
            }
            "MSNT_SystemTrace/PageFault/VirtualAlloc"
            | "MSNT_SystemTrace/PageFault/VirtualFree" => {
                let is_free = s.name() == "MSNT_SystemTrace/PageFault/VirtualFree";
                let pid = e.EventHeader.ProcessId;
                let tid = e.EventHeader.ThreadId;
                let region_size: u64 = parser.parse("RegionSize");
                let text = event_properties_to_string(&s, &mut parser, None);
                context.handle_virtual_alloc_free(
                    timestamp_raw,
                    is_free,
                    pid,
                    tid,
                    region_size,
                    text,
                );
            }
            "KernelTraceControl/ImageID/" => {
                // KernelTraceControl/ImageID/ and KernelTraceControl/ImageID/DbgID_RSDS are synthesized during merge from
                // from MSNT_SystemTrace/Image/Load but don't contain the full path of the binary, or the full debug info in one go.
                // We go through "ImageID/" to capture pid/address + the original filename, then expect to see a "DbgID_RSDS" event
                // with pdb and debug info. We link those up through the "libs" hash, and then finally add them to the process
                // in Image/Load.

                let pid = s.process_id(); // there isn't a ProcessId field here
                let image_base: u64 = parser.try_parse("ImageBase").unwrap();
                let image_timestamp: u32 = parser.try_parse("TimeDateStamp").unwrap();
                let image_size: u32 = parser.try_parse("ImageSize").unwrap();
                let image_path: String = parser.try_parse("OriginalFileName").unwrap();
                context.handle_image_id(pid, image_base, image_timestamp, image_size, image_path);
            }
            "KernelTraceControl/ImageID/DbgID_RSDS" => {
                let pid = parser.try_parse("ProcessId").unwrap();
                let image_base: u64 = parser.try_parse("ImageBase").unwrap();
                let guid: GUID = parser.try_parse("GuidSig").unwrap();
                let age: u32 = parser.try_parse("Age").unwrap();
                let debug_id = DebugId::from_parts(
                    Uuid::from_fields(guid.data1, guid.data2, guid.data3, &guid.data4),
                    age,
                );
                let pdb_path: String = parser.try_parse("PdbFileName").unwrap();
                context.handle_image_debug_id(pid, image_base, debug_id, pdb_path);
            }
            "MSNT_SystemTrace/Image/Load" | "MSNT_SystemTrace/Image/DCStart" => {
                // KernelTraceControl/ImageID/ and KernelTraceControl/ImageID/DbgID_RSDS are synthesized from MSNT_SystemTrace/Image/Load
                // but don't contain the full path of the binary. We go through a bit of a dance to store the information from those events
                // in pending_libraries and deal with it here. We assume that the KernelTraceControl events come before the Image/Load event.

                // the ProcessId field doesn't necessarily match s.process_id();
                let pid = parser.try_parse("ProcessId").unwrap();
                let image_base: u64 = parser.try_parse("ImageBase").unwrap();
                let image_size: u64 = parser.try_parse("ImageSize").unwrap();
                let path: String = parser.try_parse("FileName").unwrap();
                context.handle_image_load(timestamp_raw, pid, image_base, image_size, path);
            }
            "MSNT_SystemTrace/Image/UnLoad" => {
                // nothing, but we don't want a marker for it
            }
            "Microsoft-Windows-DxgKrnl/VSyncDPC/Info " => {
                context.handle_vsync(timestamp_raw);
            }
            "MSNT_SystemTrace/Thread/CSwitch" => {
                let old_tid: u32 = parser.parse("OldThreadId");
                let new_tid: u32 = parser.parse("NewThreadId");
                context.handle_cswitch(timestamp_raw, old_tid, new_tid);
            }
            "MSNT_SystemTrace/Thread/ReadyThread" => {
                // these events can give us the unblocking stack
                let _thread_id: u32 = parser.parse("TThreadId");
            }
            "V8.js/MethodLoad/Start"
            | "Microsoft-JScript/MethodRuntime/MethodDCStart"
            | "Microsoft-JScript/MethodRuntime/MethodLoad" => {
                let pid = s.process_id();
                let method_name: String = parser.parse("MethodName");
                let method_start_address: Address = parser.parse("MethodStartAddress");
                let method_size: u64 = parser.parse("MethodSize");
                // let source_id: u64 = parser.parse("SourceID");
                context.handle_js_method_load(
                    timestamp_raw,
                    pid,
                    method_name,
                    method_start_address.as_u64(),
                    method_size,
                );
            }
            /*"V8.js/SourceLoad/" |
            "Microsoft-JScript/MethodRuntime/MethodDCStart" |
            "Microsoft-JScript/MethodRuntime/MethodLoad" => {
                let source_id: u64 = parser.parse("SourceID");
                let url: String = parser.parse("Url");
                jscript_sources.insert(source_id, url);
                //dbg!(s.process_id(), jscript_symbols.keys());
            }*/
            "Microsoft-Windows-Direct3D11/ID3D11VideoContext_SubmitDecoderBuffers/win:Start" => {
                let tid = s.thread_id();
                let text = event_properties_to_string(&s, &mut parser, None);
                context.handle_freeform_marker_start(
                    timestamp_raw,
                    tid,
                    s.name().strip_suffix("/win:Start").unwrap(),
                    text,
                );
            }
            "Microsoft-Windows-Direct3D11/ID3D11VideoContext_SubmitDecoderBuffers/win:Stop" => {
                let tid = s.thread_id();
                let text = event_properties_to_string(&s, &mut parser, None);
                context.handle_freeform_marker_end(
                    timestamp_raw,
                    tid,
                    s.name().strip_suffix("/win:Start").unwrap(),
                    text,
                    KnownCategory::D3DVideoSubmitDecoderBuffers,
                );
            }
            marker_name if marker_name.starts_with("Mozilla.FirefoxTraceLogger/") => {
                let Some(marker_name) = marker_name
                    .strip_prefix("Mozilla.FirefoxTraceLogger/")
                    .and_then(|s| s.strip_suffix("/Info"))
                else {
                    return;
                };
                let tid = e.EventHeader.ThreadId;
                // We ignore e.EventHeader.TimeStamp and instead take the timestamp from the fields.
                let start_time_qpc: u64 = parser.try_parse("StartTime").unwrap();
                let end_time_qpc: u64 = parser.try_parse("EndTime").unwrap();
                let phase: Option<u8> = parser.try_parse("Phase").ok();
                let maybe_user_timing_name: Option<String> = parser.try_parse("name").ok();
                let maybe_explicit_marker_name: Option<String> =
                    parser.try_parse("MarkerName").ok();
                let text = event_properties_to_string(
                    &s,
                    &mut parser,
                    Some(&[
                        "MarkerName",
                        "StartTime",
                        "EndTime",
                        "Phase",
                        "InnerWindowId",
                        "CategoryPair",
                    ]),
                );
                context.handle_firefox_marker(
                    tid,
                    marker_name,
                    start_time_qpc,
                    end_time_qpc,
                    phase,
                    maybe_user_timing_name,
                    maybe_explicit_marker_name,
                    text,
                );
            }
            "MetaData/EventInfo" | "Process/Terminate" => {
                // ignore
            }
            marker_name if marker_name.starts_with("Google.Chrome/") => {
                let Some(marker_name) = marker_name
                    .strip_prefix("Google.Chrome/")
                    .and_then(|s| s.strip_suffix("/Info"))
                else {
                    return;
                };
                let tid = e.EventHeader.ThreadId;
                // We ignore e.EventHeader.TimeStamp and instead take the timestamp from the fields.
                let timestamp_raw: u64 = parser.try_parse("Timestamp").unwrap();
                let phase: String = parser.try_parse("Phase").unwrap();
                let keyword_bitfield = e.EventHeader.EventDescriptor.Keyword; // a bitfield of keywords
                let text = event_properties_to_string(
                    &s,
                    &mut parser,
                    Some(&["Timestamp", "Phase", "Duration"]),
                );
                context.handle_chrome_marker(
                    tid,
                    marker_name,
                    timestamp_raw,
                    &phase,
                    keyword_bitfield,
                    text,
                );
            }
            dotnet_event if dotnet_event.starts_with("Microsoft-Windows-DotNETRuntime") => {
                // Note: No "/" at end of event name, because we want DotNETRuntimeRundown as well
                coreclr::handle_coreclr_event(context, &mut core_clr_context, &s, &mut parser);
            }
            _ => {
                let tid = e.EventHeader.ThreadId;
                if context.has_thread(tid) {
                    let task_and_op = s.name().split_once('/').unwrap().1;
                    let text = event_properties_to_string(&s, &mut parser, None);
                    context.handle_unknown_event(timestamp_raw, tid, task_and_op, text);
                }
            }
        }
    });

    if result.is_err() {
        dbg!(&result);
        std::process::exit(1);
    }

    log::info!(
        "Took {} seconds",
        (Instant::now() - processing_start_timestamp).as_secs_f32()
    );
}
