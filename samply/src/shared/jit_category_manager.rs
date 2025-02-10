use fxprof_processed_profile::{
    Category, CategoryColor, CategoryHandle, Profile, StringHandle, SubcategoryHandle,
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
    const CATEGORIES: &'static [(&'static str, Category<'static>, bool)] = &[
        (
            "JS:~",
            Category("Interpreter", CategoryColor::Magenta),
            true,
        ),
        (
            "Script:~",
            Category("Interpreter", CategoryColor::Magenta),
            true,
        ),
        ("JS:^", Category("Baseline", CategoryColor::Blue), true),
        ("JS:+", Category("Maglev", CategoryColor::Green), true),
        ("JS:*", Category("Turbofan", CategoryColor::Green), true),
        ("JS:?", Category("JavaScript", CategoryColor::Blue), true),
        ("py::", Category("Python", CategoryColor::Blue), true),
        ("Builtin:", Category("Builtin", CategoryColor::Brown), false),
        (
            "BytecodeHandler:",
            Category("Interpreter", CategoryColor::Red),
            false,
        ),
        (
            "Interpreter: ",
            Category("Interpreter", CategoryColor::Red),
            true,
        ),
        (
            "BaselineThunk: ",
            Category("Trampoline", CategoryColor::DarkGray),
            false,
        ),
        (
            "Baseline: ",
            Category("Baseline", CategoryColor::Blue),
            true,
        ),
        (
            "PolymorphicCallStubBaseline: ",
            Category("Trampoline", CategoryColor::DarkGray),
            true,
        ),
        (
            "PolymorphicAccessStubBaseline: ",
            Category("Trampoline", CategoryColor::DarkGray),
            true,
        ),
        ("Ion: ", Category("Ion", CategoryColor::Green), true),
        ("Wasm: ", Category("Wasm", CategoryColor::Blue), true),
        (
            "BaselineIC: ",
            Category("BaselineIC", CategoryColor::Brown),
            false,
        ),
        ("IC: ", Category("IC", CategoryColor::Brown), false),
        (
            "Trampoline: ",
            Category("Trampoline", CategoryColor::DarkGray),
            false,
        ),
        (
            "WasmTrampoline: ",
            Category("Trampoline", CategoryColor::DarkGray),
            false,
        ),
        (
            "VMWrapper: ",
            Category("Trampoline", CategoryColor::DarkGray),
            false,
        ),
        (
            "Baseline JIT code for ",
            Category("Baseline", CategoryColor::Blue),
            true,
        ),
        (
            "DFG JIT code for DFG: ",
            Category("DFG", CategoryColor::Green),
            true,
        ),
        (
            "FTL B3 code for FTL: ",
            Category("FTL", CategoryColor::Green),
            true,
        ),
        ("LLInt: ", Category("LLInt", CategoryColor::Red), true),
    ];

    pub fn new() -> Self {
        Self {
            categories: Self::CATEGORIES
                .iter()
                .map(|(_prefix, category, _is_js)| (*category).into())
                .collect(),
            baseline_interpreter_category: Category("BaselineInterpreter", CategoryColor::Magenta)
                .into(),
            ion_ic_category: Category("IonIC", CategoryColor::Brown).into(),
            wasm_liftoff_category: Category("Liftoff (wasm)", CategoryColor::Blue).into(),
            wasm_turbofan_category: Category("Turbofan (wasm)", CategoryColor::Green).into(),
            generic_jit_category: Category("JIT", CategoryColor::Purple).into(),
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
    ) -> (SubcategoryHandle, Option<JsFrame>) {
        if name == "BaselineInterpreter" || name.starts_with("BlinterpOp: ") {
            return (
                self.baseline_interpreter_category.get(profile).into(),
                Some(JsFrame::BaselineInterpreter),
            );
        }

        if let Some(js_func) = name.strip_prefix("BaselineInterpreter: ") {
            let js_func =
                JsFrame::BaselineInterpreterStub(Self::handle_for_js_name(profile, js_func));
            return (
                self.baseline_interpreter_category.get(profile).into(),
                Some(js_func),
            );
        }

        if let Some(ion_ic_rest) = name.strip_prefix("IonIC: ") {
            let category = self.ion_ic_category.get(profile);
            if let Some((_ic_type, js_func)) = ion_ic_rest.split_once(" : ") {
                let js_func = JsFrame::RegularInAdditionToNativeFrame(Self::handle_for_js_name(
                    profile, js_func,
                ));
                return (category.into(), Some(js_func));
            }
            return (category.into(), None);
        }

        if let Some(ion_ic_rest) = name.strip_prefix("IonIC: ") {
            let category = self.ion_ic_category.get(profile);
            if let Some((_ic_type, js_func)) = ion_ic_rest.split_once(" : ") {
                let js_func = JsFrame::RegularInAdditionToNativeFrame(Self::handle_for_js_name(
                    profile, js_func,
                ));
                return (category.into(), Some(js_func));
            }
            return (category.into(), None);
        }

        for (&(prefix, _category, is_js), lazy_category_handle) in
            Self::CATEGORIES.iter().zip(self.categories.iter_mut())
        {
            if let Some(name_without_prefix) = name.strip_prefix(prefix) {
                let category = lazy_category_handle.get(profile);

                let js_name = if is_js {
                    Some(JsFrame::RegularInAdditionToNativeFrame(
                        Self::handle_for_js_name(profile, name_without_prefix),
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
                    let js_func = JsFrame::RegularInAdditionToNativeFrame(
                        Self::handle_for_js_name(profile, &new_name),
                    );
                    return (category.into(), Some(js_func));
                }
            }
        }

        // "run_wasm_sm.js line 41 > WebAssembly.Module:916249: Function Element.updateChild"
        // "run_wasm_sm.js line 41 > WebAssembly.Module:825626: Function wasm-function[1491]"

        let category = self.generic_jit_category.get(profile);
        (category.into(), None)
    }

    fn handle_for_js_name(profile: &mut Profile, func_name: &str) -> JsName {
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
                    return JsName::SelfHosted(profile.handle_for_string(before));
                }
                return JsName::NonSelfHosted(
                    profile.handle_for_string(&format!("{before}{after}")),
                );
            }
        }

        let s = profile.handle_for_string(func_name);
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
    category: Category<'static>,
    handle: Option<CategoryHandle>,
}

impl From<Category<'static>> for LazilyCreatedCategory {
    fn from(category: Category<'static>) -> Self {
        Self {
            category,
            handle: None,
        }
    }
}

impl LazilyCreatedCategory {
    pub fn get(&mut self, profile: &mut Profile) -> CategoryHandle {
        *self
            .handle
            .get_or_insert_with(|| profile.handle_for_category(self.category))
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
