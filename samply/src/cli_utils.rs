use std::ffi::OsStr;

#[allow(unused)]
pub fn parse_time_range(
    arg: &str,
) -> Result<(std::time::Duration, std::time::Duration), humantime::DurationError> {
    let (is_duration, splitchar) = if arg.contains('+') {
        (true, '+')
    } else {
        (false, '-')
    };

    let parts: Vec<&str> = arg.splitn(2, splitchar).collect();

    let start = if parts[0].is_empty() {
        std::time::Duration::ZERO
    } else {
        humantime::parse_duration(parts[0])?
    };

    let end = if parts.len() == 1 || parts[1].is_empty() {
        std::time::Duration::MAX
    } else {
        humantime::parse_duration(parts[1])?
    };

    Ok((start, if is_duration { start + end } else { end }))
}

pub fn split_at_first_equals(s: &OsStr) -> Option<(&OsStr, &OsStr)> {
    let bytes = s.as_encoded_bytes();
    let pos = bytes.iter().position(|b| *b == b'=')?;
    let name = &bytes[..pos];
    let val = &bytes[(pos + 1)..];
    // SAFETY:
    // - `name` and `val` only contain content that originated from `OsStr::as_encoded_bytes`
    // - Only split with ASCII '=' which is a non-empty UTF-8 substring
    let (name, val) = unsafe {
        (
            OsStr::from_encoded_bytes_unchecked(name),
            OsStr::from_encoded_bytes_unchecked(val),
        )
    };
    Some((name, val))
}
