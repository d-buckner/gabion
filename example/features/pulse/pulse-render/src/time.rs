//! Epoch seconds to a readable UTC stamp.
//!
//! The render crate cannot read a clock: the only clock it is allowed to name
//! belongs to a capability bundle, and the bundle is the host's. So it formats
//! timestamps the contract already carries instead of asking for "now".

/// `2025-11-19 19:45Z` in UTC, from unix seconds. Hand-rolled so the render crate
/// stays free of time crates (and of `std::time`, which the gate bans). The `Z`
/// stays: the render crate has no clock and no zone, so UTC is the honest label.
pub(crate) fn stamp(at: u64) -> String {
    #[allow(clippy::cast_possible_truncation)] // day count of a u64 second count
    let days = (at / 86_400) as i64;
    let secs = at % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}Z",
        secs / 3_600,
        (secs / 60) % 60
    )
}

/// Howard Hinnant's days-from-civil, inverted. Exact for the whole unix range.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::stamp;

    #[test]
    fn known_instants() {
        assert_eq!(stamp(0), "1970-01-01 00:00Z");
        assert_eq!(stamp(1_000_000_000), "2001-09-09 01:46Z");
        assert_eq!(stamp(1_700_000_000), "2023-11-14 22:13Z");
    }
}
