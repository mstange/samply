use fxprof_processed_profile::{FrameFlags, FrameHandle, Profile, SubcategoryHandle, ThreadHandle};

use super::stack_converter::ConvertedStackIter;

/// Returns `Some((start_index, count))` if part of the stack should be elided
/// in order to limit the stack length to < 2.5 * N.
///
/// The stack is partitioned into three pieces:
///   1. N frames at the beginning which are kept.
///   2. k * N frames in the middle which are elided and replaced with a placeholder.
///   3. ~avg N frames at the end which are kept.
///
/// The third piece is m frames, and k is chosen such that 0.5 * N <= m < 1.5 * N
fn should_elide_frames<const N: usize>(full_len: usize) -> Option<(usize, usize)> {
    if full_len >= N + N + N / 2 {
        let elided_count = (full_len - N - N / 2) / N * N;
        Some((N, elided_count))
    } else {
        None
    }
}

#[test]
fn test_should_elide_frames() {
    assert_eq!(should_elide_frames::<100>(100), None);
    assert_eq!(should_elide_frames::<100>(220), None);
    assert_eq!(should_elide_frames::<100>(249), None);
    assert_eq!(should_elide_frames::<100>(250), Some((100, 100)));
    assert_eq!(should_elide_frames::<100>(290), Some((100, 100)));
    assert_eq!(should_elide_frames::<100>(349), Some((100, 100)));
    assert_eq!(should_elide_frames::<100>(350), Some((100, 200)));
    assert_eq!(should_elide_frames::<100>(352), Some((100, 200)));
    assert_eq!(should_elide_frames::<100>(449), Some((100, 200)));
    assert_eq!(should_elide_frames::<100>(450), Some((100, 300)));
}

pub struct StackDepthLimitingFrameIter<'a> {
    inner: ConvertedStackIter<'a>,
    state: StackDepthLimitingFrameIterState,
}

enum StackDepthLimitingFrameIterState {
    BeforeElidedPiece {
        index: usize,
        first_elided_frame: usize,
        elision_frame_handle: FrameHandle,
        first_frame_after_elision: usize,
    },
    AtElidedPiece {
        elision_frame_handle: FrameHandle,
        first_frame_after_elision: usize,
    },
    NoMoreElision {
        index: usize,
    },
}

impl<'a> StackDepthLimitingFrameIter<'a> {
    pub fn new(
        profile: &mut Profile,
        iter: ConvertedStackIter<'a>,
        thread: ThreadHandle,
        category: SubcategoryHandle,
    ) -> Self {
        // Check if part of the stack should be elided, to limit the stack depth.
        // Without such a limit, profiles with deep recursion may become too big
        // to be processed.
        // We limit to a depth of 500 frames, eliding chunks of 200 frames in the
        // middle, keeping 200 frames at the start and 100 to 300 frames at the end.
        let full_len = iter.size_hint().0;
        let state = if let Some((first_elided_frame, elided_count)) =
            should_elide_frames::<200>(full_len)
        {
            let first_frame_after_elision = first_elided_frame + elided_count;
            let elision_frame_string =
                profile.handle_for_string(&format!("({elided_count} frames elided)"));
            let elision_frame_handle = profile.handle_for_frame_with_label(
                thread,
                elision_frame_string,
                category,
                FrameFlags::empty(),
            );
            StackDepthLimitingFrameIterState::BeforeElidedPiece {
                index: 0,
                first_elided_frame,
                elision_frame_handle,
                first_frame_after_elision,
            }
        } else {
            StackDepthLimitingFrameIterState::NoMoreElision { index: 0 }
        };
        Self { inner: iter, state }
    }
}

impl StackDepthLimitingFrameIter<'_> {
    pub fn next(&mut self, profile: &mut Profile) -> Option<FrameHandle> {
        let frame = match &mut self.state {
            StackDepthLimitingFrameIterState::BeforeElidedPiece {
                index,
                first_elided_frame,
                elision_frame_handle,
                first_frame_after_elision,
            } => {
                let frame = self.inner.next(profile)?;
                *index += 1;
                if *index == *first_elided_frame {
                    while *index < *first_frame_after_elision {
                        let _frame = self.inner.next(profile)?;
                        *index += 1;
                    }
                    self.state = StackDepthLimitingFrameIterState::AtElidedPiece {
                        elision_frame_handle: *elision_frame_handle,
                        first_frame_after_elision: *first_frame_after_elision,
                    };
                }
                frame
            }
            StackDepthLimitingFrameIterState::AtElidedPiece {
                elision_frame_handle,
                first_frame_after_elision,
            } => {
                let frame_handle = *elision_frame_handle;
                self.state = StackDepthLimitingFrameIterState::NoMoreElision {
                    index: *first_frame_after_elision,
                };
                return Some(frame_handle);
            }
            StackDepthLimitingFrameIterState::NoMoreElision { index } => {
                let frame = self.inner.next(profile)?;
                *index += 1;
                frame
            }
        };

        Some(frame)
    }
}
