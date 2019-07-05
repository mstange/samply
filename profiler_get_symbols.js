(function() {
    const __exports = {};
    let wasm;

    let cachegetUint32Memory = null;
    function getUint32Memory() {
        if (cachegetUint32Memory === null || cachegetUint32Memory.buffer !== wasm.memory.buffer) {
            cachegetUint32Memory = new Uint32Array(wasm.memory.buffer);
        }
        return cachegetUint32Memory;
    }

    function getArrayU32FromWasm(ptr, len) {
        return getUint32Memory().subarray(ptr / 4, ptr / 4 + len);
    }

    let cachedGlobalArgumentPtr = null;
    function globalArgumentPtr() {
        if (cachedGlobalArgumentPtr === null) {
            cachedGlobalArgumentPtr = wasm.__wbindgen_global_argument_ptr();
        }
        return cachedGlobalArgumentPtr;
    }

    let cachegetUint8Memory = null;
    function getUint8Memory() {
        if (cachegetUint8Memory === null || cachegetUint8Memory.buffer !== wasm.memory.buffer) {
            cachegetUint8Memory = new Uint8Array(wasm.memory.buffer);
        }
        return cachegetUint8Memory;
    }

    function getArrayU8FromWasm(ptr, len) {
        return getUint8Memory().subarray(ptr / 1, ptr / 1 + len);
    }

    let WASM_VECTOR_LEN = 0;

    function passArray8ToWasm(arg) {
        const ptr = wasm.__wbindgen_malloc(arg.length * 1);
        getUint8Memory().set(arg, ptr / 1);
        WASM_VECTOR_LEN = arg.length;
        return ptr;
    }

    let cachedTextEncoder = new TextEncoder('utf-8');

    let passStringToWasm;
    if (typeof cachedTextEncoder.encodeInto === 'function') {
        passStringToWasm = function(arg) {


            let size = arg.length;
            let ptr = wasm.__wbindgen_malloc(size);
            let offset = 0;
            {
                const mem = getUint8Memory();
                for (; offset < arg.length; offset++) {
                    const code = arg.charCodeAt(offset);
                    if (code > 0x7F) break;
                    mem[ptr + offset] = code;
                }
            }

            if (offset !== arg.length) {
                arg = arg.slice(offset);
                ptr = wasm.__wbindgen_realloc(ptr, size, size = offset + arg.length * 3);
                const view = getUint8Memory().subarray(ptr + offset, ptr + size);
                const ret = cachedTextEncoder.encodeInto(arg, view);

                offset += ret.written;
            }
            WASM_VECTOR_LEN = offset;
            return ptr;
        };
    } else {
        passStringToWasm = function(arg) {


            let size = arg.length;
            let ptr = wasm.__wbindgen_malloc(size);
            let offset = 0;
            {
                const mem = getUint8Memory();
                for (; offset < arg.length; offset++) {
                    const code = arg.charCodeAt(offset);
                    if (code > 0x7F) break;
                    mem[ptr + offset] = code;
                }
            }

            if (offset !== arg.length) {
                const buf = cachedTextEncoder.encode(arg.slice(offset));
                ptr = wasm.__wbindgen_realloc(ptr, size, size = offset + buf.length);
                getUint8Memory().set(buf, ptr + offset);
                offset += buf.length;
            }
            WASM_VECTOR_LEN = offset;
            return ptr;
        };
    }
    /**
    * @param {Uint8Array} binary_data
    * @param {Uint8Array} debug_data
    * @param {string} breakpad_id
    * @param {CompactSymbolTable} dest
    * @returns {boolean}
    */
    __exports.get_compact_symbol_table = function(binary_data, debug_data, breakpad_id, dest) {
        const ptr0 = passArray8ToWasm(binary_data);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passArray8ToWasm(debug_data);
        const len1 = WASM_VECTOR_LEN;
        const ptr2 = passStringToWasm(breakpad_id);
        const len2 = WASM_VECTOR_LEN;
        try {
            return (wasm.get_compact_symbol_table(ptr0, len0, ptr1, len1, ptr2, len2, dest.ptr)) !== 0;

        } finally {
            wasm.__wbindgen_free(ptr0, len0 * 1);
            wasm.__wbindgen_free(ptr1, len1 * 1);
            wasm.__wbindgen_free(ptr2, len2 * 1);

        }

    };

    let cachedTextDecoder = new TextDecoder('utf-8');

    function getStringFromWasm(ptr, len) {
        return cachedTextDecoder.decode(getUint8Memory().subarray(ptr, ptr + len));
    }
    /**
    */
    class CompactSymbolTable {

        free() {
            const ptr = this.ptr;
            this.ptr = 0;

            wasm.__wbg_compactsymboltable_free(ptr);
        }
        /**
        * @returns {}
        */
        constructor() {
            this.ptr = wasm.compactsymboltable_new();
        }
        /**
        * @returns {Uint32Array}
        */
        take_addr() {
            const retptr = globalArgumentPtr();
            wasm.compactsymboltable_take_addr(retptr, this.ptr);
            const mem = getUint32Memory();
            const rustptr = mem[retptr / 4];
            const rustlen = mem[retptr / 4 + 1];

            const realRet = getArrayU32FromWasm(rustptr, rustlen).slice();
            wasm.__wbindgen_free(rustptr, rustlen * 4);
            return realRet;

        }
        /**
        * @returns {Uint32Array}
        */
        take_index() {
            const retptr = globalArgumentPtr();
            wasm.compactsymboltable_take_index(retptr, this.ptr);
            const mem = getUint32Memory();
            const rustptr = mem[retptr / 4];
            const rustlen = mem[retptr / 4 + 1];

            const realRet = getArrayU32FromWasm(rustptr, rustlen).slice();
            wasm.__wbindgen_free(rustptr, rustlen * 4);
            return realRet;

        }
        /**
        * @returns {Uint8Array}
        */
        take_buffer() {
            const retptr = globalArgumentPtr();
            wasm.compactsymboltable_take_buffer(retptr, this.ptr);
            const mem = getUint32Memory();
            const rustptr = mem[retptr / 4];
            const rustlen = mem[retptr / 4 + 1];

            const realRet = getArrayU8FromWasm(rustptr, rustlen).slice();
            wasm.__wbindgen_free(rustptr, rustlen * 1);
            return realRet;

        }
    }
    __exports.CompactSymbolTable = CompactSymbolTable;

    function init(module) {

        let result;
        const imports = {};
        imports.wbg = {};
        imports.wbg.__wbindgen_throw = function(arg0, arg1) {
            let varg0 = getStringFromWasm(arg0, arg1);
            throw new Error(varg0);
        };

        if (module instanceof URL || typeof module === 'string' || module instanceof Request) {

            const response = fetch(module);
            if (typeof WebAssembly.instantiateStreaming === 'function') {
                result = WebAssembly.instantiateStreaming(response, imports)
                .catch(e => {
                    console.warn("`WebAssembly.instantiateStreaming` failed. Assuming this is because your server does not serve wasm with `application/wasm` MIME type. Falling back to `WebAssembly.instantiate` which is slower. Original error:\n", e);
                    return response
                    .then(r => r.arrayBuffer())
                    .then(bytes => WebAssembly.instantiate(bytes, imports));
                });
            } else {
                result = response
                .then(r => r.arrayBuffer())
                .then(bytes => WebAssembly.instantiate(bytes, imports));
            }
        } else {

            result = WebAssembly.instantiate(module, imports)
            .then(result => {
                if (result instanceof WebAssembly.Instance) {
                    return { instance: result, module };
                } else {
                    return result;
                }
            });
        }
        return result.then(({instance, module}) => {
            wasm = instance.exports;
            init.__wbindgen_wasm_module = module;

            return wasm;
        });
    }

    self.wasm_bindgen = Object.assign(init, __exports);

})();
