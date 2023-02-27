use fxprof_processed_profile::{CategoryColor, CategoryPairHandle, FrameFlags, Profile};

#[derive(Debug, Clone)]
pub struct JitCategoryManager {
    categories: [Option<CategoryPairHandle>; Self::CATEGORIES.len()],
}

impl JitCategoryManager {
    /// (prefix, name, color, frame_flags)
    const CATEGORIES: &'static [(&'static str, &'static str, CategoryColor, FrameFlags)] = &[
        ("JS:~", "Interpreter", CategoryColor::Red, FrameFlags::IS_JS),
        ("JS:^", "Baseline", CategoryColor::Blue, FrameFlags::IS_JS),
        (
            "JS:+",
            "Maglev",
            CategoryColor::LightGreen,
            FrameFlags::IS_JS,
        ),
        ("JS:*", "Turbofan", CategoryColor::Green, FrameFlags::IS_JS),
        (
            "Baseline: ",
            "Baseline",
            CategoryColor::Blue,
            FrameFlags::IS_JS,
        ),
        ("Ion: ", "Ion", CategoryColor::Green, FrameFlags::IS_JS),
        ("IC: ", "IC", CategoryColor::Brown, FrameFlags::empty()),
        (
            "Trampoline: ",
            "Trampoline",
            CategoryColor::DarkGray,
            FrameFlags::empty(),
        ),
        (
            "Baseline JIT code for ",
            "Baseline",
            CategoryColor::Blue,
            FrameFlags::IS_JS,
        ),
        (
            "DFG JIT code for ",
            "DFG",
            CategoryColor::LightGreen,
            FrameFlags::IS_JS,
        ),
        (
            "FTL B3 code for ",
            "FTL",
            CategoryColor::Green,
            FrameFlags::IS_JS,
        ),
        ("", "JIT", CategoryColor::Purple, FrameFlags::empty()), // Generic fallback category for JIT code
    ];

    pub fn new() -> Self {
        Self {
            categories: [None; Self::CATEGORIES.len()],
        }
    }

    /// Get the category and flame flags which should be used for the stack
    /// frame for a function from JIT code.
    ///
    /// The category is only created in the profile once a function with that
    /// category is encountered.
    pub fn classify_jit_symbol(
        &mut self,
        name: Option<&str>,
        profile: &mut Profile,
    ) -> (CategoryPairHandle, FrameFlags) {
        let name = name.unwrap_or("");
        for (&(prefix, category_name, color, flags), storage) in
            Self::CATEGORIES.iter().zip(self.categories.iter_mut())
        {
            if name.starts_with(prefix) {
                let category = *storage
                    .get_or_insert_with(|| profile.add_category(category_name, color).into());
                return (category, flags);
            }
        }
        panic!("the last category has prefix '' so it should always be hit")
    }
}
