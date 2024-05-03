use std::collections::HashMap;
use std::hash::BuildHasherDefault;

use fxhash::FxHasher;

pub type FastHashMap<K, V> = HashMap<K, V, BuildHasherDefault<FxHasher>>;
