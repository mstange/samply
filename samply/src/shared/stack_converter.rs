use std::collections::VecDeque;
use std::iter::{Cloned, Rev};

use fxprof_processed_profile::{
    FrameAddress, FrameFlags, FrameHandle, Profile, SubcategoryHandle, ThreadHandle,
};

use super::jit_category_manager::{JsFrame, JsName};
use super::lib_mappings::{AndroidArtInfo, LibMappingsHierarchy};
use super::types::{StackFrame, StackMode};

#[derive(Debug)]
pub struct StackConverter {
    user_category: SubcategoryHandle,
    kernel_category: SubcategoryHandle,
    libart_frame_buffer: VecDeque<SecondPassFrameInfo>,
}

struct FirstPassFrameInfo {
    mode: StackMode,
    lookup_address: u64,
    from_ip: bool,
}

#[derive(Debug)]
struct SecondPassFrameInfo {
    location: FrameAddress,
    category: SubcategoryHandle,
    js_frame: Option<JsFrame>,
    art_info: Option<AndroidArtInfo>,
}

struct FirstPassIter<I: Iterator<Item = StackFrame>>(I);

struct SecondPassIter<'a, I: Iterator<Item = FirstPassFrameInfo>> {
    inner: I,
    lib_mappings: &'a LibMappingsHierarchy,
    user_category: SubcategoryHandle,
    kernel_category: SubcategoryHandle,
}

struct LibartFilteringIter<'c, I: Iterator<Item = SecondPassFrameInfo>> {
    inner: I,
    last_emitted_was_java: bool,
    buffer: &'c mut VecDeque<SecondPassFrameInfo>,
}

struct ConvertedStackIterD<I: Iterator<Item = SecondPassFrameInfo>> {
    inner: I,
    thread: ThreadHandle,
    pending_frame_handle: Option<FrameHandle>,
    js_name_for_baseline_interpreter: Option<JsName>,
}

#[allow(clippy::type_complexity)]
pub struct ConvertedStackIter<'a>(
    ConvertedStackIterD<
        LibartFilteringIter<
            'a,
            SecondPassIter<'a, FirstPassIter<Rev<Cloned<std::slice::Iter<'a, StackFrame>>>>>,
        >,
    >,
);

impl<I: Iterator<Item = StackFrame>> Iterator for FirstPassIter<I> {
    type Item = FirstPassFrameInfo;

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let frame = self.0.next()?;
            let (mode, lookup_address, from_ip) = match frame {
                StackFrame::InstructionPointer(addr, mode) => (mode, addr, true),
                StackFrame::ReturnAddress(addr, mode) => (mode, addr.saturating_sub(1), false),
                StackFrame::AdjustedReturnAddress(addr, mode) => (mode, addr, false),
                StackFrame::TruncatedStackMarker => continue,
            };
            return Some(FirstPassFrameInfo {
                mode,
                lookup_address,
                from_ip,
            });
        }
    }
}

impl<I: Iterator<Item = FirstPassFrameInfo>> Iterator for SecondPassIter<'_, I> {
    type Item = SecondPassFrameInfo;

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }

    fn next(&mut self) -> Option<Self::Item> {
        let FirstPassFrameInfo {
            mode,
            lookup_address,
            from_ip,
        } = self.inner.next()?;
        let (location, category, js_frame, art_info) = match mode {
            StackMode::User => match self.lib_mappings.convert_address(lookup_address) {
                Some((relative_lookup_address, info)) => {
                    let location = if from_ip {
                        let relative_address = relative_lookup_address;
                        FrameAddress::RelativeAddressFromInstructionPointer(
                            info.lib_handle,
                            relative_address,
                        )
                    } else {
                        FrameAddress::RelativeAddressFromAdjustedReturnAddress(
                            info.lib_handle,
                            relative_lookup_address,
                        )
                    };
                    (
                        location,
                        info.category.unwrap_or(self.user_category),
                        info.js_frame,
                        info.art_info,
                    )
                }
                None => {
                    let location = match from_ip {
                        true => FrameAddress::InstructionPointer(lookup_address),
                        false => FrameAddress::AdjustedReturnAddress(lookup_address),
                    };
                    (location, self.user_category, None, None)
                }
            },
            StackMode::Kernel => {
                let location = match from_ip {
                    true => FrameAddress::InstructionPointer(lookup_address),
                    false => FrameAddress::AdjustedReturnAddress(lookup_address),
                };
                (location, self.kernel_category, None, None)
            }
        };
        Some(SecondPassFrameInfo {
            location,
            category,
            js_frame,
            art_info,
        })
    }
}

impl<I: Iterator<Item = SecondPassFrameInfo>> Iterator for LibartFilteringIter<'_, I> {
    type Item = SecondPassFrameInfo;

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(buffered_libart_frame) = self.buffer.pop_front() {
            return Some(buffered_libart_frame);
        }

        let mut frame = self.inner.next()?;

        if self.last_emitted_was_java {
            // Buffer frames until we find a non-libart frame.
            while frame.art_info == Some(AndroidArtInfo::LibArt) {
                self.buffer.push_back(frame);

                match self.inner.next() {
                    Some(next_frame) => {
                        frame = next_frame;
                    }
                    None => {
                        return self.buffer.pop_front();
                    }
                }
            }
        }

        if frame.art_info == Some(AndroidArtInfo::JavaFrame) {
            self.buffer.clear();
            self.last_emitted_was_java = true;
            return Some(frame);
        }

        self.last_emitted_was_java = false;
        if self.buffer.is_empty() {
            Some(frame)
        } else {
            self.buffer.push_back(frame);
            self.buffer.pop_front()
        }
    }
}

impl<I: Iterator<Item = SecondPassFrameInfo>> ConvertedStackIterD<I> {
    fn size_hint(&self) -> (usize, Option<usize>) {
        // Use the slice length as the size hint. This is a bit of a lie, unfortunately.
        // This iterator can yield more elements than self.inner if we add JS frames,
        // or fewer elements if the original iterator contains TruncatedStackMarker frames.
        // But it's a relatively good approximation.
        self.inner.size_hint()
    }

    fn next(&mut self, profile: &mut Profile) -> Option<FrameHandle> {
        if let Some(pending_frame_handle) = self.pending_frame_handle.take() {
            return Some(pending_frame_handle);
        }
        let SecondPassFrameInfo {
            location,
            category,
            js_frame,
            ..
        } = self.inner.next()?;

        let mut frame_flags = FrameFlags::empty();

        // Work around an imperfection in Spidermonkey's stack frames.
        // We sometimes have missing BaselineInterpreterStubs in the OSR-into-BaselineInterpreter case.
        // Usually, a BaselineInterpreter frame is directly preceded by a BaselineInterpreterStub frame.
        // However, sometimes you get Regular(x) -> None -> None -> None -> BaselineInterpreter,
        // without a BaselineInterpreterStub frame. In that case, the name "x" from the ancestor
        // JsFrame::RegularInAdditionToNativeFrame (which is really an InterpreterStub frame for the C++ interpreter)
        // should be used for the BaselineInterpreter frame. This will create a stack
        // node with the right name, category and JS-only flag, and helps with correct attribution.
        // Unfortunately it means that we'll have two prepended JS label frames for the same function
        // in that case, but that's still better than accounting those samples to the wrong JS function.
        let extra_js_name = match js_frame {
            Some(JsFrame::NativeFrameIsJs) => {
                frame_flags |= FrameFlags::IS_JS;
                None
            }
            Some(JsFrame::RegularInAdditionToNativeFrame(js_name)) => {
                // Remember the name for a potentially upcoming unnamed BaselineInterpreter frame.
                self.js_name_for_baseline_interpreter = Some(js_name);
                Some(js_name)
            }
            Some(JsFrame::BaselineInterpreterStub(js_name)) => {
                // Discard the name of an ancestor JS function.
                self.js_name_for_baseline_interpreter = None;
                Some(js_name)
            }
            Some(JsFrame::BaselineInterpreter) => self.js_name_for_baseline_interpreter.take(),
            None => None,
        };

        let mut frame_handle =
            profile.handle_for_frame_with_address(self.thread, location, category, frame_flags);
        if let Some(JsName::NonSelfHosted(js_name)) = extra_js_name {
            // Prepend a JS frame.
            // We don't treat Spidermonkey "self-hosted" functions as JS (e.g. filter/map/push).
            let prepended_js_frame = profile.handle_for_frame_with_label(
                self.thread,
                js_name,
                category,
                FrameFlags::IS_JS,
            );
            let buffered_frame = std::mem::replace(&mut frame_handle, prepended_js_frame);
            self.pending_frame_handle = Some(buffered_frame);
        };

        Some(frame_handle)
    }
}

impl ConvertedStackIter<'_> {
    pub fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }

    pub fn next(&mut self, profile: &mut Profile) -> Option<FrameHandle> {
        self.0.next(profile)
    }
}

impl StackConverter {
    pub fn new(user_category: SubcategoryHandle, kernel_category: SubcategoryHandle) -> Self {
        Self {
            user_category,
            kernel_category,
            libart_frame_buffer: VecDeque::new(),
        }
    }

    /// Takes a stack going from callee to root caller.
    ///
    /// Returns an iterator going from root caller to callee.
    pub fn convert_stack<'a>(
        &'a mut self,
        thread: ThreadHandle,
        stack: &'a [StackFrame],
        lib_mappings: &'a LibMappingsHierarchy,
        extra_first_frame: Option<FrameHandle>,
    ) -> ConvertedStackIter<'a> {
        let pass1 = FirstPassIter(stack.iter().cloned().rev());
        let pass2 = SecondPassIter {
            inner: pass1,
            lib_mappings,
            user_category: self.user_category,
            kernel_category: self.kernel_category,
        };
        self.libart_frame_buffer.clear();
        let pass3 = LibartFilteringIter {
            inner: pass2,
            last_emitted_was_java: false,
            buffer: &mut self.libart_frame_buffer,
        };
        let pass4 = ConvertedStackIterD {
            inner: pass3,
            thread,
            pending_frame_handle: extra_first_frame,
            js_name_for_baseline_interpreter: None,
        };
        ConvertedStackIter(pass4)
    }
}
