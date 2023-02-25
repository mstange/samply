use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};

use super::category_color::CategoryColor;

/// A profiling category, can be set on stack frames and markers as part of a [`CategoryPairHandle`].
///
/// Categories can be created with [`Profile::add_category`](crate::Profile::add_category).
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct CategoryHandle(pub(crate) u16);

impl CategoryHandle {
    /// The "Other" category. All profiles have this category.
    pub const OTHER: Self = CategoryHandle(0);
}

impl Serialize for CategoryHandle {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

/// A profiling subcategory, can be set on stack frames and markers as part of a [`CategoryPairHandle`].
///
/// Subategories can be created with [`Profile::add_subcategory`](crate::Profile::add_subcategory).
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct SubcategoryIndex(pub u8);

/// A profiling category pair, consisting of a category and an optional subcategory. Can be set on stack frames and markers.
///
/// Category pairs can be created with [`Profile::add_subcategory`](crate::Profile::add_subcategory)
/// and from a [`CategoryHandle`].
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct CategoryPairHandle(
    pub(crate) CategoryHandle,
    pub(crate) Option<SubcategoryIndex>,
);

impl From<CategoryHandle> for CategoryPairHandle {
    fn from(category: CategoryHandle) -> Self {
        CategoryPairHandle(category, None)
    }
}

/// The information about a category.
#[derive(Debug)]
pub struct Category {
    pub name: String,
    pub color: CategoryColor,
    pub subcategories: Vec<String>,
}

impl Category {
    /// Add a subcategory to this category.
    pub fn add_subcategory(&mut self, subcategory_name: String) -> SubcategoryIndex {
        let subcategory_index = SubcategoryIndex(u8::try_from(self.subcategories.len()).unwrap());
        self.subcategories.push(subcategory_name);
        subcategory_index
    }
}

impl Serialize for Category {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut subcategories = self.subcategories.clone();
        subcategories.push("Other".to_string());

        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("name", &self.name)?;
        map.serialize_entry("color", &self.color)?;
        map.serialize_entry("subcategories", &subcategories)?;
        map.end()
    }
}

#[derive(Debug, Clone)]
pub enum Subcategory {
    Normal(SubcategoryIndex),
    Other(CategoryHandle),
}

pub struct SerializableSubcategoryColumn<'a>(pub &'a [Subcategory], pub &'a [Category]);

impl<'a> Serialize for SerializableSubcategoryColumn<'a> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.0.len()))?;
        for subcategory in self.0 {
            match subcategory {
                Subcategory::Normal(index) => seq.serialize_element(&index.0)?,
                Subcategory::Other(category) => {
                    // There is an implicit "Other" subcategory at the end of each category's
                    // subcategory list.
                    let subcategory_count = self.1[category.0 as usize].subcategories.len();
                    seq.serialize_element(&subcategory_count)?
                }
            }
        }
        seq.end()
    }
}
