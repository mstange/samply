use std::hash::BuildHasherDefault;

use fxhash::FxHasher;

pub type FastHashMap<K, V> = hashbrown::HashMap<K, V, BuildHasherDefault<FxHasher>>;
