use std::ffi::c_void;
use std::sync::Arc;
use std::{collections::HashMap, num::NonZeroU64};

use framehop::{
    CacheNative, FrameAddress, Module, MustNotAllocateDuringUnwind, UnwindRegsNative, Unwinder, UnwinderNative
};

use crate::macho::get_module_macho;

use super::module_data::{ModuleDataSlice, RawModuleData};

pub struct InProcessUnwinder {
    unwinder: UnwinderNative<ModuleDataSlice, MustNotAllocateDuringUnwind>,
    all_modules: HashMap<usize, Arc<RawModuleData>>,
}

pub struct InProcessUnwinderCache(CacheNative<MustNotAllocateDuringUnwind>);

impl Default for InProcessUnwinderCache {
    fn default() -> Self {
        Self::new()
    }
}

impl InProcessUnwinderCache {
    pub fn new() -> Self {
        Self(CacheNative::new_in())
    }
}

#[repr(C)]
pub struct InProcessUnwinderRegs {
    pub pc: u64,
    pub lr: u64,
    pub sp: u64,
    pub fp: u64,
}

pub struct StackCollectionOutput<'a> {
    pub pc_buf: &'a mut [usize],
    pub sp_buf: &'a mut [usize],
    pub count: &'a mut usize,
}

impl Default for InProcessUnwinder {
    fn default() -> Self {
        Self::new()
    }
}

impl InProcessUnwinder {
    pub fn new() -> Self {
        Self {
            unwinder: UnwinderNative::new(),
            all_modules: HashMap::new(),
        }
    }

    ///
    /// stack_mem must start at init_regs.sp and end at or before the thread's stack base.
    pub fn walk_stack_and_collect(
        &self,
        init_regs: &InProcessUnwinderRegs,
        stack_mem: &[usize],
        cache: &mut InProcessUnwinderCache,
        output: &mut StackCollectionOutput,
    ) {
        let mut regs = UnwindRegsNative::new(init_regs.lr, init_regs.sp, init_regs.fp);

        let mut read_stack = |addr| match stack_mem.get(((addr - init_regs.sp) / 8) as usize) {
            Some(val) => Ok(*val as u64),
            None => Err(()),
        };

        output.pc_buf[0] = init_regs.pc as usize;
        output.sp_buf[0] = init_regs.sp as usize;

        let mut count = 1;
        let max_count = output.pc_buf.len().min(output.sp_buf.len());

        let mut address = FrameAddress::InstructionPointer(init_regs.pc);
        while count < max_count {
            let Ok(Some(return_address)) =
                self.unwinder
                    .unwind_frame(address, &mut regs, &mut cache.0, &mut read_stack)
            else {
                break;
            };
            output.pc_buf[count] = return_address as usize;
            output.sp_buf[count] = regs.sp() as usize;
            count += 1;

            let Some(return_address) = NonZeroU64::new(return_address) else {
                break;
            };
            address = FrameAddress::ReturnAddress(return_address);
        }

        *output.count = count;
    }

    /// # Safety
    ///
    /// - base_ptr must be a (correctly-aligned) pointer to a mach_header_64
    /// - The loaded library must be guaranteed to remain loaded until after a
    ///   call to on_before_module_removed_from_process
    pub unsafe fn add_module_macho(&mut self, base_ptr: *const c_void) {
        let (module_data, module) = get_module_macho(base_ptr).unwrap();
        self.add_module(module_data, module);
    }

    fn add_module(&mut self, module_data: Arc<RawModuleData>, module: Module<ModuleDataSlice>) {
        let base_addr = module.base_avma() as usize;
        let prev_val = self.all_modules.insert(base_addr, Arc::clone(&module_data));
        if prev_val.is_some() {
            panic!(
                "Duplicate call to InProcessUnwinder::on_module_added_to_process, or missing call to InProcessUnwinder::on_before_module_removed_from_process"
            );
        }

        self.unwinder.add_module(module);
    }

    /// # Safety
    ///
    /// - base_ptr must be a (correctly-aligned) pointer to a mach_header_64
    /// - The library must still be loaded at the point of this call.
    ///
    /// # Panics
    ///
    /// This function is not expected to panic. If it panics, that's because it
    /// detected that the data for the removed module is still used somewhere even
    /// though we were expecting all references to be gone once we remove it from
    /// the unwinder.
    pub unsafe fn on_before_module_removed_from_process(&mut self, base_ptr: *const c_void) {
        let base_addr = base_ptr as usize;
        let module_data = self.all_modules.remove(&base_addr);
        let Some(module_data) = module_data else {
            // Incorrect call?
            return;
        };

        let base_addr = base_addr as u64;
        self.unwinder.remove_module(base_addr);

        // Now module_data should be the last owner of the Arc.
        let Some(inner_module_data) = Arc::into_inner(module_data) else {
            panic!(
                "If Arc::into_inner fails, it means that someone else is still holding on to the module data. \
                We cannot allow this because the module is about to be unloaded."
            );
        };
        let _ = inner_module_data;
    }
}
