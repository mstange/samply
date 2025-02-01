pub fn as_hex_string<S, T>(field: &T, serializer: S) -> std::result::Result<S::Ok, S::Error>
where
    S: serde::Serializer,
    T: std::fmt::LowerHex,
{
    serializer.collect_str(&format_args!("{field:#x}"))
}

pub fn as_optional_hex_string<S, T>(
    field: &Option<T>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error>
where
    S: serde::Serializer,
    T: std::fmt::LowerHex,
{
    match field {
        Some(field) => serializer.collect_str(&format_args!("{field:#x}")),
        None => serializer.serialize_none(),
    }
}

pub fn from_prefixed_hex_str<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Deserialize;
    let s = String::deserialize(deserializer)?;
    let s = if let Some(s) = s.strip_prefix("0x") {
        s
    } else {
        return Err(serde::de::Error::custom(format!(
            "Unexpected hex string {s} without 0x prefix."
        )));
    };
    u32::from_str_radix(s, 16).map_err(serde::de::Error::custom)
}
