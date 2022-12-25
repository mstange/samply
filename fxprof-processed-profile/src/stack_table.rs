use serde::ser::{Serialize, SerializeMap, Serializer};

use crate::category::{
    Category, CategoryHandle, CategoryPairHandle, SerializableSubcategoryColumn, Subcategory,
};
use crate::fast_hash_map::FastHashMap;

#[derive(Debug, Clone, Default)]
pub struct StackTable {
    stack_prefixes: Vec<Option<usize>>,
    stack_frames: Vec<usize>,
    stack_categories: Vec<CategoryHandle>,
    stack_subcategories: Vec<Subcategory>,

    // (parent stack, frame_index) -> stack index
    index: FastHashMap<(Option<usize>, usize), usize>,
}

impl StackTable {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn index_for_stack(
        &mut self,
        prefix: Option<usize>,
        frame: usize,
        category_pair: CategoryPairHandle,
    ) -> usize {
        match self.index.get(&(prefix, frame)) {
            Some(stack) => *stack,
            None => {
                let CategoryPairHandle(category, subcategory_index) = category_pair;
                let subcategory = match subcategory_index {
                    Some(index) => Subcategory::Normal(index),
                    None => Subcategory::Other(category),
                };

                let stack = self.stack_prefixes.len();
                self.stack_prefixes.push(prefix);
                self.stack_frames.push(frame);
                self.stack_categories.push(category);
                self.stack_subcategories.push(subcategory);
                self.index.insert((prefix, frame), stack);
                stack
            }
        }
    }

    pub fn serialize_with_categories<'a>(
        &'a self,
        categories: &'a [Category],
    ) -> impl Serialize + 'a {
        SerializableStackTable {
            table: self,
            categories,
        }
    }
}

struct SerializableStackTable<'a> {
    table: &'a StackTable,
    categories: &'a [Category],
}

impl<'a> Serialize for SerializableStackTable<'a> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let len = self.table.stack_prefixes.len();
        let mut map = serializer.serialize_map(Some(3))?;
        map.serialize_entry("length", &len)?;
        map.serialize_entry("prefix", &self.table.stack_prefixes)?;
        map.serialize_entry("frame", &self.table.stack_frames)?;
        map.serialize_entry("category", &self.table.stack_categories)?;
        map.serialize_entry(
            "subcategory",
            &SerializableSubcategoryColumn(&self.table.stack_subcategories, self.categories),
        )?;
        map.end()
    }
}
