use fxprof_processed_profile::LibraryHandle;

use super::types::FastHashMap;

/// When running with --merge-threads, and we run a process multiple times, and each
/// of that invocations creates similar JIT functions, we want to collapse those "similar"
/// JIT functions into the same JIT function so that the assembly view shows more hits.
///
/// We define "similar" functions as functions which have the same name and code size (in bytes).
#[derive(Debug, Clone, Default)]
pub struct JitFunctionRecycler {
    new_jit_functions: Vec<(String, u32, LibraryHandle, u32)>,
    jit_functions_for_reuse_by_name_and_size: FastHashMap<(String, u32), (LibraryHandle, u32)>,
}

impl JitFunctionRecycler {
    pub fn recycle(
        &mut self,
        start_address: u64,
        end_address: u64,
        relative_address: u32,
        name: &str,
        lib_handle: LibraryHandle,
    ) -> (LibraryHandle, u32) {
        let code_size = (end_address - start_address) as u32;
        let name_and_code_size = (name.to_owned(), code_size);
        match self
            .jit_functions_for_reuse_by_name_and_size
            .get(&name_and_code_size)
        {
            Some(reused_function) => *reused_function,
            None => {
                self.new_jit_functions.push((
                    name_and_code_size.0,
                    code_size,
                    lib_handle,
                    relative_address,
                ));
                (lib_handle, relative_address)
            }
        }
    }

    pub fn finish_round(&mut self) {
        for (name, code_size, lib_handle, rel_addr) in self.new_jit_functions.drain(..) {
            self.jit_functions_for_reuse_by_name_and_size
                .insert((name, code_size), (lib_handle, rel_addr));
        }
    }
}
