use serde::ser::{Serialize, Serializer};

/// One of the named colors recognized by the Firefox Profiler for categories.
///
/// The exact color value is determined by the UI.
///
/// By convention, the "Other" category uses [`CategoryColor::Gray`] — see
/// [`Category::OTHER`](crate::Category::OTHER).
///
/// [`CategoryColor::Transparent`] can be used for activity which should not
/// show up in the activity graph in the UI, for example for blocking "wait for
/// next event" functions in an event loop.
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub enum CategoryColor {
    /// No color; the category does not paint over the default background.
    Transparent,
    /// Light blue.
    LightBlue,
    /// Red.
    Red,
    /// Light red.
    LightRed,
    /// Orange.
    Orange,
    /// Blue.
    Blue,
    /// Green.
    Green,
    /// Purple.
    Purple,
    /// Yellow.
    Yellow,
    /// Brown.
    Brown,
    /// Magenta.
    Magenta,
    /// Light green.
    LightGreen,
    /// Gray. Used by the default "Other" category, see [`Category::OTHER`](crate::Category::OTHER).
    Gray,
    /// Dark gray.
    DarkGray,
}

impl Serialize for CategoryColor {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            CategoryColor::Transparent => "transparent".serialize(serializer),
            CategoryColor::LightBlue => "lightblue".serialize(serializer),
            CategoryColor::Red => "red".serialize(serializer),
            CategoryColor::LightRed => "lightred".serialize(serializer),
            CategoryColor::Orange => "orange".serialize(serializer),
            CategoryColor::Blue => "blue".serialize(serializer),
            CategoryColor::Green => "green".serialize(serializer),
            CategoryColor::Purple => "purple".serialize(serializer),
            CategoryColor::Yellow => "yellow".serialize(serializer),
            CategoryColor::Brown => "brown".serialize(serializer),
            CategoryColor::Magenta => "magenta".serialize(serializer),
            CategoryColor::LightGreen => "lightgreen".serialize(serializer),
            CategoryColor::Gray => "grey".serialize(serializer),
            CategoryColor::DarkGray => "darkgray".serialize(serializer),
        }
    }
}
