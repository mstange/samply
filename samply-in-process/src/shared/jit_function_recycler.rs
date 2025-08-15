use fxprof_processed_profile::LibraryHandle;

use super::types::FastHashMap;

/// When running with --reuse-threads, and we run a process multiple times, and each
/// of that invocations creates similar JIT functions, we want to collapse those "similar"
/// JIT functions into the same JIT function so that the assembly view shows more hits.
///
/// We define "similar" functions as functions which have the same name and code size (in bytes).
#[derive(Debug, Clone, Default)]
pub struct JitFunctionRecycler {
    jit_functions_for_reuse_by_name_and_size: FastHashMap<(String, u32), (LibraryHandle, u32)>,
}

impl JitFunctionRecycler {
    pub fn recycle(
        &mut self,
        name: &str,
        code_size: u32,
        lib_handle: LibraryHandle,
        relative_address: u32,
    ) -> (LibraryHandle, u32) {
        *self
            .jit_functions_for_reuse_by_name_and_size
            .entry((name.to_owned(), code_size))
            .or_insert((lib_handle, relative_address))
    }
}
