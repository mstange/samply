fn is_aligned<T>(ptr: *const T) -> bool
where
    T: Sized,
{
    ptr as usize & (std::mem::align_of::<T>() - 1) == 0
}

pub fn parse_unk_size_null_utf16_string(v: &[u8]) -> String {
    // Instead of doing the following unsafe stuff ourselves we could
    // use cast_slice from bytemuck. Unfortunately, it won't work if
    // the length of the u8 is not a multiple of 2 vs. just truncating.
    // Alternatively, using safe_transmute::transmute_many_permisive should work.

    let start: *const u16 = v.as_ptr().cast();
    if !is_aligned(start) {
        panic!("Not aligned");
    }

    // safe because we not going past the end of the slice
    let end: *const u16 = unsafe { v.as_ptr().offset(v.len() as isize) }.cast();

    // find the null termination
    let mut len = 0;
    let mut ptr = start;
    while unsafe { *ptr } != 0 && ptr < end {
        len += 1;
        ptr = unsafe { ptr.offset(1) };
    }

    let slice = unsafe { std::slice::from_raw_parts(start, len) };
    String::from_utf16_lossy(slice)
}

pub fn parse_unk_size_null_unicode_size(v: &[u8]) -> usize {
    // TODO: Make sure is aligned
    v.chunks_exact(2)
        .into_iter()
        .take_while(|&a| a != &[0, 0]) // Take until null terminator
        .map(|a| u16::from_ne_bytes([a[0], a[1]]))
        .count()
        * 2
        + 2
}

pub fn parse_unk_size_null_unicode_vec(v: &[u8]) -> Vec<u16> {
    // TODO: Make sure is aligned
    v.chunks_exact(2)
        .into_iter()
        .take_while(|&a| a != &[0, 0]) // Take until null terminator
        .map(|a| u16::from_ne_bytes([a[0], a[1]]))
        .collect::<Vec<u16>>()
}

pub fn parse_unk_size_null_ansi_size(v: &[u8]) -> usize {
    v.into_iter()
        .take_while(|&&a| a != 0) // Take until null terminator
        .count()
        + 1
}

pub fn parse_unk_size_null_ansi_vec(v: &[u8]) -> Vec<u8> {
    v.into_iter()
        .take_while(|&&a| a != 0) // Take until null terminator
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
