use std::collections::HashMap;
use std::convert::TryInto;
use std::path::{Path, PathBuf};
use std::time::Instant;

use debugid::DebugId;
use fxprof_processed_profile::debugid;
use uuid::Uuid;

use super::coreclr::CoreClrContext;
use super::etw_reader::parser::{Address, Parser, TryParse};
use super::etw_reader::schema::SchemaLocator;
use super::etw_reader::{
    add_custom_schemas, event_properties_to_string, open_trace, print_property, GUID,
};
use super::profile_context::ProfileContext;
use crate::windows::coreclr;
use crate::windows::profile_context::PeInfo;

pub fn process_etl_files(
    context: &mut ProfileContext,
    etl_file: &Path,
    extra_etl_filenames: &[PathBuf],
) {
    let mut schema_locator = SchemaLocator::new();
    add_custom_schemas(&mut schema_locator);

    let processing_start_timestamp = Instant::now();

    let mut core_clr_context = CoreClrContext::new(context.creation_props());

    let result = process_trace(
        etl_file,
        context,
        &mut schema_locator,
        &mut core_clr_context,
    );
    if result.is_err() {
        dbg!(etl_file);
        dbg!(&result);
        std::process::exit(1);
    }

    for extra_etl_file in extra_etl_filenames {
        let result = process_trace(
            extra_etl_file,
            context,
            &mut schema_locator,
            &mut core_clr_context,
        );
        if result.is_err() {
            dbg!(extra_etl_file);
            dbg!(&result);
            std::process::exit(1);
        }
    }

    log::info!(
        "Took {} seconds",
        (Instant::now() - processing_start_timestamp).as_secs_f32()
    );
}

struct PendingImageInfo {
    image_timestamp: u32,
    pdb_path_and_debug_id: Option<(String, DebugId)>,
}

fn process_trace(
    etl_file: &Path,
    context: &mut ProfileContext,
    schema_locator: &mut SchemaLocator,
    core_clr_context: &mut CoreClrContext,
) -> Result<(), std::io::Error> {
    let is_arm64 = context.is_arm64();
    let demand_zero_faults = false; //pargs.contains("--demand-zero-faults");
    let mut pending_image_info: Option<((u32, u64), PendingImageInfo)> = None;

    // Cache for Chrome measure names by (tid, traceId).
    let mut measure_name_cache: HashMap<(u32, u64), String> = HashMap::new();

    open_trace(etl_file, |e| {
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
                    log::warn!("{events_lost} events lost");
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
                let cmdline: String = parser.parse("CommandLine");
                context.handle_process_dcstart(
                    timestamp_raw,
                    pid,
                    parent_pid,
                    image_file_name,
                    cmdline,
                );
            }
            "MSNT_SystemTrace/Process/Start" => {
                // note: the event's e.EventHeader.process_id here is the parent (i.e. the process that spawned
                // a new one. The process_id in ProcessId is the new process id.
                // XXXmstange then what about "ParentId"? Is it the same as e.EventHeader.process_id?
                let pid: u32 = parser.parse("ProcessId");
                let parent_pid: u32 = parser.parse("ParentId");
                let image_file_name: String = parser.parse("ImageFileName");
                let cmdline: String = parser.parse("CommandLine");
                context.handle_process_start(
                    timestamp_raw,
                    pid,
                    parent_pid,
                    image_file_name,
                    cmdline,
                );
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
                if !context.is_in_time_range(timestamp_raw) {
                    return;
                }

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
                if !context.is_in_time_range(timestamp_raw) {
                    return;
                }
                let tid: u32 = parser.parse("ThreadId");
                let cpu = u32::from(unsafe { e.BufferContext.Anonymous.ProcessorIndex });
                context.handle_sample(timestamp_raw, tid, cpu);
            }
            "MSNT_SystemTrace/PageFault/DemandZeroFault" => {
                if !context.is_in_time_range(timestamp_raw) {
                    return;
                }
                if !demand_zero_faults {
                    return;
                }

                let tid: u32 = s.thread_id();
                let cpu = u32::from(unsafe { e.BufferContext.Anonymous.ProcessorIndex });
                context.handle_sample(timestamp_raw, tid, cpu);
            }
            "MSNT_SystemTrace/PageFault/VirtualAlloc"
            | "MSNT_SystemTrace/PageFault/VirtualFree" => {
                if !context.is_in_time_range(timestamp_raw) {
                    return;
                }
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
            // KernelTraceControl/ImageID/ and KernelTraceControl/ImageID/DbgID_RSDS are synthesized by xperf during
            // `xperf -stop -d` from MSNT_SystemTrace/Image/DCStart and MSNT_SystemTrace/Image/Load; they are inserted
            // right before the original events.
            //
            // These KernelTraceControl events are not available in unmerged ETL files.
            //
            // We can get the following information out of them:
            //  - KernelTraceControl/ImageID/ has the image_timestamp (needed for the CodeId)
            //  - KernelTraceControl/ImageID/DbgID_RSDS has the guid+age and the PDB path (needed for DebugId + debug_path).
            "KernelTraceControl/ImageID/" => {
                let pid = s.process_id(); // there isn't a ProcessId field here
                let image_base: u64 = parser.try_parse("ImageBase").unwrap();
                let image_timestamp: u32 = parser.try_parse("TimeDateStamp").unwrap();
                let info = PendingImageInfo {
                    image_timestamp,
                    pdb_path_and_debug_id: None,
                };
                pending_image_info = Some(((pid, image_base), info));
            }
            "KernelTraceControl/ImageID/DbgID_RSDS" => {
                if let Some((pid_and_base, info)) = pending_image_info.as_mut() {
                    let pid = parser.try_parse("ProcessId").unwrap();
                    let image_base: u64 = parser.try_parse("ImageBase").unwrap();
                    if pid_and_base == &(pid, image_base) {
                        let guid: GUID = parser.try_parse("GuidSig").unwrap();
                        let age: u32 = parser.try_parse("Age").unwrap();
                        let pdb_path: String = parser.try_parse("PdbFileName").unwrap();
                        let debug_id = DebugId::from_parts(
                            Uuid::from_fields(guid.data1, guid.data2, guid.data3, &guid.data4),
                            age,
                        );
                        info.pdb_path_and_debug_id = Some((pdb_path, debug_id));
                    }
                };
            }
            // These events are generated by the kernel logger.
            // They are available in unmerged ETL files.
            "MSNT_SystemTrace/Image/Load" | "MSNT_SystemTrace/Image/DCStart" => {
                // the ProcessId field doesn't necessarily match s.process_id();
                let pid = parser.try_parse("ProcessId").unwrap();
                let image_base: u64 = parser.try_parse("ImageBase").unwrap();
                let image_size: u64 = parser.try_parse("ImageSize").unwrap();
                let image_timestamp_maybe_zero: u32 = parser.try_parse("TimeDateStamp").unwrap(); // zero for MSNT_SystemTrace/Image/DCStart
                let image_checksum: u32 = parser.try_parse("ImageChecksum").unwrap();
                let path: String = parser.try_parse("FileName").unwrap();

                let mut info =
                    PeInfo::new_with_size_and_checksum(image_size as u32, image_checksum);

                // Supplement the information from this event with the information from
                // KernelTraceControl/ImageID events, if available. Those events come right
                // before this one (they were inserted by xperf during merging, if this is
                // a merged file).
                match pending_image_info.take() {
                    Some((pid_and_base, pending_info)) if pid_and_base == (pid, image_base) => {
                        info.image_timestamp = Some(pending_info.image_timestamp);
                        if let Some((pdb_path, debug_id)) = pending_info.pdb_path_and_debug_id {
                            info.pdb_path = Some(pdb_path);
                            info.debug_id = Some(debug_id);
                        }
                    }
                    _ => {
                        if image_timestamp_maybe_zero != 0 {
                            info.image_timestamp = Some(image_timestamp_maybe_zero);
                        }
                    }
                }

                context.handle_image_load(timestamp_raw, pid, image_base, path, info);
            }
            "MSNT_SystemTrace/Image/UnLoad" => {
                // nothing, but we don't want a marker for it
            }
            "Microsoft-Windows-DxgKrnl/VSyncDPC/Info " => {
                if !context.is_in_time_range(timestamp_raw) {
                    return;
                }
                context.handle_vsync(timestamp_raw);
            }
            "MSNT_SystemTrace/Thread/CSwitch" => {
                if !context.is_in_time_range(timestamp_raw) {
                    return;
                }
                let old_tid: u32 = parser.parse("OldThreadId");
                let new_tid: u32 = parser.parse("NewThreadId");
                let cpu = u32::from(unsafe { e.BufferContext.Anonymous.ProcessorIndex });
                let wait_reason: i8 = parser.parse("OldThreadWaitReason");
                context.handle_cswitch(timestamp_raw, old_tid, new_tid, cpu, wait_reason);
            }
            "MSNT_SystemTrace/Thread/ReadyThread" => {
                if !context.is_in_time_range(timestamp_raw) {
                    return;
                }
                // these events can give us the unblocking stack
                let _thread_id: u32 = parser.parse("TThreadId");
            }
            "V8.js/SourceLoad/Start"
            | "Microsoft-JScript/ScriptContextRuntime/SourceLoad"
            | "Microsoft-JScript/ScriptContextRundown/SourceDCStart" => {
                let pid = s.process_id();
                if !context.has_process_at_time(pid, timestamp_raw) {
                    return;
                }
                let source_id: u64 = parser.parse("SourceID");
                let url: String = parser.parse("Url");
                context.handle_js_source_load(timestamp_raw, pid, source_id, url);
            }
            "V8.js/MethodLoad/Start"
            | "Microsoft-JScript/MethodRuntime/MethodLoad"
            | "Microsoft-JScript/MethodRundown/MethodDCStart" => {
                let pid = s.process_id();
                if !context.has_process_at_time(pid, timestamp_raw) {
                    return;
                }
                let method_name: String = parser.parse("MethodName");
                let method_start_address: Address = parser.parse("MethodStartAddress");
                let method_size: u64 = parser.parse("MethodSize");
                let source_id: u64 = parser.parse("SourceID");
                let line: u32 = parser.parse("Line");
                let column: u32 = parser.parse("Column");
                context.handle_js_method_load(
                    timestamp_raw,
                    pid,
                    method_name,
                    method_start_address.as_u64(),
                    method_size as u32,
                    source_id,
                    line,
                    column,
                );
            }
            "Microsoft-Windows-Direct3D11/ID3D11VideoContext_SubmitDecoderBuffers/win:Start" => {
                if !context.is_in_time_range(timestamp_raw) {
                    return;
                }
                let tid = s.thread_id();
                if !context.has_thread_at_time(tid, timestamp_raw) {
                    return;
                }
                let text = event_properties_to_string(&s, &mut parser, None);
                context.handle_freeform_marker_start(
                    timestamp_raw,
                    tid,
                    s.name().strip_suffix("/win:Start").unwrap(),
                    text,
                );
            }
            "Microsoft-Windows-Direct3D11/ID3D11VideoContext_SubmitDecoderBuffers/win:Stop" => {
                if !context.is_in_time_range(timestamp_raw) {
                    return;
                }
                let tid = s.thread_id();
                if !context.has_thread_at_time(tid, timestamp_raw) {
                    return;
                }
                let text = event_properties_to_string(&s, &mut parser, None);
                context.handle_freeform_marker_end(
                    timestamp_raw,
                    tid,
                    s.name().strip_suffix("/win:Stop").unwrap(),
                    text,
                );
            }
            marker_name if marker_name.starts_with("Mozilla.FirefoxTraceLogger/") => {
                if !context.is_in_time_range(timestamp_raw) {
                    return;
                }
                let tid = e.EventHeader.ThreadId;
                if !context.has_thread_at_time(tid, timestamp_raw) {
                    return;
                }
                let Some(marker_name) = marker_name
                    .strip_prefix("Mozilla.FirefoxTraceLogger/")
                    .and_then(|s| s.strip_suffix("/Info"))
                else {
                    return;
                };
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
                        "Subcategory",
                    ]),
                );
                context.handle_firefox_marker(
                    tid,
                    marker_name,
                    timestamp_raw,
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
                if !context.is_in_time_range(timestamp_raw) {
                    return;
                }
                let tid = e.EventHeader.ThreadId;
                if !context.has_thread_at_time(tid, timestamp_raw) {
                    return;
                }
                let Some(marker_name) = marker_name
                    .strip_prefix("Google.Chrome/")
                    .and_then(|s| s.strip_suffix("/Info"))
                else {
                    return;
                };

                // We ignore e.EventHeader.TimeStamp and instead take the timestamp from the fields.
                // The timestamp can be u64 or i64, depending on which code emits the events.
                // Chrome's marker timestamps are in microseconds relative to the QPC origin.
                // They are not in QPC ticks!
                // u64: https://source.chromium.org/chromium/chromium/src/+/main:base/trace_event/etw_interceptor_win.cc;l=65-85;drc=47d1537de78d69eb441b4cad8c441f0291faca9a
                // i64: https://source.chromium.org/chromium/chromium/src/+/main:base/trace_event/trace_event_etw_export_win.cc;l=316-334;drc=8c29f4a8930c3ccccdf1b66c28fe484cee7c7362
                let timestamp_us_i64: Option<i64> = parser.try_parse("Timestamp").ok();
                let timestamp_us_u64: Option<u64> = parser.try_parse("Timestamp").ok();
                let timestamp_us: Option<u64> =
                    timestamp_us_u64.or_else(|| timestamp_us_i64.and_then(|t| t.try_into().ok()));
                let Some(timestamp_us) = timestamp_us else {
                    // Saw "SequenceManagerImpl::MoveReadyDelayedTasksToWorkQueues" with no timestamp at all
                    // on 2024-05-23, possibly from VS Code electron
                    log::warn!("No Timestamp field on Chrome {marker_name} event");
                    return;
                };
                let phase: String = parser.try_parse("Phase").unwrap();
                let keyword_bitfield = e.EventHeader.EventDescriptor.Keyword; // a bitfield of keywords

                let display_name = if marker_name == "performance.measure" {
                    // Extract user-provided name from User Timing API measure events.
                    // The name is in "measureName".
                    let user_timing_measure_name: Option<String> =
                        parser.try_parse("measureName").ok();
                    let trace_id: Option<u64> = parser.try_parse("Id").ok();

                    if let Some(trace_id) = trace_id {
                        // performance.measure() emits Begin/End pairs with a trace ID.
                        // The measure name is only present in the Begin event.
                        if phase == "Begin" {
                            if let Some(name) = user_timing_measure_name {
                                measure_name_cache.insert((tid, trace_id), name.clone());
                                name
                            } else {
                                marker_name.to_string()
                            }
                        } else if phase == "End" {
                            // Look up the name we cached from the Begin event
                            measure_name_cache
                                .remove(&(tid, trace_id))
                                .unwrap_or_else(|| marker_name.to_string())
                        } else {
                            marker_name.to_string()
                        }
                    } else {
                        user_timing_measure_name.unwrap_or_else(|| marker_name.to_string())
                    }
                } else if marker_name == "performance.mark" {
                    // Extract user-provided name from User Timing API mark events.
                    // The name is in the "markName" field.
                    let user_timing_mark_name: Option<String> = parser.try_parse("markName").ok();
                    user_timing_mark_name.unwrap_or_else(|| marker_name.to_string())
                } else {
                    // For all other Chrome events, use the marker name as-is.
                    marker_name.to_string()
                };

                let text = event_properties_to_string(
                    &s,
                    &mut parser,
                    Some(&[
                        "Timestamp",
                        "Phase",
                        "Duration",
                        "markName",
                        "measureName",
                        "Id",
                    ]),
                );
                context.handle_chrome_marker(
                    tid,
                    timestamp_raw,
                    &display_name,
                    timestamp_us,
                    &phase,
                    keyword_bitfield,
                    text,
                );
            }
            dotnet_event if dotnet_event.starts_with("Microsoft-Windows-DotNETRuntime") => {
                let pid = s.process_id();
                if !context.has_process_at_time(pid, timestamp_raw) {
                    return;
                }
                let is_in_range = context.is_in_time_range(timestamp_raw);
                // Note: No "/" at end of event name, because we want DotNETRuntimeRundown as well
                coreclr::handle_coreclr_event(
                    context,
                    core_clr_context,
                    &s,
                    &mut parser,
                    is_in_range,
                );
            }
            _ => {
                if !context.is_in_time_range(timestamp_raw) {
                    return;
                }
                let tid = e.EventHeader.ThreadId;
                if !context.has_thread_at_time(tid, timestamp_raw) {
                    return;
                }

                let task_and_op = s.name().split_once('/').unwrap().1;
                let text = event_properties_to_string(&s, &mut parser, None);
                context.handle_unknown_event(timestamp_raw, tid, task_and_op, text);
            }
        }
    })
}
