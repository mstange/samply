

pub fn parse_unk_size_null_utf16_string(v: &[u8]) -> String {
    // TODO: Make sure is aligned
    String::from_utf16_lossy(
        v.chunks_exact(2)
            .into_iter()
            .take_while(|&a| a[0] != 0 && a[1] == 0) // Take until null terminator
            .map(|a| u16::from_ne_bytes([a[0], a[1]]))
            .collect::<Vec<u16>>()
            .as_slice(),
    )
    .to_string()
}


pub fn parse_unk_size_null_unicode_size(v: &[u8]) -> usize {
    // TODO: Make sure is aligned
    v.chunks_exact(2)
    .into_iter()
    .take_while(|&a| a != &[0, 0]) // Take until null terminator
    .map(|a| u16::from_ne_bytes([a[0], a[1]]))
    .count() * 2 + 2
}

pub fn parse_unk_size_null_unicode_vec(v: &[u8]) -> Vec<u16> {
    // TODO: Make sure is aligned
    v.chunks_exact(2)
        .into_iter()
        .take_while(|&a| a[0] != 0 && a[1] == 0) // Take until null terminator
        .map(|a| u16::from_ne_bytes([a[0], a[1]]))
        .collect::<Vec<u16>>()
}

pub fn parse_unk_size_null_ansi_size(v: &[u8]) -> usize {
    v.into_iter()
        .take_while(|&&a| a != 0) // Take until null terminator
        .count() + 1
}

pub fn parse_unk_size_null_ansi_vec(v: &[u8]) -> Vec<u8> {
    v.into_iter()
        .take_while(|&&a| a != 0)// Take until null terminator
        .map(|&a| a) 
        .collect::<Vec<u8>>()
}

pub fn parse_null_utf16_string(v: &[u8]) -> String {
    String::from_utf16_lossy(
        v.chunks_exact(2)
            .into_iter()
            .map(|a| u16::from_ne_bytes([a[0], a[1]]))
            .collect::<Vec<u16>>()
            .as_slice(),
    )
    .trim_matches(char::default())
    .to_string()
}

pub fn parse_utf16_guid(v: &[u8]) -> String {
    String::from_utf16_lossy(
        v.chunks_exact(2)
            .into_iter()
            .map(|a| u16::from_ne_bytes([a[0], a[1]]))
            .collect::<Vec<u16>>()
            .as_slice(),
    )
    .trim_matches(char::default())
    .trim_matches('{')
    .trim_matches('}')
    .to_string()
}
