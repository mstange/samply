use debugid::DebugId;
use fxprof_processed_profile::{
    LibraryHandle, LibraryInfo, Profile, SourceLocation, StringHandle, SubcategoryHandle,
};

use super::lib_mappings::JitSymbolInfo;
use super::types::FastHashMap;

#[derive(Debug)]
pub struct SyntheticJitLibrary {
    lib_handle: LibraryHandle,
    default_category: SubcategoryHandle,
    next_relative_address: u32,
    recycler: Option<FastHashMap<(StringHandle, u32), u32>>,
}

impl SyntheticJitLibrary {
    pub fn new(
        name: String,
        default_category: SubcategoryHandle,
        profile: &mut Profile,
        allow_recycling: bool,
    ) -> Self {
        let lib_handle = profile.add_lib(LibraryInfo {
            name: name.clone(),
            debug_name: name.clone(),
            path: name.clone(),
            debug_path: name,
            debug_id: DebugId::nil(),
            code_id: None,
            arch: None,
        });
        let recycler = if allow_recycling {
            Some(FastHashMap::default())
        } else {
            None
        };
        Self {
            lib_handle,
            default_category,
            next_relative_address: 0,
            recycler,
        }
    }

    /// Add a function to this library and return its native symbol.
    ///
    /// `source_location` ends up on the native frames for this function. If a JS
    /// label frame gets prepended to those frames, that label frame carries its
    /// own source location instead.
    pub fn add_function(
        &mut self,
        name: &str,
        size: u32,
        source_location: SourceLocation,
        profile: &mut Profile,
    ) -> JitSymbolInfo {
        let name = profile.handle_for_string(name);
        let symbol_address = self.relative_address_for_function(name, size);
        JitSymbolInfo {
            lib_handle: self.lib_handle,
            name,
            symbol_address,
            symbol_size: Some(size),
            source_location,
        }
    }

    fn relative_address_for_function(&mut self, name: StringHandle, size: u32) -> u32 {
        if let Some(recycler) = &self.recycler {
            if let Some(&relative_address) = recycler.get(&(name, size)) {
                return relative_address;
            }
        }
        let relative_address = self.next_relative_address;
        self.next_relative_address += size;
        if let Some(recycler) = &mut self.recycler {
            recycler.insert((name, size), relative_address);
        }
        relative_address
    }

    pub fn lib_handle(&self) -> LibraryHandle {
        self.lib_handle
    }

    pub fn default_category(&self) -> SubcategoryHandle {
        self.default_category
    }
}
