use rustc_hash::FxHashMap;
use indexmap::IndexSet;

pub type FastHashMap<K, V> = FxHashMap<K, V>;
pub type FastIndexSet<V> = IndexSet<V, rustc_hash::FxBuildHasher>;
