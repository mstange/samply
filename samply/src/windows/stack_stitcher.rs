//! Reassembly of the kernel/user stack fragments that ETW delivers separately.
//!
//! On Windows, a stack-bearing event (a profiler sample, or a marker) is
//! recorded first, and its stack arrives *later* as one or more `StackWalk`
//! fragments:
//!
//!  - Zero or more **kernel** fragments (the stack walk was still in kernel
//!    mode, so the outermost / root frame is a kernel address), followed by
//!  - one **user** fragment, whose root frame is a user address. This means the
//!    walk reached user space, so it is the *terminal* fragment. For *samples*
//!    the user portion applies to this sample **and to every preceding pending
//!    sample on the same thread** whose timestamp is `<=` the user fragment's
//!    timestamp (while a thread runs in the kernel — e.g. inside a syscall —
//!    several kernel-only samples can be collected before control returns to
//!    user mode and the user stack is captured). Markers do not share this way;
//!    see [`StackAssociation`] for why the two differ.
//!
//! Some requests never receive a user fragment (the System process never runs
//! user code; a marker's walk can stop at the kernel/user boundary; and the trace
//! can simply end) — those are finalized with whatever kernel frames were
//! collected, or with no stack at all.
//!
//! This module deliberately knows *nothing* about what a stack is for. The
//! caller registers a request with an opaque payload `P` (a sample descriptor, a
//! marker handle, ...) and gets that same payload back, paired with the stitched
//! stack, once the stack is complete. That keeps the (fiddly, and historically
//! buggy) stitching logic in one place, independent of samples vs. markers, and
//! unit-testable without any ETW or `Profile` machinery.
//!
//! ## Fragment ordering
//!
//! Within every fragment the frames are **leaf-first**: index 0 is the
//! instruction pointer (innermost), and the last frame is the outermost (root).
//! A complete stack therefore looks like `[kernel leaf .. kernel] [user .. user
//! root]`, i.e. a single kernel→user transition. We classify a fragment by its
//! *root* frame (not its leaf): a user root means "terminal", a kernel root
//! means "partial, more to come". A fragment that contains both (kernel leaf,
//! user root) is a complete stack delivered in one event — common on some
//! configurations — and we split it at the boundary.

use std::collections::VecDeque;

use crate::shared::types::{FastHashMap, StackFrame, StackMode};

/// The two halves of a reassembled stack, each **leaf-first** (innermost frame
/// first). Either half may be empty (a pure-user sample has no kernel frames; a
/// flushed System-process sample has no user frames).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StitchedStack {
    pub kernel: Vec<StackFrame>,
    pub user: Vec<StackFrame>,
}

/// How a request is matched to its stack fragments. This is the one place the
/// difference between "a sampling-profiler sample" and "a one-off instrumented
/// event" shows up — expressed as matching behavior, not as event type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackAssociation {
    /// The request shares the thread's *deferred* user stack: when a thread is
    /// sampled in kernel mode, the kernel stack is captured immediately but the
    /// user stack is deferred until the thread returns to user mode, and that one
    /// user stack then applies to every such request still pending on the thread.
    /// This is how the sampling profiler works.
    DeferredUserStack,
    /// The request is matched only by stack fragments carrying its *own*
    /// timestamp, and never borrows a user stack from another event. Used for
    /// one-off instrumented events (markers).
    ///
    /// The consequence is that a marker whose stack walk stopped in the kernel
    /// keeps a kernel-only stack, even when a user stack for some *later* event on
    /// the same thread is sitting right there. That is deliberate. A kernel-only
    /// sample is different: ETW queues a deferred capture for it and walks the
    /// user stack at the moment the thread crosses back to user mode, before it
    /// executes a single user instruction, so that one stack is genuinely valid
    /// for the whole preceding kernel residency — which is what makes
    /// [`Self::DeferredUserStack`] sound. Markers get no such deferral. ETW simply
    /// truncates the walk at the kernel/user boundary and delivers nothing further,
    /// so anything we attached would be a different event's walk taken at a
    /// different time, after the thread had run user code.
    ///
    /// Measured on a 63s Speedometer trace recorded with `-stackwalk DiskIo+FileIo`:
    /// 18% of marker stack walks (9009 of 49682) stop in the kernel like this, and
    /// for 4019 of them a later user fragment exists that could be borrowed. Doing
    /// so produced stacks that are not merely stale but impossible — e.g. a kernel
    /// stack executing `NtSetInformationFile` below `KiSystemServiceCopyEnd`, with
    /// `ntdll!RtlLeaveCriticalSection` spliced on above it as the supposed caller,
    /// because the thread had returned from the rename and moved on to freeing
    /// memory. Since the kernel frames are prefixed onto the user frames, the
    /// stitched result asserts a call that never happened. A truncated stack tells
    /// you less, but everything it does tell you is true.
    OwnTimestamp,
}

/// A request whose stack is now known, ready to be handed to the consumer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finalized<P> {
    /// The timestamp of the original event (not of the user stack that
    /// finalized it).
    pub timestamp: u64,
    /// The payload supplied to [`StackStitcher::request_stack`], or `None` for an
    /// "orphan" stack: a terminal fragment that matched no pending request. This
    /// happens when stacks stand alone with no preceding sample event (e.g. ARM64
    /// guests where `PROFILE` events are unavailable). The caller decides what to
    /// do with an orphan (typically: synthesize a standalone sample).
    pub payload: Option<P>,
    pub stack: StitchedStack,
}

struct PendingRequest<P> {
    timestamp: u64,
    payload: P,
    /// Kernel frames collected so far (leaf-first), accumulated across one or
    /// more kernel fragments.
    kernel: Vec<StackFrame>,
}

struct ThreadPending<P> {
    /// [`StackAssociation::DeferredUserStack`] requests awaiting their stack, in
    /// arrival order. Timestamps are assumed non-decreasing (ETW delivers events
    /// roughly in time order), which lets us finalize a contiguous prefix when a
    /// user stack arrives.
    deferred: VecDeque<PendingRequest<P>>,
    /// [`StackAssociation::OwnTimestamp`] requests, keyed by exact timestamp for
    /// O(1) match. Fragments only ever join a request here by matching its
    /// timestamp exactly, so an entry accumulates whatever ETW delivered for that
    /// one event and nothing else. Entries leave when their user fragment arrives,
    /// or at [`StackStitcher::flush_all`] — kernel-only if a kernel fragment was
    /// collected, dropped entirely if the event never had a stack walk at all
    /// (which is most unknown events). Keeping them in a map rather than a scanned
    /// list keeps the stackless majority off the hot path. If two such events share
    /// an exact (tid, timestamp), the later one wins the stack.
    own_ts: FastHashMap<u64, PendingRequest<P>>,
}

impl<P> Default for ThreadPending<P> {
    fn default() -> Self {
        Self {
            deferred: VecDeque::new(),
            own_ts: FastHashMap::default(),
        }
    }
}

/// Reassembles ETW stack fragments. Generic over the payload `P` carried for
/// each pending request.
pub struct StackStitcher<P> {
    per_thread: FastHashMap<u32, ThreadPending<P>>,
}

impl<P> Default for StackStitcher<P> {
    fn default() -> Self {
        Self {
            per_thread: FastHashMap::default(),
        }
    }
}

impl<P> StackStitcher<P> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register that an event on `tid` at `timestamp` wants a stack. The stack
    /// will be delivered later via [`Self::on_fragment`]. `assoc` controls how the
    /// request is matched (see [`StackAssociation`]).
    pub fn request_stack(
        &mut self,
        tid: u32,
        timestamp: u64,
        assoc: StackAssociation,
        payload: P,
    ) {
        let thread = self.per_thread.entry(tid).or_default();
        let request = PendingRequest {
            timestamp,
            payload,
            kernel: Vec::new(),
        };
        match assoc {
            StackAssociation::DeferredUserStack => thread.deferred.push_back(request),
            StackAssociation::OwnTimestamp => {
                thread.own_ts.insert(timestamp, request);
            }
        }
    }

    /// Feed a raw `StackWalk` fragment (frames **leaf-first**). Returns the
    /// requests that this fragment completes — none for a partial kernel
    /// fragment, one or more for a terminal user fragment (the matched request
    /// plus any preceding pending requests on the thread).
    pub fn on_fragment(
        &mut self,
        tid: u32,
        timestamp: u64,
        frames: Vec<StackFrame>,
    ) -> Vec<Finalized<P>> {
        if frames.is_empty() {
            return Vec::new();
        }

        if frames.last().unwrap().stack_mode() == Some(StackMode::User) {
            self.on_user_fragment(tid, timestamp, &frames)
        } else {
            self.on_kernel_fragment(tid, timestamp, frames);
            Vec::new()
        }
    }

    fn on_kernel_fragment(&mut self, tid: u32, timestamp: u64, frames: Vec<StackFrame>) {
        let Some(thread) = self.per_thread.get_mut(&tid) else {
            return;
        };
        // A kernel fragment belongs to one event. Prefer a deferred (sample)
        // request at this exact timestamp — this preserves the sample path
        // exactly — and only fall back to an own-timestamp (marker) request.
        if let Some(req) = thread
            .deferred
            .iter_mut()
            .rev()
            .find(|r| r.timestamp == timestamp)
        {
            req.kernel.extend_from_slice(&frames);
        } else if let Some(req) = thread.own_ts.get_mut(&timestamp) {
            // A marker that fired in kernel mode. It stays keyed by its own
            // timestamp, so any further fragments for the same event (a second
            // kernel fragment from KeUserModeCallback, or the user fragment) find
            // it, and nothing else can.
            req.kernel.extend_from_slice(&frames);
        }
        // Otherwise: a stack for an event we never saw (or already finalized). Drop it.
    }

    fn on_user_fragment(
        &mut self,
        tid: u32,
        timestamp: u64,
        frames: &[StackFrame],
    ) -> Vec<Finalized<P>> {
        // Split the fragment at its kernel→user boundary. Leading kernel frames
        // (if any) belong to the event sampled at `timestamp`; the user frames
        // are the (shared) user portion.
        let boundary = frames
            .iter()
            .position(|f| f.stack_mode() == Some(StackMode::User))
            .unwrap_or(0);
        let (leaf_kernel, user) = frames.split_at(boundary);

        let Some(thread) = self.per_thread.get_mut(&tid) else {
            // Orphan: a terminal stack with no pending requests at all.
            return vec![orphan(timestamp, leaf_kernel, user)];
        };

        let mut out = Vec::new();

        // A marker at this exact timestamp: this fragment is that event's own user
        // stack, so finalize it. A marker is *only* ever finalized this way. It
        // never borrows a user stack from a later event, even though that would
        // often be the only way to give it one — see the note below.
        if let Some(mut req) = thread.own_ts.remove(&timestamp) {
            req.kernel.extend_from_slice(leaf_kernel);
            out.push(Finalized {
                timestamp: req.timestamp,
                payload: Some(req.payload),
                stack: StitchedStack {
                    kernel: req.kernel,
                    user: user.to_vec(),
                },
            });
        }

        // Deferred (sample) requests: this user stack finalizes every one with a
        // timestamp <= ours (the deferred-user-stack sharing).
        let n = thread
            .deferred
            .iter()
            .take_while(|r| r.timestamp <= timestamp)
            .count();
        for mut req in thread.deferred.drain(..n) {
            // The fragment's own leaf kernel frames belong only to the event
            // sampled at the same timestamp; earlier samples keep just their own
            // accumulated kernel frames and borrow this user stack.
            if req.timestamp == timestamp && !leaf_kernel.is_empty() {
                req.kernel.extend_from_slice(leaf_kernel);
            }
            out.push(Finalized {
                timestamp: req.timestamp,
                payload: Some(req.payload),
                stack: StitchedStack {
                    kernel: req.kernel,
                    user: user.to_vec(),
                },
            });
        }

        if out.is_empty() {
            // Orphan: nothing matched (e.g. ARM64 guest with no PROFILE events).
            return vec![orphan(timestamp, leaf_kernel, user)];
        }
        out
    }

    /// Finalize the most recent deferred (sample) request at `timestamp` on `tid`
    /// with a kernel-only stack. Used for the System process (pid 4), whose
    /// samples never receive a user stack, so they can be emitted as soon as their
    /// kernel fragment arrives.
    pub fn finalize_kernel_only(&mut self, tid: u32, timestamp: u64) -> Option<Finalized<P>> {
        let thread = self.per_thread.get_mut(&tid)?;
        let pos = thread
            .deferred
            .iter()
            .rposition(|r| r.timestamp == timestamp)?;
        let req = thread.deferred.remove(pos)?;
        Some(Finalized {
            timestamp: req.timestamp,
            payload: Some(req.payload),
            stack: StitchedStack {
                kernel: req.kernel,
                user: Vec::new(),
            },
        })
    }

    /// Emit remaining pending requests (across all threads) with the kernel frames
    /// collected so far and no user stack. Call this at end of trace so in-flight
    /// work isn't lost. Deferred (sample) requests are always emitted (an off-cpu
    /// sample with no stack still represents real time); own-timestamp (marker)
    /// requests are emitted only if a kernel fragment was actually collected, so
    /// stackless markers don't get a spurious empty stack. Returns `(tid,
    /// finalized)` pairs.
    pub fn flush_all(&mut self) -> Vec<(u32, Finalized<P>)> {
        let mut out = Vec::new();
        for (tid, thread) in self.per_thread.drain() {
            for req in thread.deferred {
                out.push((tid, kernel_only(req)));
            }
            // Markers that fired in kernel mode and never got a user fragment:
            // keep the kernel-only stack. The rest of `own_ts` is stackless
            // markers, which get no stack at all rather than an empty one.
            for req in thread.own_ts.into_values() {
                if !req.kernel.is_empty() {
                    out.push((tid, kernel_only(req)));
                }
            }
        }
        out
    }
}

fn orphan<P>(timestamp: u64, leaf_kernel: &[StackFrame], user: &[StackFrame]) -> Finalized<P> {
    Finalized {
        timestamp,
        payload: None,
        stack: StitchedStack {
            kernel: leaf_kernel.to_vec(),
            user: user.to_vec(),
        },
    }
}

fn kernel_only<P>(req: PendingRequest<P>) -> Finalized<P> {
    Finalized {
        timestamp: req.timestamp,
        payload: Some(req.payload),
        stack: StitchedStack {
            kernel: req.kernel,
            user: Vec::new(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(addr: u64) -> StackFrame {
        StackFrame::ReturnAddress(addr, StackMode::Kernel)
    }
    fn u(addr: u64) -> StackFrame {
        StackFrame::ReturnAddress(addr, StackMode::User)
    }

    use StackAssociation::{DeferredUserStack, OwnTimestamp};

    #[test]
    fn pure_user_sample() {
        let mut s: StackStitcher<u32> = StackStitcher::new();
        s.request_stack(1, 100, DeferredUserStack, 42);
        let out = s.on_fragment(1, 100, vec![u(0x10), u(0x20)]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].payload, Some(42));
        assert_eq!(out[0].timestamp, 100);
        assert!(out[0].stack.kernel.is_empty());
        assert_eq!(out[0].stack.user, vec![u(0x10), u(0x20)]);
    }

    #[test]
    fn kernel_then_user_fragment() {
        let mut s: StackStitcher<u32> = StackStitcher::new();
        s.request_stack(1, 100, DeferredUserStack, 42);
        // Partial kernel fragment arrives first: finalizes nothing.
        assert!(s.on_fragment(1, 100, vec![k(0xf1), k(0xf2)]).is_empty());
        // User fragment finalizes it.
        let out = s.on_fragment(1, 100, vec![u(0x10), u(0x20)]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].stack.kernel, vec![k(0xf1), k(0xf2)]);
        assert_eq!(out[0].stack.user, vec![u(0x10), u(0x20)]);
    }

    #[test]
    fn user_stack_applies_to_preceding_kernel_samples() {
        let mut s: StackStitcher<u32> = StackStitcher::new();
        // Three samples taken while in the kernel, each with its own kernel stack.
        s.request_stack(1, 100, DeferredUserStack, 1);
        s.on_fragment(1, 100, vec![k(0xa)]);
        s.request_stack(1, 200, DeferredUserStack, 2);
        s.on_fragment(1, 200, vec![k(0xb)]);
        s.request_stack(1, 300, DeferredUserStack, 3);
        s.on_fragment(1, 300, vec![k(0xc)]);
        // Control returns to user mode; one user stack finalizes all three.
        let out = s.on_fragment(1, 300, vec![u(0x10)]);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].payload, Some(1));
        assert_eq!(out[0].stack.kernel, vec![k(0xa)]);
        assert_eq!(out[0].stack.user, vec![u(0x10)]);
        assert_eq!(out[2].payload, Some(3));
        assert_eq!(out[2].stack.kernel, vec![k(0xc)]);
        assert_eq!(out[2].stack.user, vec![u(0x10)]);
    }

    #[test]
    fn complete_fragment_in_one_event() {
        // Kernel leaf + user root delivered as a single fragment, with a matching
        // request. This is the case samply previously mis-routed (it classified by
        // the leaf and treated the whole thing as a pending kernel stack).
        let mut s: StackStitcher<u32> = StackStitcher::new();
        s.request_stack(1, 100, DeferredUserStack, 42);
        let out = s.on_fragment(1, 100, vec![k(0xf1), k(0xf2), u(0x10), u(0x20)]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].stack.kernel, vec![k(0xf1), k(0xf2)]);
        assert_eq!(out[0].stack.user, vec![u(0x10), u(0x20)]);
    }

    #[test]
    fn orphan_stack_without_request() {
        // No request registered (e.g. ARM64 guest with no PROFILE events).
        let mut s: StackStitcher<u32> = StackStitcher::new();
        let out = s.on_fragment(1, 100, vec![k(0xf1), u(0x10), u(0x20)]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].payload, None);
        assert_eq!(out[0].stack.kernel, vec![k(0xf1)]);
        assert_eq!(out[0].stack.user, vec![u(0x10), u(0x20)]);
    }

    #[test]
    fn multiple_kernel_fragments_concatenate() {
        // KeUserModeCallback can produce more than one kernel fragment before the
        // user stack.
        let mut s: StackStitcher<u32> = StackStitcher::new();
        s.request_stack(1, 100, DeferredUserStack, 42);
        s.on_fragment(1, 100, vec![k(0xf1), k(0xf2)]);
        s.on_fragment(1, 100, vec![k(0xf3)]);
        let out = s.on_fragment(1, 100, vec![u(0x10)]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].stack.kernel, vec![k(0xf1), k(0xf2), k(0xf3)]);
        assert_eq!(out[0].stack.user, vec![u(0x10)]);
    }

    #[test]
    fn kernel_only_finalize_for_system_process() {
        let mut s: StackStitcher<u32> = StackStitcher::new();
        s.request_stack(4, 100, DeferredUserStack, 42);
        assert!(s.on_fragment(4, 100, vec![k(0xf1), k(0xf2)]).is_empty());
        let out = s.finalize_kernel_only(4, 100).unwrap();
        assert_eq!(out.payload, Some(42));
        assert_eq!(out.stack.kernel, vec![k(0xf1), k(0xf2)]);
        assert!(out.stack.user.is_empty());
    }

    #[test]
    fn flush_emits_leftovers_kernel_only() {
        let mut s: StackStitcher<u32> = StackStitcher::new();
        s.request_stack(1, 100, DeferredUserStack, 1);
        s.on_fragment(1, 100, vec![k(0xa)]);
        s.request_stack(2, 150, DeferredUserStack, 2); // never got any fragment
        let mut out = s.flush_all();
        out.sort_by_key(|(_tid, f)| f.timestamp);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].1.payload, Some(1));
        assert_eq!(out[0].1.stack.kernel, vec![k(0xa)]);
        assert_eq!(out[1].1.payload, Some(2));
        assert!(out[1].1.stack.kernel.is_empty());
        assert!(out[1].1.stack.user.is_empty());
    }

    #[test]
    fn stack_for_unknown_event_is_dropped() {
        let mut s: StackStitcher<u32> = StackStitcher::new();
        // Kernel fragment with no matching request: dropped, nothing finalized.
        s.on_fragment(1, 100, vec![k(0xa)]);
        assert!(s.flush_all().is_empty());
    }

    // --- OwnTimestamp (marker) association ---

    #[test]
    fn marker_gets_its_own_user_stack() {
        // The common case: a marker that fired in user mode gets a complete user
        // stack at its own timestamp.
        let mut s: StackStitcher<u32> = StackStitcher::new();
        s.request_stack(1, 100, OwnTimestamp, 7);
        let out = s.on_fragment(1, 100, vec![u(0x10), u(0x20)]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].payload, Some(7));
        assert_eq!(out[0].stack.user, vec![u(0x10), u(0x20)]);
    }

    #[test]
    fn stackless_marker_is_not_swept_by_a_later_user_stack() {
        // A stackless marker (an unknown event with no stackwalk) must NOT absorb a
        // nearby sample's deferred user stack: it never received a kernel fragment,
        // so it stays in `own_ts` and is dropped at flush.
        let mut s: StackStitcher<u32> = StackStitcher::new();
        s.request_stack(1, 100, OwnTimestamp, 7); // marker, no fragment will arrive
        s.request_stack(1, 200, DeferredUserStack, 42); // sample
        s.on_fragment(1, 200, vec![k(0xa)]); // sample's kernel stack
        let out = s.on_fragment(1, 300, vec![u(0x10)]); // later user stack
        // Only the sample is finalized; the marker is left untouched.
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].payload, Some(42));
        // The stackless marker is dropped at flush (no spurious empty stack).
        assert!(s.flush_all().is_empty());
    }

    #[test]
    fn kernel_marker_does_not_borrow_a_later_user_stack() {
        // A marker that fired in kernel mode does NOT take the user stack of a
        // later event on the same thread; that stack describes different code at a
        // different time. It keeps its kernel-only stack. See `OwnTimestamp`.
        let mut s: StackStitcher<u32> = StackStitcher::new();
        s.request_stack(1, 100, OwnTimestamp, 7);
        s.on_fragment(1, 100, vec![k(0xf1), k(0xf2)]); // marker's kernel stack
        // A later user fragment belonging to some other event must not claim it.
        // Here nothing else is pending, so it comes back as an orphan.
        let later = s.on_fragment(1, 150, vec![u(0x10), u(0x20)]);
        assert_eq!(later.len(), 1);
        assert_eq!(later[0].payload, None);

        let out = s.flush_all();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].1.payload, Some(7));
        assert_eq!(out[0].1.stack.kernel, vec![k(0xf1), k(0xf2)]);
        assert!(out[0].1.stack.user.is_empty());
    }

    #[test]
    fn a_samples_user_stack_does_not_leak_into_a_kernel_marker() {
        // The realistic shape of the above: a marker fires in kernel mode, then a
        // sample on the same thread gets its own deferred user stack. The sample is
        // finalized with it; the marker is not touched.
        let mut s: StackStitcher<u32> = StackStitcher::new();
        s.request_stack(1, 100, OwnTimestamp, 7); // marker
        s.on_fragment(1, 100, vec![k(0xf1)]);
        s.request_stack(1, 200, DeferredUserStack, 42); // sample
        s.on_fragment(1, 200, vec![k(0xa)]);
        let out = s.on_fragment(1, 200, vec![u(0x10)]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].payload, Some(42));
        assert_eq!(out[0].stack.user, vec![u(0x10)]);

        let flushed = s.flush_all();
        assert_eq!(flushed.len(), 1);
        assert_eq!(flushed[0].1.payload, Some(7));
        assert_eq!(flushed[0].1.stack.kernel, vec![k(0xf1)]);
        assert!(flushed[0].1.stack.user.is_empty());
    }

    #[test]
    fn marker_kernel_then_user_at_own_ts() {
        // A marker that fired in kernel mode whose kernel and user fragments both
        // arrive at its own timestamp (e.g. FileIo/Read), interleaved with an
        // unrelated sample.
        let mut s: StackStitcher<u32> = StackStitcher::new();
        s.request_stack(1, 100, OwnTimestamp, 7);
        s.on_fragment(1, 100, vec![k(0xf1)]); // marker kernel fragment
        let out = s.on_fragment(1, 100, vec![u(0x10)]); // marker user fragment
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].payload, Some(7));
        assert_eq!(out[0].stack.kernel, vec![k(0xf1)]);
        assert_eq!(out[0].stack.user, vec![u(0x10)]);
    }

    #[test]
    fn kernel_only_marker_keeps_kernel_stack_at_flush() {
        // A marker on a kernel thread that never returns to user mode (FileIo on
        // the lazy writer): it keeps its kernel-only stack rather than borrowing a
        // wrong user stack.
        let mut s: StackStitcher<u32> = StackStitcher::new();
        s.request_stack(1, 100, OwnTimestamp, 7);
        s.on_fragment(1, 100, vec![k(0xf1), k(0xf2)]);
        let out = s.flush_all();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].1.payload, Some(7));
        assert_eq!(out[0].1.stack.kernel, vec![k(0xf1), k(0xf2)]);
        assert!(out[0].1.stack.user.is_empty());
    }
}
