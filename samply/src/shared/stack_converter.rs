use fxprof_processed_profile::{CategoryPairHandle, Frame, FrameFlags, FrameInfo};

use super::jit_category_manager::{JsFrame, JsName};
use super::lib_mappings::{AndroidArtInfo, LibMappingsHierarchy};
use super::types::{StackFrame, StackMode};

#[derive(Debug, Clone, Copy)]
pub struct StackConverter {
    user_category: CategoryPairHandle,
    kernel_category: CategoryPairHandle,
}

pub struct ConvertedStackIter<'a> {
    inner: std::iter::Rev<std::slice::Iter<'a, StackFrame>>,
    lib_mappings: &'a LibMappingsHierarchy,
    user_category: CategoryPairHandle,
    kernel_category: CategoryPairHandle,
    pending_frame_info: Option<FrameInfo>,
    pending_frame: Option<&'a StackFrame>,
    js_name_for_baseline_interpreter: Option<JsName>,
    previous_frame_was_dex_or_oat: bool,
}

impl<'a> Iterator for ConvertedStackIter<'a> {
    type Item = FrameInfo;

    // Implement this because it's called by StackDepthLimitingFrameIter
    fn size_hint(&self) -> (usize, Option<usize>) {
        // Use the slice length as the size hint. This is a bit of a lie, unfortunately.
        // This iterator can yield more elements than self.inner if we add JS frames,
        // or fewer elements if the original iterator contains TruncatedStackMarker frames.
        // But it's a relatively good approximation.
        self.inner.size_hint()
    }

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(pending_frame_info) = self.pending_frame_info.take() {
                return Some(pending_frame_info);
            }
            let frame = self.pending_frame.take().or_else(|| self.inner.next())?;
            let (mode, lookup_address, from_ip) = match *frame {
                StackFrame::InstructionPointer(addr, mode) => (mode, addr, true),
                StackFrame::ReturnAddress(addr, mode) => (mode, addr.saturating_sub(1), false),
                StackFrame::AdjustedReturnAddress(addr, mode) => (mode, addr, false),
                StackFrame::TruncatedStackMarker => continue,
            };
            let (location, category, js_frame, art_info) = match mode {
                StackMode::User => match self.lib_mappings.convert_address(lookup_address) {
                    Some((relative_lookup_address, info)) => {
                        let location = if from_ip {
                            let relative_address = relative_lookup_address;
                            Frame::RelativeAddressFromInstructionPointer(
                                info.lib_handle,
                                relative_address,
                            )
                        } else {
                            Frame::RelativeAddressFromAdjustedReturnAddress(
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
                            true => Frame::InstructionPointer(lookup_address),
                            false => Frame::AdjustedReturnAddress(lookup_address),
                        };
                        (location, self.user_category, None, None)
                    }
                },
                StackMode::Kernel => {
                    let location = match from_ip {
                        true => Frame::InstructionPointer(lookup_address),
                        false => Frame::AdjustedReturnAddress(lookup_address),
                    };
                    (location, self.kernel_category, None, None)
                }
            };

            match art_info {
                Some(AndroidArtInfo::LibArt) if self.previous_frame_was_dex_or_oat => {
                    // We want to skip libart frames if they're immediately surrounded by
                    // dex_or_oat frames on both sides.
                    if let Some(next_frame) = self.inner.next() {
                        let should_skip =
                            StackConverter::frame_is_dex_or_oat(next_frame, self.lib_mappings);
                        self.pending_frame = Some(next_frame);
                        if should_skip {
                            continue; // Skip this libart frame.
                        }
                    }
                    self.previous_frame_was_dex_or_oat = false;
                }
                Some(AndroidArtInfo::DexOrOat) => {
                    self.previous_frame_was_dex_or_oat = true;
                }
                _ => self.previous_frame_was_dex_or_oat = false,
            }

            let frame_info = FrameInfo {
                frame: location,
                category_pair: category,
                flags: FrameFlags::empty(),
            };

            // Work around an imperfection in Spidermonkey's stack frames.
            // We sometimes have missing BaselineInterpreterStubs in the OSR-into-BaselineInterpreter case.
            // Usually, a BaselineInterpreter frame is directly preceded by a BaselineInterpreterStub frame.
            // However, sometimes you get Regular(x) -> None -> None -> None -> BaselineInterpreter,
            // without a BaselineInterpreterStub frame. In that case, the name "x" from the ancestor
            // JsFrame::Regular (which is really an InterpreterStub frame for the C++ interpreter)
            // should be used for the BaselineInterpreter frame. This will create a stack
            // node with the right name, category and JS-only flag, and helps with correct attribution.
            // Unfortunately it means that we'll have two prepended JS label frames for the same function
            // in that case, but that's still better than accounting those samples to the wrong JS function.
            let js_name = match js_frame {
                Some(JsFrame::Regular(js_name)) => {
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

            let frame_info = match js_name {
                Some(JsName::NonSelfHosted(js_name)) => {
                    // Prepend a JS frame.
                    self.pending_frame_info = Some(frame_info);
                    FrameInfo {
                        frame: Frame::Label(js_name),
                        category_pair: category,
                        flags: FrameFlags::IS_JS,
                    }
                }
                // Don't treat Spidermonkey "self-hosted" functions as JS (e.g. filter/map/push).
                Some(JsName::SelfHosted(_)) | None => frame_info,
            };
            return Some(frame_info);
        }
    }
}

impl StackConverter {
    pub fn new(user_category: CategoryPairHandle, kernel_category: CategoryPairHandle) -> Self {
        Self {
            user_category,
            kernel_category,
        }
    }

    pub fn convert_stack<'a>(
        &self,
        stack: &'a [StackFrame],
        lib_mappings: &'a LibMappingsHierarchy,
        extra_first_frame: Option<FrameInfo>,
    ) -> impl Iterator<Item = FrameInfo> + 'a {
        ConvertedStackIter {
            inner: stack.iter().rev(),
            lib_mappings,
            user_category: self.user_category,
            kernel_category: self.kernel_category,
            pending_frame_info: extra_first_frame,
            pending_frame: None,
            js_name_for_baseline_interpreter: None,
            previous_frame_was_dex_or_oat: false,
        }
    }

    fn frame_is_dex_or_oat(frame: &StackFrame, lib_mappings: &LibMappingsHierarchy) -> bool {
        let lookup_address = match *frame {
            StackFrame::InstructionPointer(addr, StackMode::User) => addr,
            StackFrame::ReturnAddress(addr, StackMode::User) => addr.saturating_sub(1),
            StackFrame::AdjustedReturnAddress(addr, StackMode::User) => addr,
            _ => return false,
        };
        let Some((_, info)) = lib_mappings.convert_address(lookup_address) else {
            return false;
        };
        info.art_info == Some(AndroidArtInfo::DexOrOat)
    }
}
