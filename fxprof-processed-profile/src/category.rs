use serde::ser::{Serialize, SerializeMap, Serializer};

use super::category_color::CategoryColor;
use super::fast_hash_map::FastHashMap;

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct Category<'a>(pub &'a str, pub CategoryColor);

impl hashbrown::Equivalent<(String, CategoryColor)> for Category<'_> {
    fn equivalent(&self, key: &(String, CategoryColor)) -> bool {
        let Category(name_l, color_l) = self;
        let (name_r, color_r) = key;
        name_l == name_r && color_l == color_r
    }
}

pub struct Subcategory<'a>(pub Category<'a>, pub &'a str);

/// A profiling category, can be set on stack frames and markers as part of a [`SubcategoryHandle`].
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

/// A profiling subcategory, can be set on stack frames and markers as part of a [`SubcategoryHandle`].
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
pub struct SubcategoryHandle(pub(crate) CategoryHandle, pub(crate) SubcategoryIndex);

impl From<CategoryHandle> for SubcategoryHandle {
    fn from(category: CategoryHandle) -> Self {
        SubcategoryHandle(category, SubcategoryIndex::OTHER)
    }
}

/// The information about a category.
#[derive(Debug)]
pub struct InternalCategory {
    name: String,
    color: CategoryColor,
    subcategories: Vec<String>,
    subcategory_map: FastHashMap<String, SubcategoryIndex>,
}

impl InternalCategory {
    pub fn new(name: &str, color: CategoryColor) -> Self {
        let subcategories = vec!["Other".to_string()];
        let mut subcategory_map = FastHashMap::with_capacity_and_hasher(1, Default::default());
        subcategory_map.insert("Other".to_string(), SubcategoryIndex(0));
        Self {
            name: name.to_string(),
            color,
            subcategories,
            subcategory_map,
        }
    }

    /// Get or create a subcategory to this category.
    pub fn index_for_subcategory(&mut self, subcategory_name: &str) -> SubcategoryIndex {
        match self.subcategory_map.get(subcategory_name) {
            Some(handle) => *handle,
            None => {
                let handle = SubcategoryIndex(u16::try_from(self.subcategories.len()).unwrap());
                self.subcategories.push(subcategory_name.to_string());
                self.subcategory_map
                    .insert(subcategory_name.to_string(), handle);
                handle
            }
        }
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
