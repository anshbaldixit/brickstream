//! Timestamps (rules.md "Timestamps"): F1 `YYYY-MM-DDTHH:MM:SSZ`, F2 `YYYY-MM-DDTHH:MM:SS±HH:MM`,
//! F3 ASCII digits = epoch milliseconds. Everything else is bad.
//!
//! The form is chosen by content, not length: a value made only of ASCII digits is F3 whatever its
//! length (a zero-padded F3 can be 20 or 25 bytes long), and only a value with the separators in place
//! can be F1 or F2. Calendar values are checked, never normalized: month 1-12, day within the month
//! (Feb 29 only in leap years), hour <= 23, minute and second <= 59; an F2 offset needs hours 00-23,
//! minutes 00-59 and the colon. An F3 value too large for i64 is outside every window, never wrapped.

pub const MS_PER_SECOND: i64 = 1_000;
pub const MS_PER_DAY: i64 = 86_400_000;

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's days_from_civil).
#[inline(always)]
pub fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[inline(always)]
fn is_leap(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

#[inline(always)]
fn days_in_month(year: i64, month: i64) -> i64 {
    const DAYS: [u8; 13] = [0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if month == 2 && is_leap(year) { 29 } else { DAYS[month as usize] as i64 }
}

#[inline(always)]
fn digit(b: u8) -> (i64, bool) {
    let d = b.wrapping_sub(b'0');
    (d as i64, d < 10)
}

#[inline(always)]
fn two(b: &[u8], at: usize) -> (i64, bool) {
    let (a, ok_a) = digit(b[at]);
    let (c, ok_c) = digit(b[at + 1]);
    (a * 10 + c, ok_a & ok_c)
}

/// Seconds since the epoch of the local date-time in `b[..19]` (`YYYY-MM-DDTHH:MM:SS`), checked.
#[inline(always)]
fn local_seconds(b: &[u8]) -> Option<i64> {
    let (y1, ok1) = two(b, 0);
    let (y2, ok2) = two(b, 2);
    let (month, ok3) = two(b, 5);
    let (day, ok4) = two(b, 8);
    let (hour, ok5) = two(b, 11);
    let (minute, ok6) = two(b, 14);
    let (second, ok7) = two(b, 17);
    let seps = (b[4] == b'-') & (b[7] == b'-') & (b[10] == b'T') & (b[13] == b':') & (b[16] == b':');
    if !(ok1 & ok2 & ok3 & ok4 & ok5 & ok6 & ok7 & seps) {
        return None;
    }
    let year = y1 * 100 + y2;
    if !(1..=12).contains(&month) || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    if day < 1 || day > days_in_month(year, month) {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second)
}

/// F1 as whole seconds (the fast path for `ingest_ts`).
#[inline(always)]
pub fn parse_f1_seconds(b: &[u8]) -> Option<i64> {
    if b.len() != 20 || b[19] != b'Z' {
        return None;
    }
    local_seconds(b)
}

#[inline(always)]
fn parse_f2_ms(b: &[u8]) -> Option<i64> {
    let sign = match b[19] {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let (oh, ok_h) = two(b, 20);
    let (om, ok_m) = two(b, 23);
    if !(ok_h & ok_m) || b[22] != b':' || oh > 23 || om > 59 {
        return None;
    }
    let local = local_seconds(b)?;
    Some((local - sign * (oh * 3_600 + om * 60)) * MS_PER_SECOND)
}

/// F3: digits only, any length, leading zeros allowed. Values over i64::MAX saturate (outside every
/// window). Returns `None` if any byte is not an ASCII digit or the value is empty.
#[inline(always)]
fn parse_f3_ms(b: &[u8]) -> Option<i64> {
    if b.is_empty() {
        return None;
    }
    let mut value: u64 = 0;
    let mut overflow = false;
    for &c in b {
        let d = c.wrapping_sub(b'0');
        if d >= 10 {
            return None;
        }
        match value.checked_mul(10).and_then(|v| v.checked_add(d as u64)) {
            Some(v) => value = v,
            None => overflow = true,
        }
    }
    if overflow || value > i64::MAX as u64 { Some(i64::MAX) } else { Some(value as i64) }
}

/// Any `ts` value to epoch milliseconds, or `None` if it is a bad timestamp.
#[inline(always)]
pub fn parse_ts(b: &[u8]) -> Option<i64> {
    match b.len() {
        20 if b[19] == b'Z' => local_seconds(b).map(|s| s * MS_PER_SECOND),
        25 if b[19] == b'+' || b[19] == b'-' => parse_f2_ms(b),
        _ => parse_f3_ms(b),
    }
}

/// `ingest_ts` in whole seconds. The rules promise valid F1; anything else falls back to the
/// general parser (floored to a second), and `None` only if it is no timestamp at all.
#[inline(always)]
pub fn parse_ingest_seconds(b: &[u8]) -> Option<i64> {
    match parse_f1_seconds(b) {
        Some(s) => Some(s),
        None => parse_ts(b).map(|ms| ms.div_euclid(MS_PER_SECOND)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(s: &str) -> Option<i64> {
        parse_ts(s.as_bytes())
    }

    /// A slow, obviously-correct civil-to-days for cross-checking: count days year by year.
    fn slow_days(year: i64, month: i64, day: i64) -> i64 {
        let mut days = 0i64;
        if year >= 1970 {
            for y in 1970..year {
                days += if is_leap(y) { 366 } else { 365 };
            }
        } else {
            for y in year..1970 {
                days -= if is_leap(y) { 366 } else { 365 };
            }
        }
        for m in 1..month {
            days += days_in_month(year, m);
        }
        days + day - 1
    }

    #[test]
    fn days_from_civil_matches_slow_count() {
        for year in [0, 1, 399, 400, 1600, 1900, 1969, 1970, 2000, 2024, 2025, 2027, 2028, 2100, 9999] {
            for month in 1..=12 {
                for day in [1, 15, days_in_month(year, month)] {
                    assert_eq!(days_from_civil(year, month, day), slow_days(year, month, day), "{year}-{month}-{day}");
                }
            }
        }
    }

    #[test]
    fn rules_examples() {
        let f1 = ms("2026-10-03T23:58:10Z").unwrap();
        assert_eq!(f1, 1_791_071_890_000);
        assert_eq!(ms("2026-10-03T19:58:10-04:00"), Some(f1));
        assert_eq!(ms("1791071890000"), Some(f1));
        assert_eq!(ms("0001791071890000"), Some(f1));
        assert_eq!(ms("86400000"), Some(86_400_000));
        // E7: the offset moves the date back.
        assert_eq!(ms("2026-10-04T01:30:00+02:00"), ms("2026-10-03T23:30:00Z"));
        // T2: the sign covers the minutes too.
        assert_eq!(ms("2026-10-03T10:00:00-03:30"), ms("2026-10-03T13:30:00Z"));
        assert_eq!(ms("2026-10-03T10:00:00+05:45"), ms("2026-10-03T04:15:00Z"));
        assert_eq!(ms("2026-10-03T10:00:00-00:00"), ms("2026-10-03T10:00:00Z"));
        assert_eq!(ms("2026-10-03T10:00:00+23:59"), ms("2026-10-02T10:01:00Z"));
    }

    #[test]
    fn zero_padded_f3_of_f1_and_f2_lengths() {
        // All-digit values of length 20 and 25 are F3, and can be in the window.
        let v = 1_791_021_600_000i64;
        let s20 = format!("{v:020}");
        let s25 = format!("{v:025}");
        assert_eq!(s20.len(), 20);
        assert_eq!(s25.len(), 25);
        assert_eq!(ms(&s20), Some(v));
        assert_eq!(ms(&s25), Some(v));
        assert_eq!(ms(&format!("{:0>50}", v)), Some(v));
    }

    #[test]
    fn bad_forms() {
        for bad in [
            "",
            "2026-10-03T10:00:00.123Z",
            "2026-10-03T10:00:00z",
            "2026-10-03t10:00:00Z",
            "2026-10-03 10:00:00Z",
            "2026-10-03 10:00:00",
            "2026-10-03T10:00:00",
            "1791071890000.0",
            "1.79e12",
            "-5",
            "+1791021600000",
            " 1791021600000",
            "1791021600000 ",
            "2026-13-03T10:00:00Z",
            "2026-02-30T10:00:00Z",
            "2027-02-29T10:00:00Z",
            "2026-09-31T10:00:00Z",
            "2026-10-00T10:00:00Z",
            "2026-00-10T10:00:00Z",
            "2026-10-03T24:00:00Z",
            "2026-10-03T10:60:00Z",
            "2026-10-03T10:00:60Z",
            "2026-10-03T10:00:00+24:00",
            "2026-10-03T10:00:00-24:00",
            "2026-10-03T10:00:00+05:60",
            "2026-10-03T10:00:00+0200",
            "2026-10-03T10:00:00+5:30",
            "2026-10-03T10:00:00+05",
            "2026-10-03T10:00:00Z\r",
            "1791071890000\r",
            "\u{0661}\u{0667}\u{0669}\u{0661}\u{0660}\u{0667}\u{0661}\u{0668}\u{0669}\u{0660}\u{0660}\u{0660}\u{0660}",
            "\u{FF12}026-10-03T10:00:00Z",
            "2026-10-03T10:00:00 ",
            " 2026-10-03T10:00:00Z",
        ] {
            assert_eq!(ms(bad), None, "{bad:?} should be bad");
        }
    }

    #[test]
    fn leap_days_and_overflow() {
        assert_eq!(ms("2028-02-29T00:00:00Z"), Some(days_from_civil(2028, 2, 29) * MS_PER_DAY));
        assert_eq!(ms("2000-02-29T12:00:00Z").is_some(), true);
        assert_eq!(ms("1900-02-29T12:00:00Z"), None);
        // 21 digits and 2^64 + an in-window instant: outside every window, never wrapped.
        assert_eq!(ms("179102160000000000000"), Some(i64::MAX));
        assert_eq!(ms("18446745864731151616"), Some(i64::MAX));
        assert_eq!(ms("9223372036854775807"), Some(i64::MAX));
        assert_eq!(ms("9223372036854775808"), Some(i64::MAX));
    }

    #[test]
    fn random_roundtrip_against_slow_formatting() {
        let mut x: u64 = 0x1234_5678_9abc_def1;
        for _ in 0..200_000 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            // Instants from 1900 to 2100, whole seconds.
            let secs = (x % (200 * 365 * 86_400)) as i64 - 70 * 365 * 86_400;
            let days = secs.div_euclid(86_400);
            let sod = secs.rem_euclid(86_400);
            // civil from days by search (slow but obviously right).
            let mut year = 1970 + days.div_euclid(366) - 1;
            while slow_days(year + 1, 1, 1) <= days {
                year += 1;
            }
            let mut month = 1;
            while month < 12 && slow_days(year, month + 1, 1) <= days {
                month += 1;
            }
            let day = days - slow_days(year, month, 1) + 1;
            let f1 = format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", sod / 3600, sod / 60 % 60, sod % 60);
            assert_eq!(ms(&f1), Some(secs * 1000), "{f1}");
            assert_eq!(parse_f1_seconds(f1.as_bytes()), Some(secs));
            let offset_min = (x >> 40) as i64 % (24 * 60) * if x & 1 == 0 { 1 } else { -1 };
            // F2 of the same instant: local = utc + offset.
            let local = secs + offset_min * 60;
            let ldays = local.div_euclid(86_400);
            let lsod = local.rem_euclid(86_400);
            let mut ly = 1970 + ldays.div_euclid(366) - 1;
            while slow_days(ly + 1, 1, 1) <= ldays {
                ly += 1;
            }
            let mut lm = 1;
            while lm < 12 && slow_days(ly, lm + 1, 1) <= ldays {
                lm += 1;
            }
            let ld = ldays - slow_days(ly, lm, 1) + 1;
            let sign = if offset_min < 0 { '-' } else { '+' };
            let f2 = format!(
                "{ly:04}-{lm:02}-{ld:02}T{:02}:{:02}:{:02}{sign}{:02}:{:02}",
                lsod / 3600,
                lsod / 60 % 60,
                lsod % 60,
                offset_min.abs() / 60,
                offset_min.abs() % 60
            );
            assert_eq!(ms(&f2), Some(secs * 1000), "{f2}");
            if secs >= 0 {
                let f3 = format!("{}", secs * 1000 + (x % 1000) as i64);
                assert_eq!(ms(&f3), Some(secs * 1000 + (x % 1000) as i64));
            }
        }
    }
}
