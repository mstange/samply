//! A deduplicating container that allows storing the elements in a columnar
//! fashion.
//!
//! This is used by various tables in the profile, e.g. the FuncTable or the
//! NativeSymbols table, where we have the following constraints:
//!
//! - Element deduplication: We only want to store each element (e.g. each
//!   func) once, so we need a fast way to check if a candidate element is
//!   already stored (and at what index).
//! - Columnar storage: In the profile format, these tables are stored
//!   column-by-column, so we'll want the same layout in memory, for fast
//!   serialization.
//! - Low memory usage: The values of the stored element properties should
//!   only be stored in one place (the colunms), not in two places;
//!   specifically, we don't want to duplicate the information in the key of
//!   a `HashMap<ElemKey, ElemIndex>`.
//!
//! Every table which uses this has to provide two types:
//!
//! - a "row" type, e.g. `FuncRow`, and
//! - a "store" type, e.g. `FuncCols`, which implements the [`ColumnarStore`]
//!   trait
//!
//! Then it can create a [`ColumnarInterner`], e.g. `ColumnarInterner<FuncCols>`,
//! and call `interner.insert(row)`.

use std::hash::BuildHasher;

use hashbrown::HashTable;
use rustc_hash::FxBuildHasher;

/// User-provided columnar storage for the rows tracked by a
/// [`ColumnarInterner`].
///
/// Implementors decide how to lay out columns and how to hash and compare
/// rows. In the typical case, [`hash_row`](Self::hash_row) and
/// [`hash_at`](Self::hash_at) must hash the same fields in the same order,
/// and [`eq_at`](Self::eq_at) must compare the same fields — but the trait
/// doesn't require this: you can dedup on a subset of columns and treat the
/// rest as payload if you want "first insertion wins" semantics on those
/// fields.
pub trait ColumnarStore {
    /// The row type that gets pushed into the columns.
    type Row;

    /// Number of rows currently in the store. Must equal the length of each
    /// column and stay in sync with calls to [`push`](Self::push).
    fn len(&self) -> usize;

    /// Hash an incoming (not-yet-stored) row.
    fn hash_row<H: BuildHasher>(row: &Self::Row, hasher: &H) -> u64;

    /// Hash the row currently at column index `index`.
    ///
    /// Must return the same value as `hash_row(&row, hasher)` when
    /// `eq_at(index, &row)` is `true`.
    fn hash_at<H: BuildHasher>(&self, index: usize, hasher: &H) -> u64;

    /// Return whether the row at column index `index` equals `row`.
    fn eq_at(&self, index: usize, row: &Self::Row) -> bool;

    /// Append `row` to the columns. Must extend [`len`](Self::len) by exactly 1.
    fn push(&mut self, row: Self::Row);
}

/// A primitive integer type that can be used as an index into a
/// [`ColumnarInterner`]'s storage.
///
/// This exists to allow using a smaller type than usize, e.g. u32
/// or u16, which saves space for the table of indexes.
pub trait Index: Copy {
    fn from_usize(n: usize) -> Self;
    fn to_usize(self) -> usize;
}

macro_rules! impl_index {
    ($($t:ty),*) => {$(
        impl Index for $t {
            #[inline]
            fn from_usize(n: usize) -> Self {
                <$t>::try_from(n).expect("index does not fit in target primitive type")
            }
            #[inline]
            fn to_usize(self) -> usize { self as usize }
        }
    )*};
}
impl_index!(u32, i32, usize);

/// A deduplicating index set backed by columnar storage.
///
/// Type parameters:
/// - `S`: the columnar storage type; must implement [`ColumnarStore`].
/// - `Idx`: the primitive index type. Defaults to `u32`.
/// - `H`: the [`BuildHasher`]. Defaults to
///   [`rustc_hash::FxBuildHasher`], which is fast and DoS-non-resistant.
///   Swap for [`std::hash::RandomState`] if you need DoS resistance.
pub struct ColumnarInterner<S: ColumnarStore, Idx: Index = u32, H: BuildHasher = FxBuildHasher> {
    store: S,
    table: HashTable<Idx>,
    hasher: H,
}

impl<S, Idx, H> std::fmt::Debug for ColumnarInterner<S, Idx, H>
where
    S: ColumnarStore + std::fmt::Debug,
    Idx: Index,
    H: BuildHasher,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ColumnarInterner")
            .field("store", &self.store)
            .field("len", &self.len())
            .finish_non_exhaustive()
    }
}

impl<S, Idx, H> Clone for ColumnarInterner<S, Idx, H>
where
    S: ColumnarStore + Clone,
    Idx: Index,
    H: BuildHasher + Clone,
{
    fn clone(&self) -> Self {
        Self {
            store: self.store.clone(),
            table: self.table.clone(),
            hasher: self.hasher.clone(),
        }
    }
}

impl<S: ColumnarStore + Default, Idx: Index> Default for ColumnarInterner<S, Idx, FxBuildHasher> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S: ColumnarStore + Default, Idx: Index> ColumnarInterner<S, Idx, FxBuildHasher> {
    /// Create an empty interner with default storage and the default hasher.
    pub fn new() -> Self {
        Self {
            store: S::default(),
            table: HashTable::new(),
            hasher: FxBuildHasher,
        }
    }
}

impl<S: ColumnarStore, Idx: Index, H: BuildHasher> ColumnarInterner<S, Idx, H> {
    /// Number of unique rows.
    pub fn len(&self) -> usize {
        self.table.len()
    }

    /// Immutable access to the columnar storage.
    pub fn store(&self) -> &S {
        &self.store
    }

    /// Consume the interner and return the columnar storage.
    ///
    /// Common when you want the columns without paying to keep the hash
    /// table around (e.g. before serialization).
    pub fn into_store(self) -> S {
        self.store
    }

    /// Insert `row`, deduplicating against existing rows. Returns the
    /// existing index if `row` compares equal to a stored row, or the
    /// newly-assigned index otherwise.
    pub fn insert(&mut self, row: S::Row) -> Idx {
        let hash = S::hash_row(&row, &self.hasher);

        {
            let store = &self.store;
            if let Some(&idx) = self.table.find(hash, |&i| store.eq_at(i.to_usize(), &row)) {
                return idx;
            }
        }

        let new_idx = Idx::from_usize(self.store.len());
        self.store.push(row);

        let store = &self.store;
        let hasher = &self.hasher;
        self.table
            .insert_unique(hash, new_idx, |&i| store.hash_at(i.to_usize(), hasher));
        new_idx
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::hash::{Hash, Hasher};

    #[derive(Clone, Copy)]
    struct Symbol {
        name_index: i32,
        lib_index: i32,
        address: i32,
    }

    #[derive(Default)]
    struct SymbolCols {
        name_index: Vec<i32>,
        lib_index: Vec<i32>,
        address: Vec<i32>,
    }

    impl ColumnarStore for SymbolCols {
        type Row = Symbol;
        fn len(&self) -> usize {
            self.name_index.len()
        }
        fn hash_row<H: BuildHasher>(row: &Symbol, hasher: &H) -> u64 {
            let mut h = hasher.build_hasher();
            row.name_index.hash(&mut h);
            row.lib_index.hash(&mut h);
            row.address.hash(&mut h);
            h.finish()
        }
        fn hash_at<H: BuildHasher>(&self, i: usize, hasher: &H) -> u64 {
            let mut h = hasher.build_hasher();
            self.name_index[i].hash(&mut h);
            self.lib_index[i].hash(&mut h);
            self.address[i].hash(&mut h);
            h.finish()
        }
        fn eq_at(&self, i: usize, row: &Symbol) -> bool {
            self.name_index[i] == row.name_index
                && self.lib_index[i] == row.lib_index
                && self.address[i] == row.address
        }
        fn push(&mut self, row: Symbol) {
            self.name_index.push(row.name_index);
            self.lib_index.push(row.lib_index);
            self.address.push(row.address);
        }
    }

    #[test]
    fn dedup_and_index() {
        let mut set: ColumnarInterner<SymbolCols> = ColumnarInterner::new();
        let a = set.insert(Symbol {
            name_index: 7,
            lib_index: 1,
            address: 0x100,
        });
        let b = set.insert(Symbol {
            name_index: 8,
            lib_index: 1,
            address: 0x200,
        });
        let a2 = set.insert(Symbol {
            name_index: 7,
            lib_index: 1,
            address: 0x100,
        });
        assert_eq!(a, a2);
        assert_ne!(a, b);
        assert_eq!(set.len(), 2);
        assert_eq!(set.store().name_index, vec![7, 8]);
    }

    #[test]
    fn stress_many_rows() {
        // Push enough rows to force at least one hash-table resize.
        let mut set: ColumnarInterner<SymbolCols> = ColumnarInterner::new();
        for i in 0..10_000i32 {
            let idx = set.insert(Symbol {
                name_index: i,
                lib_index: i / 100,
                address: i * 4,
            });
            assert_eq!(idx as i32, i);
        }
        for i in 0..10_000i32 {
            let idx = set.insert(Symbol {
                name_index: i,
                lib_index: i / 100,
                address: i * 4,
            });
            assert_eq!(idx as i32, i);
        }
        assert_eq!(set.len(), 10_000);
    }

    #[test]
    fn usize_index() {
        let mut set: ColumnarInterner<SymbolCols, usize> = ColumnarInterner::new();
        let a: usize = set.insert(Symbol {
            name_index: 1,
            lib_index: 0,
            address: 0,
        });
        let a2: usize = set.insert(Symbol {
            name_index: 1,
            lib_index: 0,
            address: 0,
        });
        assert_eq!(a, a2);
        assert_eq!(a, 0);
    }
}
