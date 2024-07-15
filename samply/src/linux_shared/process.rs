use std::path::{Path, PathBuf};

use framehop::Unwinder;
use fxprof_processed_profile::{
    CounterHandle, FrameInfo, LibraryHandle, MarkerTiming, ProcessHandle, Profile, ThreadHandle,
    Timestamp,
};

use super::process_threads::ProcessThreads;
use super::thread::Thread;
use crate::shared::jit_category_manager::JitCategoryManager;
use crate::shared::jit_function_add_marker::JitFunctionAddMarker;
use crate::shared::jit_function_recycler::JitFunctionRecycler;
use crate::shared::jitdump_manager::JitDumpManager;
use crate::shared::lib_mappings::{LibMappingAdd, LibMappingInfo, LibMappingOp, LibMappingOpQueue};
use crate::shared::marker_file::get_markers;
use crate::shared::perf_map::try_load_perf_map;
use crate::shared::process_sample_data::{MarkerSpanOnThread, ProcessSampleData};
use crate::shared::recycling::{ProcessRecyclingData, ThreadRecycler};
use crate::shared::synthetic_jit_library::SyntheticJitLibrary;
use crate::shared::timestamp_converter::TimestampConverter;
use crate::shared::unresolved_samples::UnresolvedSamples;

pub struct Process<U> {
    pub profile_process: ProcessHandle,
    pub unwinder: U,
    pub jitdump_manager: JitDumpManager,
    pub lib_mapping_ops: LibMappingOpQueue,
    pub name: Option<String>,
    pub threads: ProcessThreads,
    pub pid: i32,
    pub unresolved_samples: UnresolvedSamples,
    pub jit_app_cache_mapping_ops: LibMappingOpQueue,
    pub jit_function_recycler: Option<JitFunctionRecycler>,
    marker_file_paths: Vec<(ThreadHandle, PathBuf, Vec<PathBuf>)>,
    pub prev_mm_filepages_size: i64,
    pub prev_mm_anonpages_size: i64,
    pub prev_mm_swapents_size: i64,
    pub prev_mm_shmempages_size: i64,
    pub mem_counter: Option<CounterHandle>,
}

pub struct ProcessForkData<U> {
    unwinder: U,
    lib_mapping_ops: LibMappingOpQueue,
}

impl<U> Process<U>
where
    U: Unwinder + Default,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        pid: i32,
        process_handle: ProcessHandle,
        main_thread_handle: ThreadHandle,
        main_thread_label_frame: FrameInfo,
        name: Option<String>,
        thread_recycler: Option<ThreadRecycler>,
        jit_function_recycler: Option<JitFunctionRecycler>,
        unlink_aux_files: bool,
    ) -> Self {
        Self {
            profile_process: process_handle,
            unwinder: U::default(),
            jitdump_manager: JitDumpManager::new(unlink_aux_files),
            lib_mapping_ops: Default::default(),
            name: name.clone(),
            pid,
            threads: ProcessThreads::new(
                pid,
                process_handle,
                main_thread_handle,
                main_thread_label_frame,
                name,
                thread_recycler,
            ),
            unresolved_samples: Default::default(),
            jit_app_cache_mapping_ops: LibMappingOpQueue::default(),
            jit_function_recycler,
            marker_file_paths: Vec::new(),
            prev_mm_filepages_size: 0,
            prev_mm_anonpages_size: 0,
            prev_mm_swapents_size: 0,
            prev_mm_shmempages_size: 0,
            mem_counter: None,
        }
    }

    /// Called when this process forks and creates a child process.
    pub fn clone_fork_data(&self) -> ProcessForkData<U> {
        ProcessForkData {
            unwinder: self.unwinder.clone(),
            lib_mapping_ops: self.lib_mapping_ops.clone(),
        }
    }

    /// Called on the child process that was created by the fork.
    pub fn adopt_fork_data_from_parent(&mut self, fork_data: ProcessForkData<U>) {
        self.unwinder = fork_data.unwinder;
        self.lib_mapping_ops = fork_data.lib_mapping_ops;
    }

    pub fn rename_with_recycling(
        &mut self,
        name: String,
        recycling_data: ProcessRecyclingData,
    ) -> (ProcessRecyclingData, Option<String>) {
        let ProcessRecyclingData {
            process_handle,
            main_thread_recycling_data,
            thread_recycler,
            jit_function_recycler,
        } = recycling_data;
        let old_process_handle = std::mem::replace(&mut self.profile_process, process_handle);
        let old_jit_function_recycler =
            std::mem::replace(&mut self.jit_function_recycler, Some(jit_function_recycler));
        let (old_thread_recycler, old_main_thread_recycling_data) =
            self.threads.rename_process_with_recycling(
                name.clone(),
                process_handle,
                main_thread_recycling_data,
                thread_recycler,
            );
        let old_name = std::mem::replace(&mut self.name, Some(name));
        let recycling_data = ProcessRecyclingData {
            process_handle: old_process_handle,
            main_thread_recycling_data: old_main_thread_recycling_data,
            thread_recycler: old_thread_recycler,
            jit_function_recycler: old_jit_function_recycler
                .expect("jit_function_recycler should be Some"),
        };
        (recycling_data, old_name)
    }

    pub fn rename_without_recycling(
        &mut self,
        name: String,
        main_thread_label_frame: FrameInfo,
        profile: &mut Profile,
    ) {
        profile.set_process_name(self.profile_process, &name);
        self.threads.main_thread.rename_without_recycling(
            name.clone(),
            main_thread_label_frame,
            profile,
        );
        self.name = Some(name);
    }

    pub fn recycle_or_get_new_thread(
        &mut self,
        tid: i32,
        name: Option<String>,
        start_time: Timestamp,
        profile: &mut Profile,
    ) -> &mut Thread {
        self.threads
            .recycle_or_get_new_thread(tid, name, start_time, profile)
    }

    pub fn check_jitdump(
        &mut self,
        jit_category_manager: &mut JitCategoryManager,
        profile: &mut Profile,
        timestamp_converter: &TimestampConverter,
    ) {
        self.jitdump_manager.process_pending_records(
            jit_category_manager,
            profile,
            self.jit_function_recycler.as_mut(),
            timestamp_converter,
        );
    }

    pub fn add_marker_file_path(
        &mut self,
        thread: ThreadHandle,
        path: &Path,
        lookup_dirs: Vec<PathBuf>,
    ) {
        self.marker_file_paths
            .push((thread, path.to_owned(), lookup_dirs));
    }

    pub fn notify_dead(&mut self, end_time: Timestamp, profile: &mut Profile) {
        self.threads.notify_process_dead(end_time, profile);
        profile.set_process_end_time(self.profile_process, end_time);
    }

    pub fn finish(
        mut self,
        profile: &mut Profile,
        jit_category_manager: &mut JitCategoryManager,
        timestamp_converter: &TimestampConverter,
    ) -> (ProcessSampleData, Option<(String, ProcessRecyclingData)>) {
        self.unwinder = U::default();

        let perf_map_mappings = if !self.unresolved_samples.is_empty() {
            try_load_perf_map(
                self.pid as u32,
                profile,
                jit_category_manager,
                self.jit_function_recycler.as_mut(),
            )
        } else {
            None
        };

        let jitdump_manager =
            std::mem::replace(&mut self.jitdump_manager, JitDumpManager::new(false));
        let mut jitdump_ops = jitdump_manager.finish(
            jit_category_manager,
            profile,
            self.jit_function_recycler.as_mut(),
            timestamp_converter,
        );

        if !self.jit_app_cache_mapping_ops.is_empty() {
            jitdump_ops.insert(0, self.jit_app_cache_mapping_ops);
        }

        let mut marker_spans = Vec::new();
        for (thread_handle, marker_file_path, lookup_dirs) in self.marker_file_paths {
            if let Ok(marker_spans_from_this_file) =
                get_markers(&marker_file_path, &lookup_dirs, *timestamp_converter)
            {
                marker_spans.extend(marker_spans_from_this_file.into_iter().map(|span| {
                    MarkerSpanOnThread {
                        thread_handle,
                        start_time: span.start_time,
                        end_time: span.end_time,
                        name: span.name,
                    }
                }));
            }
        }

        let process_sample_data = ProcessSampleData::new(
            std::mem::take(&mut self.unresolved_samples),
            std::mem::take(&mut self.lib_mapping_ops),
            jitdump_ops,
            perf_map_mappings,
            marker_spans,
        );

        let thread_recycler = self.threads.finish();

        let process_recycling_data = if let (
            Some(name),
            Some(jit_function_recycler),
            (Some(thread_recycler), main_thread_recycling_data),
        ) = (self.name, self.jit_function_recycler, thread_recycler)
        {
            let recycling_data = ProcessRecyclingData {
                process_handle: self.profile_process,
                main_thread_recycling_data,
                thread_recycler,
                jit_function_recycler,
            };
            Some((name, recycling_data))
        } else {
            None
        };

        (process_sample_data, process_recycling_data)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_regular_lib_mapping(
        &mut self,
        timestamp: u64,
        start_address: u64,
        end_address: u64,
        relative_address_at_start: u32,
        info: LibMappingInfo,
    ) {
        self.lib_mapping_ops.push(
            timestamp,
            LibMappingOp::Add(LibMappingAdd {
                start_avma: start_address,
                end_avma: end_address,
                relative_address_at_start,
                info,
            }),
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_lib_mapping_for_injected_jit_lib(
        &mut self,
        timestamp: u64,
        profile_timestamp: Timestamp,
        symbol_name: Option<&str>,
        start_address: u64,
        end_address: u64,
        mut relative_address_at_start: u32,
        mut lib_handle: LibraryHandle,
        jit_category_manager: &mut JitCategoryManager,
        profile: &mut Profile,
    ) {
        let main_thread = self.threads.main_thread.profile_thread;
        let timing = MarkerTiming::Instant(profile_timestamp);
        let name = match symbol_name {
            Some(name) => profile.intern_string(name),
            None => profile.intern_string("<unknown>"),
        };
        profile.add_marker(main_thread, timing, JitFunctionAddMarker(name));

        if let (Some(name), Some(recycler)) = (symbol_name, self.jit_function_recycler.as_mut()) {
            let code_size = (end_address - start_address) as u32;
            (lib_handle, relative_address_at_start) =
                recycler.recycle(name, code_size, lib_handle, relative_address_at_start);
        }

        let (category, js_frame) =
            jit_category_manager.classify_jit_symbol(symbol_name.unwrap_or(""), profile);
        self.lib_mapping_ops.push(
            timestamp,
            LibMappingOp::Add(LibMappingAdd {
                start_avma: start_address,
                end_avma: end_address,
                relative_address_at_start,
                info: LibMappingInfo::new_jit_function(lib_handle, category, js_frame),
            }),
        );
    }

    pub fn add_jit_function(
        &mut self,
        timestamp_raw: u64,
        jit_lib: &mut SyntheticJitLibrary,
        name: String,
        start_avma: u64,
        size: u32,
        info: LibMappingInfo,
    ) {
        let relative_address = jit_lib.add_function(name, size);

        self.jit_app_cache_mapping_ops.push(
            timestamp_raw,
            LibMappingOp::Add(LibMappingAdd {
                start_avma,
                end_avma: start_avma + u64::from(size),
                relative_address_at_start: relative_address,
                info,
            }),
        );
    }

    pub fn get_or_make_mem_counter(&mut self, profile: &mut Profile) -> CounterHandle {
        *self.mem_counter.get_or_insert_with(|| {
            profile.add_counter(
                self.profile_process,
                "malloc",
                "Memory",
                "Amount of allocated memory",
            )
        })
    }
}
