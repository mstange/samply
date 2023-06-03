use mach::mach_time;
use once_cell::sync::OnceCell;

static NANOS_PER_TICK: OnceCell<mach_time::mach_timebase_info> = OnceCell::new();

pub fn get_monotonic_timestamp() -> u64 {
    let nanos_per_tick = NANOS_PER_TICK.get_or_init(|| unsafe {
        let mut info = mach_time::mach_timebase_info::default();
        let errno = mach_time::mach_timebase_info(&mut info as *mut _);
        if errno != 0 || info.denom == 0 {
            info.numer = 1;
            info.denom = 1;
        };
        info
    });

    let time = unsafe { mach_time::mach_absolute_time() };

    time * nanos_per_tick.numer as u64 / nanos_per_tick.denom as u64
}
