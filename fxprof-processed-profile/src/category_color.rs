use std::io::Write;

use crate::writer::Writer;

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

impl CategoryColor {
    fn as_json_str(self) -> &'static str {
        match self {
            CategoryColor::Transparent => "transparent",
            CategoryColor::LightBlue => "lightblue",
            CategoryColor::Red => "red",
            CategoryColor::LightRed => "lightred",
            CategoryColor::Orange => "orange",
            CategoryColor::Blue => "blue",
            CategoryColor::Green => "green",
            CategoryColor::Purple => "purple",
            CategoryColor::Yellow => "yellow",
            CategoryColor::Brown => "brown",
            CategoryColor::Magenta => "magenta",
            CategoryColor::LightGreen => "lightgreen",
            CategoryColor::Gray => "grey",
            CategoryColor::DarkGray => "darkgray",
        }
    }

    pub(crate) fn write_json<W: Write>(self, w: &mut Writer<W>) -> std::io::Result<()> {
        w.string_value(self.as_json_str())
    }
}
