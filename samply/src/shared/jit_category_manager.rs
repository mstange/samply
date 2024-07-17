use fxprof_processed_profile::{
    CategoryColor, CategoryHandle, CategoryPairHandle, Profile, StringHandle,
};

#[derive(Debug, Clone, Copy)]
pub enum JsFrame {
    #[allow(dead_code)]
    NativeFrameIsJs,
    RegularInAdditionToNativeFrame(JsName),
    BaselineInterpreterStub(JsName),
    BaselineInterpreter,
}

#[derive(Debug, Clone, Copy)]
pub enum JsName {
    SelfHosted(#[allow(dead_code)] StringHandle),
    NonSelfHosted(StringHandle),
}

#[derive(Debug, Clone)]
pub struct JitCategoryManager {
    categories: Vec<LazilyCreatedCategory>,
    baseline_interpreter_category: LazilyCreatedCategory,
    ion_ic_category: LazilyCreatedCategory,
    wasm_liftoff_category: LazilyCreatedCategory,
    wasm_turbofan_category: LazilyCreatedCategory,
    generic_jit_category: LazilyCreatedCategory,
}

impl JitCategoryManager {
    /// (prefix, name, color, is_js)
    const CATEGORIES: &'static [(&'static str, &'static str, CategoryColor, bool)] = &[
        ("JS:~", "Interpreter", CategoryColor::Magenta, true),
        ("Script:~", "Interpreter", CategoryColor::Magenta, true),
        ("JS:^", "Baseline", CategoryColor::Blue, true),
        ("JS:+", "Maglev", CategoryColor::Green, true),
        ("JS:*", "Turbofan", CategoryColor::Green, true),
        ("JS:?", "JavaScript", CategoryColor::Blue, true),
        ("Builtin:", "Builtin", CategoryColor::Brown, false),
        ("BytecodeHandler:", "Interpreter", CategoryColor::Red, false),
        ("Interpreter: ", "Interpreter", CategoryColor::Red, true),
        (
            "BaselineThunk: ",
            "Trampoline",
            CategoryColor::DarkGray,
            false,
        ),
        ("Baseline: ", "Baseline", CategoryColor::Blue, true),
        (
            "PolymorphicCallStubBaseline: ",
            "Trampoline",
            CategoryColor::DarkGray,
            true,
        ),
        (
            "PolymorphicAccessStubBaseline: ",
            "Trampoline",
            CategoryColor::DarkGray,
            true,
        ),
        ("Ion: ", "Ion", CategoryColor::Green, true),
        ("Wasm: ", "Wasm", CategoryColor::Blue, true),
        ("BaselineIC: ", "BaselineIC", CategoryColor::Brown, false),
        ("IC: ", "IC", CategoryColor::Brown, false),
        ("Trampoline: ", "Trampoline", CategoryColor::DarkGray, false),
        (
            "WasmTrampoline: ",
            "Trampoline",
            CategoryColor::DarkGray,
            false,
        ),
        ("VMWrapper: ", "Trampoline", CategoryColor::DarkGray, false),
        (
            "Baseline JIT code for ",
            "Baseline",
            CategoryColor::Blue,
            true,
        ),
        ("DFG JIT code for DFG: ", "DFG", CategoryColor::Green, true),
        ("FTL B3 code for FTL: ", "FTL", CategoryColor::Green, true),
        ("LLInt: ", "LLInt", CategoryColor::Red, true),
    ];

    pub fn new() -> Self {
        Self {
            categories: Self::CATEGORIES
                .iter()
                .map(|(_prefix, name, color, _is_js)| LazilyCreatedCategory::new(name, *color))
                .collect(),
            baseline_interpreter_category: LazilyCreatedCategory::new(
                "BaselineInterpreter",
                CategoryColor::Magenta,
            ),
            ion_ic_category: LazilyCreatedCategory::new("IonIC", CategoryColor::Brown),
            wasm_liftoff_category: LazilyCreatedCategory::new(
                "Liftoff (wasm)",
                CategoryColor::Blue,
            ),
            wasm_turbofan_category: LazilyCreatedCategory::new(
                "Turbofan (wasm)",
                CategoryColor::Green,
            ),
            generic_jit_category: LazilyCreatedCategory::new("JIT", CategoryColor::Purple),
        }
    }

    #[allow(dead_code)]
    pub fn default_category(&mut self, profile: &mut Profile) -> CategoryHandle {
        self.generic_jit_category.get(profile)
    }

    /// Get the category and JS function name for a function from JIT code.
    ///
    /// The category is only created in the profile once a function with that
    /// category is encountered.
    pub fn classify_jit_symbol(
        &mut self,
        name: &str,
        profile: &mut Profile,
    ) -> (CategoryPairHandle, Option<JsFrame>) {
        if name == "BaselineInterpreter" || name.starts_with("BlinterpOp: ") {
            return (
                self.baseline_interpreter_category.get(profile).into(),
                Some(JsFrame::BaselineInterpreter),
            );
        }

        if let Some(js_func) = name.strip_prefix("BaselineInterpreter: ") {
            let js_func = JsFrame::BaselineInterpreterStub(Self::intern_js_name(profile, js_func));
            return (
                self.baseline_interpreter_category.get(profile).into(),
                Some(js_func),
            );
        }

        if let Some(ion_ic_rest) = name.strip_prefix("IonIC: ") {
            let category = self.ion_ic_category.get(profile);
            if let Some((_ic_type, js_func)) = ion_ic_rest.split_once(" : ") {
                let js_func =
                    JsFrame::RegularInAdditionToNativeFrame(Self::intern_js_name(profile, js_func));
                return (category.into(), Some(js_func));
            }
            return (category.into(), None);
        }

        if let Some(ion_ic_rest) = name.strip_prefix("IonIC: ") {
            let category = self.ion_ic_category.get(profile);
            if let Some((_ic_type, js_func)) = ion_ic_rest.split_once(" : ") {
                let js_func =
                    JsFrame::RegularInAdditionToNativeFrame(Self::intern_js_name(profile, js_func));
                return (category.into(), Some(js_func));
            }
            return (category.into(), None);
        }

        for (&(prefix, _category_name, _color, is_js), lazy_category_handle) in
            Self::CATEGORIES.iter().zip(self.categories.iter_mut())
        {
            if let Some(name_without_prefix) = name.strip_prefix(prefix) {
                let category = lazy_category_handle.get(profile);

                let js_name = if is_js {
                    Some(JsFrame::RegularInAdditionToNativeFrame(
                        Self::intern_js_name(profile, name_without_prefix),
                    ))
                } else {
                    None
                };
                return (category.into(), js_name);
            }
        }

        if let Some(v8_wasm_name) = name.strip_prefix("JS:") {
            let stripped_name =
                if let Some(v8_wasm_liftoff_name) = v8_wasm_name.strip_suffix("-liftoff") {
                    // "JS:wasm-function[5206]-5206-liftoff"
                    // "JS:StatefulElement.performRebuild-2761-liftoff"
                    Some((v8_wasm_liftoff_name, &mut self.wasm_liftoff_category))
                } else if let Some(v8_wasm_turbofan_name) = v8_wasm_name.strip_suffix("-turbofan") {
                    // "JS:wasm-function[5307]-5307-turbofan"
                    // "JS:SceneBuilder._pushLayer-10063-turbofan"
                    Some((v8_wasm_turbofan_name, &mut self.wasm_turbofan_category))
                } else {
                    None
                };
            if let Some((v8_wasm_name_with_index, category)) = stripped_name {
                // "SceneBuilder._pushLayer-10063"
                if let Some((v8_wasm_name, func_index)) = v8_wasm_name_with_index.rsplit_once('-') {
                    let new_name = format!("{v8_wasm_name} (WASM:{func_index})");
                    let category = category.get(profile);
                    let js_func = JsFrame::RegularInAdditionToNativeFrame(Self::intern_js_name(
                        profile, &new_name,
                    ));
                    return (category.into(), Some(js_func));
                }
            }
        }

        // "run_wasm_sm.js line 41 > WebAssembly.Module:916249: Function Element.updateChild"
        // "run_wasm_sm.js line 41 > WebAssembly.Module:825626: Function wasm-function[1491]"

        let category = self.generic_jit_category.get(profile);
        (category.into(), None)
    }

    fn intern_js_name(profile: &mut Profile, func_name: &str) -> JsName {
        if let Some((before, after)) = func_name
            .split_once("[Call")
            .or_else(|| func_name.split_once("[Construct"))
        {
            // Canonicalize JSC name, e.g. "diffProps[Call (StrictMode)] /home/.../index.js:123:12"
            // and "diffProps[Call (DidTryToEnterInLoop) (StrictMode)] /home/.../index.js:123:12"
            if let Some((_square_bracket_call_stuff, after)) = after.split_once(']') {
                if after.is_empty() {
                    // Nothing is following the closing square bracket, in particular no filename.
                    // Example: "forEach[Call (StrictMode)]"
                    // This is likely a self-hosted function.
                    return JsName::SelfHosted(profile.intern_string(before));
                }
                return JsName::NonSelfHosted(profile.intern_string(&format!("{before}{after}")));
            }
        }

        let s = profile.intern_string(func_name);
        match func_name.contains("(self-hosted:")
            || func_name.ends_with("valueIsFalsey")
            || func_name.ends_with("valueIsTruthy")
        {
            true => JsName::SelfHosted(s),
            false => JsName::NonSelfHosted(s),
        }
    }
}

#[derive(Debug, Clone)]
struct LazilyCreatedCategory {
    name: &'static str,
    color: CategoryColor,
    handle: Option<CategoryHandle>,
}

impl LazilyCreatedCategory {
    pub fn new(name: &'static str, color: CategoryColor) -> Self {
        Self {
            name,
            color,
            handle: None,
        }
    }

    pub fn get(&mut self, profile: &mut Profile) -> CategoryHandle {
        *self
            .handle
            .get_or_insert_with(|| profile.add_category(self.name, self.color))
    }
}

#[cfg(test)]
mod test {
    use fxprof_processed_profile::{ReferenceTimestamp, SamplingInterval};

    use super::*;

    #[test]
    fn test() {
        let mut manager = JitCategoryManager::new();
        let mut profile = Profile::new(
            "",
            ReferenceTimestamp::from_millis_since_unix_epoch(0.0),
            SamplingInterval::from_millis(1),
        );
        let (_category, js_name) = manager.classify_jit_symbol(
            "IonIC: SetElem : AccessibleButton (main.js:3560:25)",
            &mut profile,
        );
        match js_name {
            Some(JsFrame::RegularInAdditionToNativeFrame(JsName::NonSelfHosted(s))) => {
                assert_eq!(profile.get_string(s), "AccessibleButton (main.js:3560:25)")
            }
            _ => panic!(),
        }
    }
}
