use fxprof_processed_profile::{CategoryColor, CategoryPairHandle, Profile};

#[derive(Debug, Clone)]
pub struct JitCategoryManager {
    categories: [Option<CategoryPairHandle>; Self::CATEGORIES.len()],
}

impl JitCategoryManager {
    /// (prefix, name, color)
    const CATEGORIES: &'static [(&'static str, &'static str, CategoryColor)] = &[
        ("JS:~", "Interpreter", CategoryColor::Red),
        ("JS:^", "Baseline", CategoryColor::Blue),
        ("JS:+", "Maglev", CategoryColor::LightGreen),
        ("JS:*", "Turbofan", CategoryColor::Green),
        ("Baseline: ", "Baseline", CategoryColor::Blue),
        ("Ion: ", "Ion", CategoryColor::Green),
        ("IC: ", "IC", CategoryColor::Brown),
        ("Trampoline: ", "Trampoline", CategoryColor::DarkGray),
        ("", "JIT", CategoryColor::Purple), // Generic fallback category for JIT code
    ];

    pub fn new() -> Self {
        Self {
            categories: [None; Self::CATEGORIES.len()],
        }
    }

    /// Get the category which should be used for the stack frame for a function
    /// from JIT code.
    ///
    /// The category is only created in the profile once a function with that
    /// category is encountered.
    pub fn get_category(
        &mut self,
        name: Option<&str>,
        profile: &mut Profile,
    ) -> CategoryPairHandle {
        let name = name.unwrap_or("");
        for (&(prefix, category_name, color), storage) in
            Self::CATEGORIES.iter().zip(self.categories.iter_mut())
        {
            if name.starts_with(prefix) {
                let category = *storage
                    .get_or_insert_with(|| profile.add_category(category_name, color).into());
                return category;
            }
        }
        panic!("the last category has prefix '' so it should always be hit")
    }
}
