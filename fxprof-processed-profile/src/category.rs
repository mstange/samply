use serde::ser::{Serialize, SerializeMap, Serializer};

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
pub struct SubcategoryIndex(pub u16);

impl SubcategoryIndex {
    /// The "Other" subcategory. All categories have this subcategory as their first subcategory.
    pub const OTHER: Self = SubcategoryIndex(0);
}

/// A profiling category pair, consisting of a category and an optional subcategory. Can be set on stack frames and markers.
///
/// Category pairs can be created with [`Profile::add_subcategory`](crate::Profile::add_subcategory)
/// and from a [`CategoryHandle`].
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct CategoryPairHandle(pub(crate) CategoryHandle, pub(crate) SubcategoryIndex);

impl From<CategoryHandle> for CategoryPairHandle {
    fn from(category: CategoryHandle) -> Self {
        CategoryPairHandle(category, SubcategoryIndex::OTHER)
    }
}

/// The information about a category.
#[derive(Debug)]
pub struct InternalCategory {
    name: String,
    color: CategoryColor,
    subcategories: Vec<String>,
}

impl InternalCategory {
    pub fn new(name: String, color: CategoryColor) -> Self {
        let subcategories = vec!["Other".to_string()];
        Self {
            name,
            color,
            subcategories,
        }
    }

    /// Add a subcategory to this category.
    pub fn add_subcategory(&mut self, subcategory_name: String) -> SubcategoryIndex {
        let subcategory_index = SubcategoryIndex(u16::try_from(self.subcategories.len()).unwrap());
        self.subcategories.push(subcategory_name);
        subcategory_index
    }
}

impl Serialize for InternalCategory {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("name", &self.name)?;
        map.serialize_entry("color", &self.color)?;
        map.serialize_entry("subcategories", &self.subcategories)?;
        map.end()
    }
}

impl Serialize for SubcategoryIndex {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}
