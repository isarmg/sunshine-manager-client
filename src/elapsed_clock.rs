//! Elapsed time that includes machine sleep. Never substitute an active-time clock.
#[cfg(target_os = "linux")]
pub fn milliseconds() -> Option<u64> {
    parse_uptime(&std::fs::read_to_string("/proc/uptime").ok()?)
}
#[cfg(target_os = "linux")]
fn parse_uptime(value: &str) -> Option<u64> {
    let (whole, fraction) = value.split_whitespace().next()?.split_once('.')?;
    if fraction.is_empty() || !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut milliseconds = fraction
        .bytes()
        .take(3)
        .fold(0u64, |sum, byte| sum * 10 + u64::from(byte - b'0'));
    for _ in fraction.len().min(3)..3 {
        milliseconds *= 10;
    }
    whole
        .parse::<u64>()
        .ok()?
        .checked_mul(1000)?
        .checked_add(milliseconds)
}
#[cfg(target_os = "windows")]
pub fn milliseconds() -> Option<u64> {
    Some(unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() })
}
#[cfg(target_os = "macos")]
pub fn milliseconds() -> Option<u64> {
    #[repr(C)]
    struct Timebase {
        numerator: u32,
        denominator: u32,
    }
    unsafe extern "C" {
        fn mach_continuous_time() -> u64;
        fn mach_timebase_info(info: *mut Timebase) -> i32;
    }
    let mut info = Timebase {
        numerator: 0,
        denominator: 0,
    };
    if unsafe { mach_timebase_info(&mut info) } != 0 || info.denominator == 0 {
        return None;
    }
    let ticks = unsafe { mach_continuous_time() };
    u64::try_from(
        u128::from(ticks) * u128::from(info.numerator) / u128::from(info.denominator) / 1_000_000,
    )
    .ok()
}
#[cfg(all(test, target_os = "linux"))]
mod tests {
    #[test]
    fn uptime_conversion_is_bounded_and_preserves_elapsed_sleep_time() {
        assert_eq!(super::parse_uptime("123.45 456.00\n"), Some(123450));
        assert_eq!(super::parse_uptime("123.4 456.00\n"), Some(123400));
        assert_eq!(super::parse_uptime("123.456789 456.00\n"), Some(123456));
        assert!(super::parse_uptime("nan 0.00").is_none());
        assert!(super::parse_uptime("18446744073709551615.0 0.0").is_none());
        assert!(super::milliseconds().is_some());
    }
}
