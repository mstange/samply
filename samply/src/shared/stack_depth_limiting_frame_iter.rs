use fxprof_processed_profile::{
    Frame, FrameFlags, FrameInfo, Profile, StringHandle, SubcategoryHandle,
};

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

pub struct StackDepthLimitingFrameIter<I: Iterator<Item = FrameInfo>> {
    inner: I,
    category: SubcategoryHandle,
    state: StackDepthLimitingFrameIterState,
}

enum StackDepthLimitingFrameIterState {
    BeforeElidedPiece {
        index: usize,
        first_elided_frame: usize,
        elision_frame_string: StringHandle,
        first_frame_after_elision: usize,
    },
    AtElidedPiece {
        elision_frame_string: StringHandle,
        first_frame_after_elision: usize,
    },
    NoMoreElision {
        index: usize,
    },
}

impl<I: Iterator<Item = FrameInfo>> StackDepthLimitingFrameIter<I> {
    pub fn new(profile: &mut Profile, iter: I, category: SubcategoryHandle) -> Self {
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
            StackDepthLimitingFrameIterState::BeforeElidedPiece {
                index: 0,
                first_elided_frame,
                elision_frame_string,
                first_frame_after_elision,
            }
        } else {
            StackDepthLimitingFrameIterState::NoMoreElision { index: 0 }
        };
        Self {
            inner: iter,
            category,
            state,
        }
    }
}

impl<I: Iterator<Item = FrameInfo>> Iterator for StackDepthLimitingFrameIter<I> {
    type Item = FrameInfo;

    fn next(&mut self) -> Option<Self::Item> {
        let frame = match &mut self.state {
            StackDepthLimitingFrameIterState::BeforeElidedPiece {
                index,
                first_elided_frame,
                elision_frame_string,
                first_frame_after_elision,
            } => {
                let frame = self.inner.next()?;
                *index += 1;
                if *index == *first_elided_frame {
                    while *index < *first_frame_after_elision {
                        let _frame = self.inner.next()?;
                        *index += 1;
                    }
                    self.state = StackDepthLimitingFrameIterState::AtElidedPiece {
                        elision_frame_string: *elision_frame_string,
                        first_frame_after_elision: *first_frame_after_elision,
                    };
                }
                frame
            }
            StackDepthLimitingFrameIterState::AtElidedPiece {
                elision_frame_string,
                first_frame_after_elision,
            } => {
                let frame = Frame::Label(*elision_frame_string);
                self.state = StackDepthLimitingFrameIterState::NoMoreElision {
                    index: *first_frame_after_elision,
                };
                return Some(FrameInfo {
                    frame,
                    subcategory: self.category,
                    flags: FrameFlags::empty(),
                });
            }
            StackDepthLimitingFrameIterState::NoMoreElision { index } => {
                let frame = self.inner.next()?;
                *index += 1;
                frame
            }
        };

        Some(frame)
    }
}
