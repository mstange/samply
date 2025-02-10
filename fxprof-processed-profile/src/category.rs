use std::hash::Hash;

use indexmap::Equivalent;
use serde::ser::{Serialize, SerializeMap, Serializer};

use super::category_color::CategoryColor;
use super::fast_hash_map::FastIndexSet;

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct Category<'a>(pub &'a str, pub CategoryColor);

/// A profiling category, obtained from [`Profile::handle_for_category`](crate::Profile::handle_for_category).
///
/// Used to categorize stack frames and markers in the front-end. Every category has a color;
/// the color is used in the activity graph and in the call tree, and in a few other places.
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

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct SubcategoryIndex(pub u16);

impl SubcategoryIndex {
    /// The "Other" subcategory. All categories have this subcategory as their first subcategory.
    pub const OTHER: Self = SubcategoryIndex(0);
}

/// A subcategory of a [`CategoryHandle`], used to annotate stack frames.
///
/// Every [`CategoryHandle`] can be turned into a [`SubcategoryHandle`] by calling `.into()` -
/// this will give you the default subcategory of that category.
///
/// Subcategory handles for named subcategories can be obtained from
/// [`Profile::handle_for_subcategory`](crate::Profile::handle_for_subcategory).
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
    subcategories: FastIndexSet<String>,
}

impl Hash for InternalCategory {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.as_category().hash(state)
    }
}

impl Equivalent<Category<'_>> for InternalCategory {
    fn equivalent(&self, key: &Category<'_>) -> bool {
        &self.as_category() == key
    }
}

impl Equivalent<InternalCategory> for Category<'_> {
    fn equivalent(&self, key: &InternalCategory) -> bool {
        self == &key.as_category()
    }
}

impl PartialEq for InternalCategory {
    fn eq(&self, other: &Self) -> bool {
        self.as_category() == other.as_category()
    }
}

impl Eq for InternalCategory {}

impl InternalCategory {
    pub fn new(name: &str, color: CategoryColor) -> Self {
        let mut subcategories = FastIndexSet::default();
        subcategories.insert("Other".to_string());
        Self {
            name: name.to_string(),
            color,
            subcategories,
        }
    }

    /// Get or create a subcategory to this category.
    pub fn index_for_subcategory(&mut self, subcategory_name: &str) -> SubcategoryIndex {
        let index = self
            .subcategories
            .get_index_of(subcategory_name)
            .unwrap_or_else(|| {
                self.subcategories
                    .insert_full(subcategory_name.to_owned())
                    .0
            });
        SubcategoryIndex(u16::try_from(index).unwrap())
    }

    pub fn as_category(&self) -> Category<'_> {
        Category(&self.name, self.color)
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
