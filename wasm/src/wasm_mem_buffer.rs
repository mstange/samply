use js_sys;
use wasm_bindgen::prelude::*;

/// WasmMemBuffer lets you allocate a chunk of memory on the wasm heap and
/// directly initialize it from JS without a copy. The constructor takes the
/// allocation size and a callback function which does the initialization.
/// This is useful if you need to get very large amounts of data from JS into
/// wasm (for example, the contents of a 1.7GB libxul.so).
#[wasm_bindgen]
pub struct WasmMemBuffer {
    buffer: Vec<u8>,
}

#[wasm_bindgen]
impl WasmMemBuffer {
    /// Create the buffer and initialize it synchronously in the callback function.
    /// f is called with one argument: the Uint8Array that wraps our buffer.
    /// f should not return anything; its return value is ignored.
    /// f must not call any exported wasm functions! Anything that causes the
    /// wasm heap to resize will invalidate the typed array's internal buffer!
    /// Do not hold on to the array that is passed to f after f completes.
    #[wasm_bindgen(constructor)]
    pub fn new(byte_length: u32, f: &js_sys::Function) -> Self {
        let mut buffer: Vec<u8> = Vec::new();
        buffer.reserve(byte_length as usize);
        unsafe {
            // Let JavaScript fill the buffer without making a copy.
            // We give the callback function access to the wasm memory via a
            // JS Uint8Array which wraps the underlying wasm memory buffer at
            // the appropriate offset and length.
            // The callback function has to fill the buffer with valid contents.
            let array = js_sys::Uint8Array::view_mut_raw(buffer.as_mut_ptr(), byte_length as usize);
            f.call1(&JsValue::NULL, &JsValue::from(array))
                .expect("The callback function should not throw");
            buffer.set_len(byte_length as usize);
        }
        Self { buffer }
    }
}

impl WasmMemBuffer {
    pub fn get(&self) -> &[u8] {
        &self.buffer
    }

    pub fn get_mut(&mut self) -> &[u8] {
        &mut self.buffer
    }
}

impl profiler_get_symbols::OwnedFileData for WasmMemBuffer {
    fn get_data(&self) -> &[u8] {
        self.get()
    }
}
