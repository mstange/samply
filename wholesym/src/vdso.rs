/// Returns the memory address range in this process where the VDSO is mapped.
pub fn get_vdso_range() -> Option<(usize, usize)> {
    let proc_maps_file = std::fs::File::open("/proc/self/maps").ok()?;
    use std::io::{BufRead, BufReader};
    let mut lines = BufReader::new(proc_maps_file).lines().map_while(Result::ok);
    // "ffffa613c000-ffffa613d000 r-xp 00000000 00:00 0                          [vdso]"
    let proc_maps_vdso_line = lines.find(|l| l.ends_with("[vdso]"))?;
    let (start, end) = proc_maps_vdso_line.split_once(' ')?.0.split_once('-')?;
    Some((
        usize::from_str_radix(start, 16).ok()?,
        usize::from_str_radix(end, 16).ok()?,
    ))
}

/// Returns the in-memory slice that contains the VDSO, if found.
pub fn get_vdso_data() -> Option<&'static [u8]> {
    let (start, end) = get_vdso_range()?;
    let len = end.checked_sub(start)?;
    // Make a slice around the vdso contents.
    //
    // Safety: the address range came from /proc/self/maps and the VDSO mapping
    // does not change throughout the lifetime of the process. It contains immutable
    // and initial data.
    Some(unsafe { core::slice::from_raw_parts(start as *const u8, len) })
}
