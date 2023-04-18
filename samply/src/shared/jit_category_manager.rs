use fxprof_processed_profile::{
    CategoryColor, CategoryHandle, CategoryPairHandle, Profile, StringHandle,
};

#[derive(Debug, Clone, Copy)]
pub enum JsFrame {
    Regular(JsName),
    BaselineInterpreterStub(JsName),
    BaselineInterpreter,
}

#[derive(Debug, Clone, Copy)]
pub enum JsName {
    SelfHosted(StringHandle),
    NonSelfHosted(StringHandle),
}

#[derive(Debug, Clone)]
pub struct JitCategoryManager {
    categories: Vec<LazilyCreatedCategory>,
    baseline_interpreter_category: LazilyCreatedCategory,
    ion_ic_category: LazilyCreatedCategory,
}

impl JitCategoryManager {
    /// (prefix, name, color, is_js)
    const CATEGORIES: &'static [(&'static str, &'static str, CategoryColor, bool)] = &[
        ("JS:~", "Interpreter", CategoryColor::Magenta, true),
        ("Script:~", "Interpreter", CategoryColor::Magenta, true),
        ("JS:^", "Baseline", CategoryColor::Blue, true),
        ("JS:+", "Maglev", CategoryColor::LightGreen, true),
        ("JS:*", "Turbofan", CategoryColor::Green, true),
        ("Builtin:", "Builtin", CategoryColor::Brown, false),
        ("BytecodeHandler:", "Interpreter", CategoryColor::Red, false),
        ("Interpreter: ", "Interpreter", CategoryColor::Red, true),
        ("Baseline: ", "Baseline", CategoryColor::Blue, true),
        ("Ion: ", "Ion", CategoryColor::Green, true),
        ("BaselineIC: ", "BaselineIC", CategoryColor::Brown, false),
        ("IC: ", "IC", CategoryColor::Brown, false),
        ("Trampoline: ", "Trampoline", CategoryColor::DarkGray, false),
        (
            "Baseline JIT code for ",
            "Baseline",
            CategoryColor::Blue,
            true,
        ),
        ("DFG JIT code for ", "DFG", CategoryColor::LightGreen, true),
        ("FTL B3 code for ", "FTL", CategoryColor::Green, true),
        ("", "JIT", CategoryColor::Purple, false), // Generic fallback category for JIT code
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
        }
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
        if name == "BaselineInterpreter" {
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
                let js_func = JsFrame::Regular(Self::intern_js_name(profile, js_func));
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
                    Some(JsFrame::Regular(Self::intern_js_name(
                        profile,
                        name_without_prefix,
                    )))
                } else {
                    None
                };
                return (category.into(), js_name);
            }
        }
        panic!("the last category has prefix '' so it should always be hit")
    }

    fn intern_js_name(profile: &mut Profile, func_name: &str) -> JsName {
        let s = profile.intern_string(func_name);
        match func_name.contains("(self-hosted:") {
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
            Some(JsFrame::Regular(JsName::NonSelfHosted(s))) => {
                assert_eq!(profile.get_string(s), "AccessibleButton (main.js:3560:25)")
            }
            _ => panic!(),
        }
    }
}
