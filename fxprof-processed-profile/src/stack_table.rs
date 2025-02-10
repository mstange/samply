use serde::ser::{Serialize, SerializeMap, Serializer};

use crate::fast_hash_map::FastHashMap;

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
    stack_prefixes: Vec<Option<usize>>,
    stack_frames: Vec<usize>,

    // (parent stack, frame_index) -> stack index
    index: FastHashMap<(Option<usize>, usize), usize>,
}

impl StackTable {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn index_for_stack(&mut self, prefix: Option<usize>, frame: usize) -> usize {
        match self.index.get(&(prefix, frame)) {
            Some(stack) => *stack,
            None => {
                let stack = self.stack_prefixes.len();
                self.stack_prefixes.push(prefix);
                self.stack_frames.push(frame);
                self.index.insert((prefix, frame), stack);
                stack
            }
        }
    }
}

impl Serialize for StackTable {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let len = self.stack_prefixes.len();
        let mut map = serializer.serialize_map(Some(3))?;
        map.serialize_entry("length", &len)?;
        map.serialize_entry("prefix", &self.stack_prefixes)?;
        map.serialize_entry("frame", &self.stack_frames)?;
        map.end()
    }
}
