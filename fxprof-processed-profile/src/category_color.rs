use serde::ser::{Serialize, Serializer};

/// One of the available colors for a category.
#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq)]
pub enum CategoryColor {
    Transparent,
    Purple,
    Green,
    Orange,
    Yellow,
    LightBlue,
    Grey,
    Blue,
    Brown,
    LightGreen,
    Red,
    LightRed,
    DarkGray,
}

impl Serialize for CategoryColor {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            CategoryColor::Transparent => "transparent".serialize(serializer),
            CategoryColor::Purple => "purple".serialize(serializer),
            CategoryColor::Green => "green".serialize(serializer),
            CategoryColor::Orange => "orange".serialize(serializer),
            CategoryColor::Yellow => "yellow".serialize(serializer),
            CategoryColor::LightBlue => "lightblue".serialize(serializer),
            CategoryColor::Grey => "grey".serialize(serializer),
            CategoryColor::Blue => "blue".serialize(serializer),
            CategoryColor::Brown => "brown".serialize(serializer),
            CategoryColor::LightGreen => "lightgreen".serialize(serializer),
            CategoryColor::Red => "red".serialize(serializer),
            CategoryColor::LightRed => "lightred".serialize(serializer),
            CategoryColor::DarkGray => "darkgray".serialize(serializer),
        }
    }
}
