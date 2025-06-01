use indexmap::IndexSet;
use rustc_hash::{FxHashMap, FxHashSet};

pub type FastHashMap<K, V> = FxHashMap<K, V>;
pub type FastHashSet<V> = FxHashSet<V>;
pub type FastIndexSet<V> = IndexSet<V, rustc_hash::FxBuildHasher>;
