use std::ops::Deref;
use std::sync::Arc;

#[derive(Debug)]
pub struct RawModuleData {
    base_ptr: *const u8,
    byte_len: usize,
}

impl RawModuleData {
    /// # Safety
    ///
    /// - base_ptr must be a valid pointer to initialized data of length byte_len
    /// - The RawModuleData object must not outlive this data. It must be dropped
    ///   before the module is unloaded.
    pub unsafe fn new(base_ptr: *const u8, byte_len: usize) -> Self {
        Self { base_ptr, byte_len}
    }

    pub fn slice(
        self: &Arc<Self>,
        start_byte_offset: usize,
        byte_len: usize,
    ) -> Option<ModuleDataSlice> {
        if start_byte_offset > self.byte_len {
            return None;
        }
        let end_byte_offset = start_byte_offset.checked_add(byte_len)?;
        if end_byte_offset > self.byte_len {
            return None;
        }
        Some(ModuleDataSlice {
            module_data: Arc::clone(self),
            start_byte_offset,
            byte_len,
        })
    }

    /// # Safety
    ///
    /// The passed offsets must describe a range within 0..self.byte_len.
    unsafe fn make_u8_slice(&self, start_byte_offset: usize, byte_len: usize) -> &[u8] {
        let start_ptr = unsafe { self.base_ptr.byte_add(start_byte_offset) };
        unsafe { std::slice::from_raw_parts(start_ptr, byte_len) }
    }
}

unsafe impl Send for RawModuleData {}
unsafe impl Sync for RawModuleData {}

#[derive(Debug)]
pub struct ModuleDataSlice {
    module_data: Arc<RawModuleData>,
    start_byte_offset: usize,
    byte_len: usize,
}

impl Deref for ModuleDataSlice {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        // Safety: validated range during construction
        unsafe {
            self.module_data
                .make_u8_slice(self.start_byte_offset, self.byte_len)
        }
    }
}
