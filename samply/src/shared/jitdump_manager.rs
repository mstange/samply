use std::path::{Path, PathBuf};
use std::sync::Arc;

use fxprof_processed_profile::{
    LibraryHandle, MarkerTiming, Profile, Symbol, SymbolTable, ThreadHandle,
};
use linux_perf_data::jitdump::{JitDumpReader, JitDumpRecord, JitDumpRecordType};

use super::jit_category_manager::JitCategoryManager;
use super::jit_function_add_marker::JitFunctionAddMarker;
use super::jit_function_recycler::JitFunctionRecycler;
use super::lib_mappings::{
    LibMappingAdd, LibMappingInfo, LibMappingMove, LibMappingOp, LibMappingOpQueue,
};
use super::timestamp_converter::TimestampConverter;
use super::utils::open_file_with_fallback;

#[derive(Debug)]
pub struct JitDumpManager {
    pending_jitdump_paths: Vec<(ThreadHandle, PathBuf, Vec<PathBuf>)>,
    processors: Vec<SingleJitDumpProcessor>,
    unlink_after_open: bool,
}

impl JitDumpManager {
    pub fn new(unlink_after_open: bool) -> Self {
        JitDumpManager {
            pending_jitdump_paths: Vec::new(),
            processors: Vec::new(),
            unlink_after_open,
        }
    }

    pub fn add_jitdump_path(
        &mut self,
        thread: ThreadHandle,
        path: impl Into<PathBuf>,
        lookup_dirs: Vec<PathBuf>,
    ) {
        self.pending_jitdump_paths
            .push((thread, path.into(), lookup_dirs));
    }

    pub fn process_pending_records(
        &mut self,
        jit_category_manager: &mut JitCategoryManager,
        profile: &mut Profile,
        mut recycler: Option<&mut JitFunctionRecycler>,
        timestamp_converter: &TimestampConverter,
    ) {
        self.pending_jitdump_paths
            .retain_mut(|(thread, path, lookup_dirs)| {
                fn jitdump_reader_for_path(
                    path: &Path,
                    lookup_dirs: &[PathBuf],
                    unlink_after_open: bool,
                ) -> Option<(JitDumpReader<std::fs::File>, PathBuf)> {
                    let (file, path) = open_file_with_fallback(path, lookup_dirs).ok()?;
                    let reader = JitDumpReader::new(file).ok()?;
                    if unlink_after_open {
                        std::fs::remove_file(&path).ok()?;
                    }
                    Some((reader, path))
                }
                let Some((reader, actual_path)) =
                    jitdump_reader_for_path(path, lookup_dirs, self.unlink_after_open)
                else {
                    return true;
                };
                let lib_handle = crate::shared::utils::lib_handle_for_jitdump(
                    &actual_path,
                    reader.header(),
                    profile,
                );
                self.processors
                    .push(SingleJitDumpProcessor::new(reader, lib_handle, *thread));
                false // "Do not retain", i.e. remove from pending_jitdump_paths
            });

        for jitdump in &mut self.processors {
            jitdump.process_pending_records(
                jit_category_manager,
                profile,
                recycler.as_deref_mut(),
                timestamp_converter,
            );
        }
    }

    pub fn finish(
        mut self,
        jit_category_manager: &mut JitCategoryManager,
        profile: &mut Profile,
        recycler: Option<&mut JitFunctionRecycler>,
        timestamp_converter: &TimestampConverter,
    ) -> Vec<LibMappingOpQueue> {
        self.process_pending_records(jit_category_manager, profile, recycler, timestamp_converter);
        self.processors
            .into_iter()
            .map(|processor| processor.finish(profile))
            .collect()
    }
}

#[derive(Debug)]
struct SingleJitDumpProcessor {
    /// Some() until a JIT_CODE_CLOSE record is encountered.
    reader: Option<JitDumpReader<std::fs::File>>,
    lib_handle: LibraryHandle,
    lib_mapping_ops: LibMappingOpQueue,
    symbols: Vec<Symbol>,
    thread_handle: ThreadHandle,

    /// The relative_address of the next JIT function.
    ///
    /// We define the relative address space for Jitdump files as follows:
    /// Pretend that all JIT code is located in sequence, without gaps, in
    /// the order of JIT_CODE_LOAD entries in the file. A given JIT function's
    /// relative address is the sum of the `code_size`s of all the `JIT_CODE_LOAD`
    /// entries that came before it in the file.
    cumulative_address: u32,
}

impl SingleJitDumpProcessor {
    pub fn new(
        reader: JitDumpReader<std::fs::File>,
        lib_handle: LibraryHandle,
        thread_handle: ThreadHandle,
    ) -> Self {
        Self {
            reader: Some(reader),
            lib_handle,
            lib_mapping_ops: Default::default(),
            symbols: Default::default(),
            thread_handle,
            cumulative_address: 0,
        }
    }

    pub fn process_pending_records(
        &mut self,
        jit_category_manager: &mut JitCategoryManager,
        profile: &mut Profile,
        mut recycler: Option<&mut JitFunctionRecycler>,
        timestamp_converter: &TimestampConverter,
    ) {
        let Some(reader) = self.reader.as_mut() else {
            return;
        };
        while let Ok(Some(next_record_header)) = reader.next_record_header() {
            match next_record_header.record_type {
                JitDumpRecordType::JIT_CODE_LOAD
                | JitDumpRecordType::JIT_CODE_MOVE
                | JitDumpRecordType::JIT_CODE_UNWINDING_INFO
                | JitDumpRecordType::JIT_CODE_CLOSE => {
                    // These are interesting.
                }
                _ => {
                    // We skip other records. We especially want to skip JIT_CODE_DEBUG_INFO
                    // records because they can be big and we don't need to read them from
                    // the file.
                    if let Ok(true) = reader.skip_next_record() {
                        continue;
                    } else {
                        break;
                    }
                }
            }
            let Ok(Some(raw_jitdump_record)) = reader.next_record() else {
                break;
            };
            match raw_jitdump_record.parse() {
                Ok(JitDumpRecord::CodeLoad(record)) => {
                    let start_avma = record.code_addr;
                    let code_size = record.code_bytes.len() as u32;
                    let end_avma = start_avma + u64::from(code_size);

                    let relative_address_at_start = self.cumulative_address;
                    self.cumulative_address += code_size;

                    let symbol_name = record.function_name.as_slice();
                    let symbol_name = std::str::from_utf8(&symbol_name).unwrap_or("");
                    self.symbols.push(Symbol {
                        address: relative_address_at_start,
                        size: Some(code_size),
                        name: symbol_name.to_owned(),
                    });

                    let timestamp = timestamp_converter.convert_time(raw_jitdump_record.timestamp);
                    let symbol_name_handle = profile.intern_string(symbol_name);
                    profile.add_marker(
                        self.thread_handle,
                        MarkerTiming::Instant(timestamp),
                        JitFunctionAddMarker(symbol_name_handle),
                    );

                    let (lib_handle, relative_address_at_start) =
                        if let Some(recycler) = recycler.as_deref_mut() {
                            recycler.recycle(
                                symbol_name,
                                code_size,
                                self.lib_handle,
                                relative_address_at_start,
                            )
                        } else {
                            (self.lib_handle, relative_address_at_start)
                        };

                    let (category, js_frame) =
                        jit_category_manager.classify_jit_symbol(symbol_name, profile);
                    self.lib_mapping_ops.push(
                        raw_jitdump_record.timestamp,
                        LibMappingOp::Add(LibMappingAdd {
                            start_avma,
                            end_avma,
                            relative_address_at_start,
                            info: LibMappingInfo::new_jit_function(lib_handle, category, js_frame),
                        }),
                    );
                    // TODO: Add to unwinder so that it can use the code bytes for prologue / epilogue detection
                }
                Ok(JitDumpRecord::CodeMove(record)) => {
                    self.lib_mapping_ops.push(
                        raw_jitdump_record.timestamp,
                        LibMappingOp::Move(LibMappingMove {
                            old_start_avma: record.old_code_addr,
                            new_start_avma: record.new_code_addr,
                            new_end_avma: record.new_code_addr + record.code_size,
                        }),
                    );
                    // TODO: Remove from + add to unwinder
                }
                Ok(JitDumpRecord::CodeUnwindingInfo(_unwinding_info)) => {
                    // TODO: Queue up, and add to unwinder on next CodeLoad
                }
                Ok(JitDumpRecord::CodeClose) => {
                    self.lib_mapping_ops
                        .push(raw_jitdump_record.timestamp, LibMappingOp::Clear);
                    self.close_and_commit_symbol_table(profile);
                    return;
                }
                _ => {}
            }
        }
    }

    fn close_and_commit_symbol_table(&mut self, profile: &mut Profile) {
        if self.reader.is_none() {
            // We're already closed.
            return;
        }

        let symbol_table = SymbolTable::new(std::mem::take(&mut self.symbols));
        profile.set_lib_symbol_table(self.lib_handle, Arc::new(symbol_table));
        self.reader = None;
    }

    pub fn finish(mut self, profile: &mut Profile) -> LibMappingOpQueue {
        self.close_and_commit_symbol_table(profile);
        self.lib_mapping_ops
    }
}
