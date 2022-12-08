use std::fmt::Display;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HexPfxLowerU32(pub u32);

impl Serialize for HexPfxLowerU32 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_str(&format_args!("{:#x}", self.0))
    }
}

impl<'de> Deserialize<'de> for HexPfxLowerU32 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        let s = if let Some(s) = s.strip_prefix("0x") {
            s
        } else {
            return Err(serde::de::Error::custom(format!(
                "Unexpected hex string {s} without 0x prefix."
            )));
        };
        let x = u32::from_str_radix(s, 16).map_err(serde::de::Error::custom)?;
        Ok(Self(x))
    }
}

impl Display for HexPfxLowerU32 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl From<HexPfxLowerU32> for u32 {
    fn from(hex: HexPfxLowerU32) -> Self {
        hex.0
    }
}

impl From<u32> for HexPfxLowerU32 {
    fn from(num: u32) -> Self {
        Self(num)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_hex() {
        assert_eq!(
            &serde_json::to_string_pretty(&HexPfxLowerU32(254)).unwrap(),
            "\"0xfe\""
        );
        assert_eq!(
            serde_json::from_str::<HexPfxLowerU32>("\"0xfe\"").unwrap(),
            HexPfxLowerU32(254)
        );
        assert!(serde_json::from_str::<HexPfxLowerU32>("\"fe\"").is_err(),);
    }
}
