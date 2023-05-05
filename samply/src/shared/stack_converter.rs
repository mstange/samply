use fxprof_processed_profile::{CategoryPairHandle, Frame, FrameFlags, FrameInfo};

use super::jit_category_manager::{JsFrame, JsName};
use super::lib_mappings::LibMappingsHierarchy;
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
    pending_frame: Option<FrameInfo>,
    js_name_for_baseline_interpreter: Option<JsName>,
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
            if let Some(pending_frame) = self.pending_frame.take() {
                return Some(pending_frame);
            }
            let frame = self.inner.next()?;
            let (mode, addr, lookup_address, from_ip) = match *frame {
                StackFrame::InstructionPointer(addr, mode) => (mode, addr, addr, true),
                StackFrame::ReturnAddress(addr, mode) => {
                    (mode, addr, addr.saturating_sub(1), false)
                }
                StackFrame::TruncatedStackMarker => continue,
            };
            let (location, category, js_frame) = match mode {
                StackMode::User => match self.lib_mappings.convert_address(lookup_address) {
                    Some((relative_address, info)) => {
                        let location = match from_ip {
                            true => Frame::RelativeAddressFromInstructionPointer(
                                info.lib_handle,
                                relative_address,
                            ),
                            false => Frame::RelativeAddressFromReturnAddress(
                                info.lib_handle,
                                relative_address,
                            ),
                        };
                        (
                            location,
                            info.category.unwrap_or(self.user_category),
                            info.js_frame,
                        )
                    }
                    None => {
                        let location = match from_ip {
                            true => Frame::InstructionPointer(addr),
                            false => Frame::ReturnAddress(addr),
                        };
                        (location, self.user_category, None)
                    }
                },
                StackMode::Kernel => {
                    let location = match from_ip {
                        true => Frame::InstructionPointer(addr),
                        false => Frame::ReturnAddress(addr),
                    };
                    (location, self.kernel_category, None)
                }
            };
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
                    self.pending_frame = Some(frame_info);
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
            pending_frame: extra_first_frame,
            js_name_for_baseline_interpreter: None,
        }
    }
}
