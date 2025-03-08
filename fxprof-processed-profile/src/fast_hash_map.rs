use indexmap::IndexSet;
use rustc_hash::FxHashMap;

pub type FastHashMap<K, V> = FxHashMap<K, V>;
pub type FastIndexSet<V> = IndexSet<V, rustc_hash::FxBuildHasher>;
