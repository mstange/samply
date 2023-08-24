use std::cmp::Ordering;
use std::collections::BinaryHeap;

struct EventHeapItem<G: Clone + Ord, K: Ord, V> {
    group: G,
    round: usize,
    key: K,
    value: V,
}

impl<G: Clone + Ord, K: Ord, V> PartialEq<Self> for EventHeapItem<G, K, V> {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl<G: Clone + Ord, K: Ord, V> Eq for EventHeapItem<G, K, V> {}

impl<G: Clone + Ord, K: Ord, V> Ord for EventHeapItem<G, K, V> {
    fn cmp(&self, other: &Self) -> Ordering {
        // Invert order to make BinaryHeap min-heap.
        self.key.cmp(&other.key).reverse()
    }
}

impl<G: Clone + Ord, K: Ord, V> PartialOrd<Self> for EventHeapItem<G, K, V> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// An event sorter for correctly ordering events from multiple perf ring buffers.
///
/// As we are reading events from ring buffers in bulk, concatenating the events does not yield
/// the correct ordering across different buffers, hence sorting is required.
///
/// `EventSorter` is an incremental sorter: we buffer events from each ring buffer, keeping them
/// sorted inside the buffer. Once we are sure that nothing earlier than the first event in the
/// buffer can arrive, we release it for consumption. For an event read from group N, such release
/// happens when we have issued reads for all groups != N afterwards. For the implementation, we
/// additionally assume that reads are performed in a round-robin manner.
///
/// `EventSorter` has 3 generic parameters:
/// - `G` is the type of the group identifier. Each "group" represents a ring buffer, and `G` is
///   an ordered identifier for the group.
/// - `K` is the type of the key used for sorting. It is usually a timestamp.
/// - `V` is the type of consumed events.
pub struct EventSorter<G: Clone + Ord, K: Ord, V> {
    heap: BinaryHeap<EventHeapItem<G, K, V>>,
    round: usize,
    current_group: Option<G>,
}

impl<G: Clone + Ord, K: Ord, V> EventSorter<G, K, V> {
    pub fn new() -> Self {
        EventSorter {
            heap: BinaryHeap::new(),
            round: 0,
            current_group: None,
        }
    }

    /// Check if there are buffered events.
    ///
    /// Even when [`EventSorter::pop`] returns `None`, there may still be events buffered that will
    /// be released when additional rounds are read.
    ///
    /// This helper allows to check if additional rounds should be read before going into a wait
    /// state.
    pub fn has_more(&self) -> bool {
        !self.heap.is_empty()
    }

    /// Begin a new round.
    ///
    /// This should be called when the group with the largest identifier has been read and returning
    /// to the group with the smallest identifier.
    pub fn advance_round(&mut self) {
        self.round += 1;
        self.current_group = None;
    }

    /// Begin reading events from a new group.
    ///
    /// **Panics**: If `group` is not monotonically increasing within the same round.
    pub fn begin_group(&mut self, group: G) {
        assert!(
            Some(&group) >= self.current_group.as_ref(),
            "Group keys must be monotonically increasing"
        );
        self.current_group = Some(group);
    }

    /// Try to consume an event from the sorter.
    pub fn pop(&mut self) -> Option<V> {
        let event = self.heap.peek()?;
        if (event.round + 1, Some(&event.group)) > (self.round, self.current_group.as_ref()) {
            return None;
        }
        self.heap.pop().map(|x| x.value)
    }
}

impl<G: Clone + Ord, K: Ord, V> Extend<(K, V)> for EventSorter<G, K, V> {
    fn extend<I: IntoIterator<Item = (K, V)>>(&mut self, iter: I) {
        self.heap.extend(iter.into_iter().map(|(seq, value)| {
            EventHeapItem {
                group: self
                    .current_group
                    .clone()
                    .expect("begin_group must be called before insertion"),
                round: self.round,
                key: seq,
                value,
            }
        }));
    }
}
