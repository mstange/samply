use fxprof_processed_profile::{CategoryColor, CategoryPairHandle, Profile, StringHandle};

#[derive(Debug, Clone)]
pub struct JitCategoryManager {
    categories: [Option<CategoryPairHandle>; Self::CATEGORIES.len()],
}

impl JitCategoryManager {
    /// (prefix, name, color, is_js)
    const CATEGORIES: &'static [(&'static str, &'static str, CategoryColor, bool)] = &[
        ("JS:~", "Interpreter", CategoryColor::Red, true),
        ("Script:~", "Interpreter", CategoryColor::Red, true),
        ("JS:^", "Baseline", CategoryColor::Blue, true),
        ("JS:+", "Maglev", CategoryColor::LightGreen, true),
        ("JS:*", "Turbofan", CategoryColor::Green, true),
        ("Interpreter: ", "Interpreter", CategoryColor::Red, true),
        ("Baseline: ", "Baseline", CategoryColor::Blue, true),
        (
            "BaselineInterpreter: ",
            "BaselineInterpreter",
            CategoryColor::Brown,
            true,
        ),
        (
            "BaselineInterpreter",
            "BaselineInterpreter",
            CategoryColor::Brown,
            false,
        ),
        ("Ion: ", "Ion", CategoryColor::Green, true),
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
            categories: [None; Self::CATEGORIES.len()],
        }
    }

    /// Get the category and JS function name for a function from JIT code.
    ///
    /// The category is only created in the profile once a function with that
    /// category is encountered.
    pub fn classify_jit_symbol(
        &mut self,
        name: Option<&str>,
        profile: &mut Profile,
    ) -> (CategoryPairHandle, Option<StringHandle>) {
        let name = name.unwrap_or("");
        for (&(prefix, category_name, color, is_js), storage) in
            Self::CATEGORIES.iter().zip(self.categories.iter_mut())
        {
            if let Some(name_without_prefix) = name.strip_prefix(prefix) {
                let category = *storage
                    .get_or_insert_with(|| profile.add_category(category_name, color).into());

                let js_name = if is_js && !name.contains("(self-hosted:") {
                    // If the entire name was just the prefix (such as with "BaselineInterpreter"), use the unstripped name.
                    let canonicalized_name = if name_without_prefix.is_empty() {
                        name
                    } else {
                        name_without_prefix
                    };
                    Some(profile.intern_string(canonicalized_name))
                } else {
                    None
                };
                return (category, js_name);
            }
        }
        panic!("the last category has prefix '' so it should always be hit")
    }
}
