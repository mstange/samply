use std::{borrow::Cow, collections::HashMap};

use crate::{
    generation::SymbolMapGeneration, source_file_path::SourceFilePathIndex, SourceFilePathHandle,
};

#[derive(Debug)]
pub struct PathInterner<'a> {
    symbol_map_generation: SymbolMapGeneration,
    borrowed_strings: Vec<&'a str>,
    index_for_borrowed_string: HashMap<&'a str, usize>,
    owned_strings: Vec<String>,
    index_for_owned_string: HashMap<String, usize>,
}

impl<'a> PathInterner<'a> {
    pub fn new(symbol_map_generation: SymbolMapGeneration) -> Self {
        Self {
            symbol_map_generation,
            borrowed_strings: Default::default(),
            index_for_borrowed_string: Default::default(),
            owned_strings: Default::default(),
            index_for_owned_string: Default::default(),
        }
    }

    pub fn intern_cow(&mut self, cow: Cow<'a, str>) -> SourceFilePathHandle {
        match cow {
            Cow::Borrowed(s) => self.intern(s),
            Cow::Owned(s) => self.intern_owned(&s),
        }
    }

    pub fn intern(&mut self, s: &'a str) -> SourceFilePathHandle {
        let index = self.intern_inner(s);
        self.symbol_map_generation.source_file_handle(index)
    }

    fn intern_inner(&mut self, s: &'a str) -> SourceFilePathIndex {
        if let Some(index) = self.index_for_borrowed_string.get(s) {
            return SourceFilePathIndex((*index as u32) << 1);
        }
        if let Some(index) = self.index_for_owned_string.get(s) {
            return SourceFilePathIndex(((*index as u32) << 1) | 1);
        }
        let index = self.borrowed_strings.len();
        self.borrowed_strings.push(s);
        self.index_for_borrowed_string.insert(s, index);
        SourceFilePathIndex((index as u32) << 1)
    }

    pub fn intern_owned(&mut self, s: &str) -> SourceFilePathHandle {
        let index = self.intern_owned_inner(s);
        self.symbol_map_generation.source_file_handle(index)
    }

    pub fn intern_owned_inner(&mut self, s: &str) -> SourceFilePathIndex {
        if let Some(index) = self.index_for_borrowed_string.get(s) {
            return SourceFilePathIndex((*index as u32) << 1);
        }
        if let Some(index) = self.index_for_owned_string.get(s) {
            return SourceFilePathIndex(((*index as u32) << 1) | 1);
        }
        let index = self.owned_strings.len();
        self.owned_strings.push(s.to_string());
        self.index_for_owned_string.insert(s.to_string(), index);
        SourceFilePathIndex(((index as u32) << 1) | 1)
    }

    pub fn resolve(&self, handle: SourceFilePathHandle) -> Option<Cow<'a, str>> {
        let index = self.symbol_map_generation.unwrap_source_file_index(handle);
        match index.0 & 1 {
            0 => self
                .borrowed_strings
                .get((index.0 >> 1) as usize)
                .map(|s| Cow::Borrowed(*s)),
            1 => self
                .owned_strings
                .get((index.0 >> 1) as usize)
                .map(|s| Cow::Owned(s.clone())),
            _ => unreachable!(),
        }
    }
}
