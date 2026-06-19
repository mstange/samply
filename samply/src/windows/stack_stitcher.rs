//! Reassembly of the kernel/user stack fragments that ETW delivers separately.
//!
//! On Windows, a stack-bearing event (a profiler sample, or a marker) is
//! recorded first, and its stack arrives *later* as one or more `StackWalk`
//! fragments:
//!
//!  - Zero or more **kernel** fragments (the stack walk was still in kernel
//!    mode, so the outermost / root frame is a kernel address), followed by
//!  - one **user** fragment, whose root frame is a user address. This means the
//!    walk reached user space, so it is the *terminal* fragment. The user
//!    portion applies to this sample **and to every preceding pending request on
//!    the same thread** whose timestamp is `<=` the user fragment's timestamp
//!    (while a thread runs in the kernel — e.g. inside a syscall — several
//!    kernel-only samples can be collected before control returns to user mode
//!    and the user stack is captured).
//!
//! Some requests never receive a user fragment (the System process never runs
//! user code; and the trace can simply end) — those are flushed with whatever
//! kernel frames were collected, or an empty stack.
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
    /// Requests awaiting their stack, in arrival order. Timestamps are assumed
    /// non-decreasing (ETW delivers events roughly in time order), which lets us
    /// finalize a contiguous prefix when a user stack arrives.
    requests: VecDeque<PendingRequest<P>>,
}

impl<P> Default for ThreadPending<P> {
    fn default() -> Self {
        Self {
            requests: VecDeque::new(),
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
    /// will be delivered later via [`Self::on_fragment`].
    pub fn request_stack(&mut self, tid: u32, timestamp: u64, payload: P) {
        self.per_thread
            .entry(tid)
            .or_default()
            .requests
            .push_back(PendingRequest {
                timestamp,
                payload,
                kernel: Vec::new(),
            });
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
        // Attach to the most recent pending request at this exact timestamp.
        let Some(req) = thread
            .requests
            .iter_mut()
            .rev()
            .find(|r| r.timestamp == timestamp)
        else {
            // A stack for an event we never saw (or already finalized). Drop it.
            return;
        };
        req.kernel.extend_from_slice(&frames);
    }

    fn on_user_fragment(
        &mut self,
        tid: u32,
        timestamp: u64,
        frames: &[StackFrame],
    ) -> Vec<Finalized<P>> {
        // Split the fragment at its kernel→user boundary. Leading kernel frames
        // (if any) belong to the sample taken at `timestamp`; the user frames
        // are shared with all preceding pending requests.
        let boundary = frames
            .iter()
            .position(|f| f.stack_mode() == Some(StackMode::User))
            .unwrap_or(0);
        let (leaf_kernel, user) = frames.split_at(boundary);

        let n = self
            .per_thread
            .get(&tid)
            .map(|t| {
                t.requests
                    .iter()
                    .take_while(|r| r.timestamp <= timestamp)
                    .count()
            })
            .unwrap_or(0);

        if n == 0 {
            // Orphan: a terminal stack with no matching request. Hand back the
            // whole (possibly mixed) fragment with no payload.
            return vec![Finalized {
                timestamp,
                payload: None,
                stack: StitchedStack {
                    kernel: leaf_kernel.to_vec(),
                    user: user.to_vec(),
                },
            }];
        }

        let thread = self.per_thread.get_mut(&tid).unwrap();
        let mut out = Vec::with_capacity(n);
        for mut req in thread.requests.drain(..n) {
            // The fragment's own leaf kernel frames belong only to the sample
            // taken at the same timestamp; earlier samples keep just their own
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
        out
    }

    /// Finalize the most recent pending request at `timestamp` on `tid` with a
    /// kernel-only stack. Used for the System process (pid 4), whose samples
    /// never receive a user stack, so they can be emitted as soon as their
    /// kernel fragment arrives.
    pub fn finalize_kernel_only(&mut self, tid: u32, timestamp: u64) -> Option<Finalized<P>> {
        let thread = self.per_thread.get_mut(&tid)?;
        let pos = thread
            .requests
            .iter()
            .rposition(|r| r.timestamp == timestamp)?;
        let req = thread.requests.remove(pos)?;
        Some(Finalized {
            timestamp: req.timestamp,
            payload: Some(req.payload),
            stack: StitchedStack {
                kernel: req.kernel,
                user: Vec::new(),
            },
        })
    }

    /// Emit every remaining pending request (across all threads) with the kernel
    /// frames collected so far and no user stack. Call this at end of trace so
    /// in-flight samples aren't lost. Returns `(tid, finalized)` pairs.
    pub fn flush_all(&mut self) -> Vec<(u32, Finalized<P>)> {
        let mut out = Vec::new();
        for (tid, thread) in self.per_thread.drain() {
            for req in thread.requests {
                out.push((
                    tid,
                    Finalized {
                        timestamp: req.timestamp,
                        payload: Some(req.payload),
                        stack: StitchedStack {
                            kernel: req.kernel,
                            user: Vec::new(),
                        },
                    },
                ));
            }
        }
        out
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

    #[test]
    fn pure_user_sample() {
        let mut s: StackStitcher<u32> = StackStitcher::new();
        s.request_stack(1, 100, 42);
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
        s.request_stack(1, 100, 42);
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
        s.request_stack(1, 100, 1);
        s.on_fragment(1, 100, vec![k(0xa)]);
        s.request_stack(1, 200, 2);
        s.on_fragment(1, 200, vec![k(0xb)]);
        s.request_stack(1, 300, 3);
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
        s.request_stack(1, 100, 42);
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
        s.request_stack(1, 100, 42);
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
        s.request_stack(4, 100, 42);
        assert!(s.on_fragment(4, 100, vec![k(0xf1), k(0xf2)]).is_empty());
        let out = s.finalize_kernel_only(4, 100).unwrap();
        assert_eq!(out.payload, Some(42));
        assert_eq!(out.stack.kernel, vec![k(0xf1), k(0xf2)]);
        assert!(out.stack.user.is_empty());
    }

    #[test]
    fn flush_emits_leftovers_kernel_only() {
        let mut s: StackStitcher<u32> = StackStitcher::new();
        s.request_stack(1, 100, 1);
        s.on_fragment(1, 100, vec![k(0xa)]);
        s.request_stack(2, 150, 2); // never got any fragment
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
}
