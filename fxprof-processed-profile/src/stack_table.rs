use std::hash::{BuildHasher, Hash, Hasher};

use crate::columnar_interner::{ColumnarInterner, ColumnarStore};
use serde::ser::{Serialize, SerializeMap, Serializer};

use crate::{FrameHandle, StackHandle};

/// The stack table stores the tree of stack nodes of a thread. The shape of the tree is encoded in
/// the prefix column: Root stack nodes have null as their prefix, and every non-root stack has the
/// stack index of its "caller" / "parent" as its prefix. Every stack node also has a frame and a
/// category. A "call stack" is a list of frames. Every stack index in the stack table represents
/// such a call stack; the "list of frames" is obtained by walking the path in the tree from the
/// root to the given stack node.
///
/// Stacks are used in the thread's samples; each sample refers to a stack index. Stacks can be
/// shared between samples.
///
/// With this representation, every sample only needs to store a single integer to identify the
/// sample's stack. We take advantage of the fact that many call stacks in the profile have a
/// shared prefix; storing these stacks as a tree saves a lot of space compared to storing them as
/// actual lists of frames.
///
/// The category of a stack node is always non-null and is derived from a stack's frame and its
/// prefix. Frames can have null categories, stacks cannot. If a stack's frame has a null category,
/// the stack inherits the category of its prefix stack. Root stacks whose frame has a null stack
/// have their category set to the "default category". (The default category is currently defined
/// as the category in the profile's category list whose color is "grey", and such a category is
/// required to be present.)
///
/// You could argue that the stack table's category column is derived data and as such doesn't need
/// to be stored in the profile itself. This is true, but storing this information in the stack
/// table makes it a lot easier to carry it through various transforms that we apply to threads.
/// For example, here's a case where a stack's category is not recoverable from any other
/// information in the transformed thread:
///
/// In the call path
///   someJSFunction [JS] -> Node.insertBefore [DOM] -> nsAttrAndChildArray::InsertChildAt,
///
/// the stack node for nsAttrAndChildArray::InsertChildAt should inherit the category DOM from its
/// "Node.insertBefore" prefix stack. And it should keep the DOM category even if you apply the
/// "Merge node into calling function" transform to Node.insertBefore. This transform removes the
/// stack node "Node.insertBefore" from the stackTable, so the information about the DOM category
/// would be lost if it wasn't inherited into the nsAttrAndChildArray::InsertChildAt stack before
/// transforms are applied.
#[derive(Debug, Clone, Default)]
pub struct StackTable {
    set: ColumnarInterner<StackCols, usize>,
}

#[derive(Debug, Clone, Default)]
struct StackCols {
    prefix: Vec<Option<StackHandle>>,
    frame: Vec<FrameHandle>,
}

impl ColumnarStore for StackCols {
    type Row = (Option<StackHandle>, FrameHandle);

    fn len(&self) -> usize {
        self.frame.len()
    }

    fn hash_row<H: BuildHasher>(row: &Self::Row, hasher: &H) -> u64 {
        let mut h = hasher.build_hasher();
        row.0.hash(&mut h);
        row.1.hash(&mut h);
        h.finish()
    }

    fn hash_at<H: BuildHasher>(&self, i: usize, hasher: &H) -> u64 {
        let mut h = hasher.build_hasher();
        self.prefix[i].hash(&mut h);
        self.frame[i].hash(&mut h);
        h.finish()
    }

    fn eq_at(&self, i: usize, row: &Self::Row) -> bool {
        self.prefix[i] == row.0 && self.frame[i] == row.1
    }

    fn push(&mut self, row: Self::Row) {
        self.prefix.push(row.0);
        self.frame.push(row.1);
    }
}

impl StackTable {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn index_for_stack(
        &mut self,
        prefix: Option<StackHandle>,
        frame: FrameHandle,
    ) -> StackHandle {
        StackHandle(self.set.insert((prefix, frame)))
    }

    pub fn len(&self) -> usize {
        self.set.len()
    }

    pub fn into_stacks(self) -> impl Iterator<Item = (Option<StackHandle>, FrameHandle)> {
        let cols = self.set.into_store();
        cols.prefix.into_iter().zip(cols.frame)
    }
}

impl Serialize for StackTable {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let cols = self.set.store();
        let len = self.set.len();
        let mut map = serializer.serialize_map(Some(3))?;
        map.serialize_entry("length", &len)?;
        map.serialize_entry("prefix", &cols.prefix)?;
        map.serialize_entry("frame", &cols.frame)?;
        map.end()
    }
}
