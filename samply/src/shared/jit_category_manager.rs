use fxprof_processed_profile::{
    Category, CategoryColor, CategoryHandle, Profile, StringHandle, SubcategoryHandle,
};

/// The script URL that SpiderMonkey reports for its built-in ("self-hosted")
/// functions.
const SELF_HOSTED_SCRIPT_URL: &str = "self-hosted";

/// The script a JIT'ed JS function was defined in, for data sources which report
/// it out-of-band instead of baking it into the symbol name.
#[derive(Debug, Clone, Copy)]
pub struct JsScriptSource<'a> {
    /// The script URL, e.g. `https://example.com/app.js`.
    pub url: &'a str,
}

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
    /// A function used in the implementation of built-in JS APIs such as
    /// filter / map / RegExp.test etc. In SpiderMonkey these are "self-hosted"
    /// JS functions; in V8 they're precompiled "builtins" that don't show up as
    /// JS functions at all. We don't surface these as JS frames, so we don't
    /// bother interning the name.
    Builtin,
    /// A JS function that is not shipped as part of a JS engine.
    NonBuiltin(StringHandle),
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
        // JSC categories.
        (
            "JSC-Baseline: ",
            Category("Baseline", CategoryColor::Blue),
            true,
        ),
        ("JSC-DFG: ", Category("DFG", CategoryColor::Magenta), true),
        ("JSC-FTL: ", Category("FTL", CategoryColor::Green), true),
        (
            "JSC-DFGOSREntry: ",
            Category("DFGOSREntry", CategoryColor::DarkGray),
            false,
        ),
        (
            "JSC-DFGOSRExit: ",
            Category("DFGOSRExit", CategoryColor::DarkGray),
            false,
        ),
        (
            "JSC-FTLOSRExit: ",
            Category("FTLOSRExit", CategoryColor::DarkGray),
            false,
        ),
        (
            "JSC-InlineCache: ",
            Category("IC", CategoryColor::Brown),
            false,
        ),
        (
            "JSC-JumpIsland: ",
            Category("JumpIsland", CategoryColor::DarkGray),
            false,
        ),
        (
            "JSC-Thunk: ",
            Category("Thunk", CategoryColor::DarkGray),
            false,
        ),
        (
            "JSC-LLIntThunk: ",
            Category("LLIntThunk", CategoryColor::DarkGray),
            false,
        ),
        (
            "JSC-DFGThunk: ",
            Category("DFGThunk", CategoryColor::DarkGray),
            false,
        ),
        (
            "JSC-FTLThunk: ",
            Category("FTLThunk", CategoryColor::DarkGray),
            false,
        ),
        (
            "JSC-WasmThunk: ",
            Category("WasmThunk", CategoryColor::DarkGray),
            false,
        ),
        (
            "JSC-ExtraCTIThunk: ",
            Category("ExtraCTIThunk", CategoryColor::DarkGray),
            false,
        ),
        ("JSC-WasmOMG: ", Category("OMG", CategoryColor::Green), true),
        ("JSC-WasmBBQ: ", Category("BBQ", CategoryColor::Blue), true),
        (
            "JSC-YarrJIT: ",
            Category("YarrJIT", CategoryColor::Red),
            false,
        ),
        (
            "JSC-CSSJIT: ",
            Category("CSSJIT", CategoryColor::Red),
            false,
        ),
        (
            "JSC-Uncategorized: ",
            Category("Uncategorized", CategoryColor::DarkGray),
            false,
        ),
        // V8 prefixes: https://source.chromium.org/chromium/chromium/src/+/main:v8/src/objects/code-kind.cc;l=21;drc=5fe0c7b2435bb2d92f434a81272b6752a5f34b9b
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
        (
            "JS:+'",
            Category("Maglev (context specialized)", CategoryColor::Green),
            true,
        ),
        ("JS:+", Category("Maglev", CategoryColor::Green), true),
        (
            "JS:o+",
            Category("Maglev (OSR)", CategoryColor::Green),
            true,
        ),
        (
            "JS:*'",
            Category("Turbofan (context specialized)", CategoryColor::Green),
            true,
        ),
        ("JS:*", Category("Turbofan", CategoryColor::Green), true),
        (
            "JS:o*",
            Category("Turbofan (OSR)", CategoryColor::Green),
            true,
        ),
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
    /// `script_source` is the script this function was defined in - pass `None`
    /// if unavailable. (E.g. ETW has it separately from the name, jitdump has it
    /// in the debug info (if present) but also puts it in the name.)
    ///
    /// The category is only created in the profile once a function with that
    /// category is encountered.
    pub fn classify_jit_symbol(
        &mut self,
        name: &str,
        script_source: Option<JsScriptSource<'_>>,
        profile: &mut Profile,
    ) -> (SubcategoryHandle, Option<JsFrame>) {
        if name == "BaselineInterpreter" || name.starts_with("BlinterpOp: ") {
            return (
                self.baseline_interpreter_category.get(profile).into(),
                Some(JsFrame::BaselineInterpreter),
            );
        }

        if let Some(js_func) = name.strip_prefix("BaselineInterpreter: ") {
            let js_func = JsFrame::BaselineInterpreterStub(Self::handle_for_js_name(
                profile,
                js_func,
                script_source,
            ));
            return (
                self.baseline_interpreter_category.get(profile).into(),
                Some(js_func),
            );
        }

        if let Some(ion_ic_rest) = name.strip_prefix("IonIC: ") {
            let category = self.ion_ic_category.get(profile);
            if let Some((_ic_type, js_func)) = ion_ic_rest.split_once(" : ") {
                let js_func = JsFrame::RegularInAdditionToNativeFrame(Self::handle_for_js_name(
                    profile,
                    js_func,
                    script_source,
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
                        Self::handle_for_js_name(profile, name_without_prefix, script_source),
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
                        Self::handle_for_js_name(profile, &new_name, script_source),
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

    fn handle_for_js_name(
        profile: &mut Profile,
        func_name: &str,
        script_source: Option<JsScriptSource<'_>>,
    ) -> JsName {
        if let Some(script_source) = script_source {
            // The data source told us which script this function came from, so we
            // don't have to guess.
            if script_source.url == SELF_HOSTED_SCRIPT_URL {
                return JsName::Builtin;
            }
            return JsName::NonBuiltin(profile.handle_for_string(func_name));
        }

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
                    // This is likely a builtin function.
                    return JsName::Builtin;
                }
                return JsName::NonBuiltin(profile.handle_for_string(&format!("{before}{after}")));
            }
        }

        // SpiderMonkey bakes the script location into the symbol name, so the
        // built-in script shows up as e.g. "filter (self-hosted:1234:5)".
        if func_name.contains("(self-hosted:")
            || func_name.ends_with("valueIsFalsey")
            || func_name.ends_with("valueIsTruthy")
        {
            return JsName::Builtin;
        }
        JsName::NonBuiltin(profile.handle_for_string(func_name))
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

    fn new_profile() -> Profile {
        Profile::new(
            "",
            ReferenceTimestamp::from_millis_since_unix_epoch(0.0),
            SamplingInterval::from_millis(1),
        )
    }

    #[test]
    fn test() {
        let mut manager = JitCategoryManager::new();
        let mut profile = new_profile();
        let (_category, js_name) = manager.classify_jit_symbol(
            "IonIC: SetElem : AccessibleButton (main.js:3560:25)",
            None,
            &mut profile,
        );
        match js_name {
            Some(JsFrame::RegularInAdditionToNativeFrame(JsName::NonBuiltin(s))) => {
                assert_eq!(profile.get_string(s), "AccessibleButton (main.js:3560:25)")
            }
            _ => panic!(),
        }
    }

    /// SpiderMonkey bakes the script location into the symbol name when it isn't
    /// reported separately, so we have to recognize it there.
    #[test]
    fn test_builtin_from_name() {
        let mut manager = JitCategoryManager::new();
        let mut profile = new_profile();
        let (_category, js_name) =
            manager.classify_jit_symbol("Ion: filter (self-hosted:1234:5)", None, &mut profile);
        assert!(matches!(
            js_name,
            Some(JsFrame::RegularInAdditionToNativeFrame(JsName::Builtin))
        ));
    }

    /// The Windows ETW path reports the script URL separately, so we don't have to
    /// guess from the name.
    #[test]
    fn test_builtin_from_script_source() {
        let mut manager = JitCategoryManager::new();
        let mut profile = new_profile();
        let (_category, js_name) = manager.classify_jit_symbol(
            "Ion: filter self-hosted:1234:5",
            Some(JsScriptSource { url: "self-hosted" }),
            &mut profile,
        );
        assert!(matches!(
            js_name,
            Some(JsFrame::RegularInAdditionToNativeFrame(JsName::Builtin))
        ));
    }

    /// A regular script is never treated as builtin, even if the function name
    /// happens to look like one of SpiderMonkey's built-ins.
    #[test]
    fn test_regular_script_source_wins_over_name() {
        let mut manager = JitCategoryManager::new();
        let mut profile = new_profile();
        let (_category, js_name) = manager.classify_jit_symbol(
            "Ion: valueIsTruthy https://example.com/app.js:12:3",
            Some(JsScriptSource {
                url: "https://example.com/app.js",
            }),
            &mut profile,
        );
        match js_name {
            Some(JsFrame::RegularInAdditionToNativeFrame(JsName::NonBuiltin(s))) => assert_eq!(
                profile.get_string(s),
                "valueIsTruthy https://example.com/app.js:12:3"
            ),
            _ => panic!(),
        }
    }
}
