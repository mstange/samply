use std::hash::Hash;

use indexmap::Equivalent;
use serde::ser::{Serialize, SerializeMap, Serializer};

use super::category_color::CategoryColor;
use super::fast_hash_map::FastIndexSet;
use crate::Profile;

/// Implemented by [`Category`], [`Subcategory`], [`CategoryHandle`] and [`SubcategoryHandle`].
pub trait IntoSubcategoryHandle {
    /// Returns the corresponding [`SubcategoryHandle`].
    fn into_subcategory_handle(self, profile: &mut Profile) -> SubcategoryHandle;
}

/// A profiling category. Has a name and a color.
///
/// Used to categorize stack frames and markers in the front-end. The category's
/// color is used in the activity graph and in the call tree, and in a few other places.
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct Category<'a>(pub &'a str, pub CategoryColor);

impl Category<'_> {
    /// The "Other" category. All profiles have this category.
    pub const OTHER: Category<'static> = Category("Other", CategoryColor::Gray);
}

impl IntoSubcategoryHandle for Category<'_> {
    fn into_subcategory_handle(self, profile: &mut Profile) -> SubcategoryHandle {
        let category_handle = profile.handle_for_category(self);
        category_handle.into()
    }
}

/// The handle for a [`Category`], obtained from [`Profile::handle_for_category`](crate::Profile::handle_for_category).
///
/// Storing and reusing the handle avoids repeated lookups and can improve performance.
///
/// The handle is specific to a [`Profile`] instance and cannot be reused across profiles.
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct CategoryHandle(pub(crate) u16);

impl CategoryHandle {
    /// The "Other" category. All profiles have this category.
    pub const OTHER: Self = CategoryHandle(0);
}

impl IntoSubcategoryHandle for CategoryHandle {
    fn into_subcategory_handle(self, _profile: &mut Profile) -> SubcategoryHandle {
        self.into()
    }
}

impl Serialize for CategoryHandle {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

/// A named subcategory of a [`Category`], for fine-grained annotation of stack frames.
///
/// If you don't need named subcategories, you can just pass a [`Category`] or a
/// [`CategoryHandle`] in any place where an [`IntoSubcategoryHandle`] is expected;
/// this will give you the category's default subcategory.
pub struct Subcategory<'a>(pub Category<'a>, pub &'a str);

impl IntoSubcategoryHandle for Subcategory<'_> {
    fn into_subcategory_handle(self, profile: &mut Profile) -> SubcategoryHandle {
        let Subcategory(category, subcategory_name) = self;
        let category_handle = profile.handle_for_category(category);
        profile.handle_for_subcategory(category_handle, subcategory_name)
    }
}

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct SubcategoryIndex(pub u16);

impl SubcategoryIndex {
    /// The "Other" subcategory. All categories have this subcategory as their first subcategory.
    pub const OTHER: Self = SubcategoryIndex(0);
}

/// A handle for a [`Subcategory`], or for the default subcategory of a [`CategoryHandle`].
///
/// Used to annotate stack frames.
///
/// Every [`CategoryHandle`] can be turned into a [`SubcategoryHandle`] by calling `.into()` -
/// this will give you the default subcategory of that category.
///
/// Subcategory handles for named subcategories can be obtained from
/// [`Profile::handle_for_subcategory`](crate::Profile::handle_for_subcategory).
/// Storing and reusing the handle avoids repeated lookups and can improve performance.
///
/// The handle is specific to a Profile instance and cannot be reused across profiles.
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct SubcategoryHandle(pub(crate) CategoryHandle, pub(crate) SubcategoryIndex);

impl IntoSubcategoryHandle for SubcategoryHandle {
    fn into_subcategory_handle(self, _profile: &mut Profile) -> SubcategoryHandle {
        self
    }
}

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
