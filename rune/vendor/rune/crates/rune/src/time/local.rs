//! An instant as the user's wall clock reads it: the OS time zone, with daylight saving and its
//! offset from UTC at that instant. The OS gives the offset; the calendar math is plain Rust, so
//! every platform splits an instant into fields the same way.

/// The date and time a clock shows for an instant, and that clock's offset from UTC then.
/// [`LocalTime::at`] reads the user's time zone. Fields may be added, so build one with
/// [`LocalTime::at`] or [`LocalTime::at_offset`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct LocalTime {
    pub year: i32,
    /// 1–12.
    pub month: u32,
    /// 1–31.
    pub day: u32,
    /// 0–23.
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    /// Seconds east of UTC, daylight saving included.
    pub offset: i32,
}

impl LocalTime {
    /// `epoch_ms` (milliseconds since 1970-01-01 UTC) in the OS time zone, with the daylight
    /// saving rules of that instant's own year: `localtime_r` on Unix, the dynamic time zone on
    /// Windows, `Date` on the web. UTC where the OS has no answer (an instant outside its range,
    /// or a platform without time zones).
    pub fn at(epoch_ms: i64) -> LocalTime {
        LocalTime::at_offset(epoch_ms, offset_at(epoch_ms.div_euclid(1000)).unwrap_or(0))
    }

    /// `epoch_ms` on a clock `offset` seconds east of UTC, whatever the OS time zone is: for
    /// stable output in tests and headless captures.
    pub fn at_offset(epoch_ms: i64, offset: i32) -> LocalTime {
        // |epoch_ms / 1000| is under 2^54, so adding an offset can't overflow.
        let secs = epoch_ms.div_euclid(1000) + i64::from(offset);
        let (days, sod) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
        // Days to a proleptic Gregorian date, from Howard Hinnant's `civil_from_days`.
        let z = days + 719_468;
        let (era, doe) = (z.div_euclid(146_097), z.rem_euclid(146_097));
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        // About 292 million years either way at most, well inside i32.
        let year = (era * 400 + yoe + i64::from(month <= 2)) as i32;
        LocalTime {
            year,
            month: month as u32,
            day: (doy - (153 * mp + 2) / 5 + 1) as u32,
            hour: (sod / 3600) as u32,
            minute: (sod % 3600 / 60) as u32,
            second: (sod % 60) as u32,
            offset,
        }
    }
}

/// The OS time zone's offset east of UTC, in seconds, at `secs` since the Unix epoch.
#[cfg(unix)]
// `time_t` is 32 bits on some targets.
#[allow(clippy::useless_conversion, clippy::unnecessary_fallible_conversions)]
fn offset_at(secs: i64) -> Option<i32> {
    let secs = libc::time_t::try_from(secs).ok()?;
    // SAFETY: `libc::tm` is a plain C struct where all-zero bytes are valid.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: `localtime_r` reads `secs` and writes only into `tm`, both of which outlive the
    // call. It returns null when the year doesn't fit `tm`.
    let ok = unsafe { !libc::localtime_r(&secs, &mut tm).is_null() };
    ok.then_some(tm.tm_gmtoff as i32)
}

/// The OS time zone's offset east of UTC, in seconds, at `secs` since the Unix epoch. The dynamic
/// time zone carries the zone's yearly daylight saving rules, so a past or future instant gets
/// the offset of its own year.
#[cfg(windows)]
fn offset_at(secs: i64) -> Option<i32> {
    use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows_sys::Win32::System::Time::{
        DYNAMIC_TIME_ZONE_INFORMATION, FileTimeToSystemTime, GetDynamicTimeZoneInformation, SystemTimeToFileTime,
        SystemTimeToTzSpecificLocalTimeEx, TIME_ZONE_ID_INVALID,
    };

    // FILETIME counts 100 ns ticks from 1601-01-01 UTC; it has no ticks before that.
    const EPOCH_TICKS: i64 = 116_444_736_000_000_000;
    let utc_ticks = secs.checked_mul(10_000_000).and_then(|t| t.checked_add(EPOCH_TICKS)).filter(|&t| t >= 0)?;
    let utc_file = FILETIME { dwLowDateTime: utc_ticks as u32, dwHighDateTime: (utc_ticks >> 32) as u32 };
    // SAFETY: plain C structs where all-zero bytes are valid.
    let (mut utc, mut wall, mut zone, mut wall_file): (SYSTEMTIME, SYSTEMTIME, DYNAMIC_TIME_ZONE_INFORMATION, FILETIME) =
        unsafe { std::mem::zeroed() };
    // SAFETY: each call reads and writes only the structs passed to it, which outlive the calls.
    // FileTimeToSystemTime fails past year 30827, so `wall_file` stays well inside i64.
    let ok = unsafe {
        FileTimeToSystemTime(&utc_file, &mut utc) != 0
            && GetDynamicTimeZoneInformation(&mut zone) != TIME_ZONE_ID_INVALID
            && SystemTimeToTzSpecificLocalTimeEx(&zone, &utc, &mut wall) != 0
            && SystemTimeToFileTime(&wall, &mut wall_file) != 0
    };
    let wall_ticks = ((u64::from(wall_file.dwHighDateTime) << 32) | u64::from(wall_file.dwLowDateTime)) as i64;
    ok.then(|| ((wall_ticks - utc_ticks) / 10_000_000) as i32)
}

/// The browser's time zone offset east of UTC, in seconds, at `secs` since the Unix epoch.
#[cfg(all(target_arch = "wasm32", not(unix)))]
fn offset_at(secs: i64) -> Option<i32> {
    // `Date` holds ±8.64e15 ms; past that, its offset is NaN.
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(secs as f64 * 1000.0));
    let minutes_west = date.get_timezone_offset();
    minutes_west.is_finite().then(|| (-minutes_west * 60.0) as i32)
}

#[cfg(not(any(unix, windows, target_arch = "wasm32")))]
fn offset_at(_secs: i64) -> Option<i32> {
    None
}
