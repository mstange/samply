use framehop::{Module, Unwinder};
use fxprof_processed_profile::{
    CounterHandle, LibraryHandle, MarkerTiming, ProcessHandle, Profile, Timestamp,
};

use super::process_threads::ProcessThreads;

use crate::shared::jit_category_manager::JitCategoryManager;
use crate::shared::jit_function_add_marker::JitFunctionAddMarker;
use crate::shared::jit_function_recycler::JitFunctionRecycler;
use crate::shared::jitdump_manager::JitDumpManager;
use crate::shared::lib_mappings::{LibMappingAdd, LibMappingInfo, LibMappingOp, LibMappingOpQueue};
use crate::shared::perf_map::try_load_perf_map;
use crate::shared::process_sample_data::ProcessSampleData;
use crate::shared::timestamp_converter::TimestampConverter;

use crate::shared::unresolved_samples::UnresolvedSamples;

pub struct Process<U>
where
    U: Unwinder<Module = Module<Vec<u8>>> + Default,
{
    pub profile_process: ProcessHandle,
    pub unwinder: U,
    pub jitdump_manager: JitDumpManager,
    pub lib_mapping_ops: LibMappingOpQueue,
    pub name: Option<String>,
    pub threads: ProcessThreads,
    pub pid: i32,
    pub unresolved_samples: UnresolvedSamples,
    pub jit_function_recycler: Option<JitFunctionRecycler>,
    pub prev_mm_filepages_size: i64,
    pub prev_mm_anonpages_size: i64,
    pub prev_mm_swapents_size: i64,
    pub prev_mm_shmempages_size: i64,
    pub mem_counter: Option<CounterHandle>,
}

impl<U> Process<U>
where
    U: Unwinder<Module = Module<Vec<u8>>> + Default,
{
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

    pub fn reset_for_reuse(&mut self, new_pid: i32) {
        self.pid = new_pid;
        self.threads.pid = new_pid;
    }

    pub fn on_remove(
        &mut self,
        allow_thread_reuse: bool,
        profile: &mut Profile,
        jit_category_manager: &mut JitCategoryManager,
        timestamp_converter: &TimestampConverter,
    ) -> ProcessSampleData {
        self.unwinder = U::default();

        if allow_thread_reuse {
            self.threads.prepare_for_reuse();
        }

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

        if let Some(recycler) = self.jit_function_recycler.as_mut() {
            recycler.finish_round();
        }

        let jitdump_manager = std::mem::replace(
            &mut self.jitdump_manager,
            JitDumpManager::new_for_process(self.threads.main_thread.profile_thread),
        );
        let jitdump_ops = jitdump_manager.finish(
            jit_category_manager,
            profile,
            self.jit_function_recycler.as_mut(),
            timestamp_converter,
        );

        ProcessSampleData::new(
            std::mem::take(&mut self.unresolved_samples),
            std::mem::take(&mut self.lib_mapping_ops),
            jitdump_ops,
            perf_map_mappings,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_regular_lib_mapping(
        &mut self,
        timestamp: u64,
        start_address: u64,
        end_address: u64,
        relative_address_at_start: u32,
        lib_handle: LibraryHandle,
    ) {
        self.lib_mapping_ops.push(
            timestamp,
            LibMappingOp::Add(LibMappingAdd {
                start_avma: start_address,
                end_avma: end_address,
                relative_address_at_start,
                info: LibMappingInfo::new_lib(lib_handle),
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
        profile.add_marker(
            main_thread,
            "JitFunctionAdd",
            JitFunctionAddMarker(symbol_name.unwrap_or("<unknown>").to_owned()),
            timing,
        );

        if let (Some(name), Some(recycler)) = (symbol_name, self.jit_function_recycler.as_mut()) {
            (lib_handle, relative_address_at_start) = recycler.recycle(
                start_address,
                end_address,
                relative_address_at_start,
                name,
                lib_handle,
            );
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
