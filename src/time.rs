//! UTC times in the form `2026-11-12T10:00:00Z`, with no time library.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Formats a time as UTC, to the second.
pub fn utc(t: SystemTime) -> String {
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// A time as Unix seconds, to the millisecond, as the field `at` of a record.
pub fn unix(t: SystemTime) -> f64 {
    let ms = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    ms as f64 / 1000.0
}

/// The time of a field `at`, or None when it is not a time.
pub fn from_unix(at: f64) -> Option<SystemTime> {
    (at.is_finite() && at >= 0.0)
        .then(|| UNIX_EPOCH + Duration::from_millis((at * 1000.0).round() as u64))
}

/// A length of time in seconds, to the millisecond.
pub fn seconds(d: Duration) -> f64 {
    d.as_millis() as f64 / 1000.0
}

/// The seconds from `from` to `to`, both fields `at`, to the millisecond. Never below 0.
pub fn between(from: f64, to: f64) -> f64 {
    (((to - from) * 1000.0).round() / 1000.0).max(0.0)
}

/// The civil date of a count of days since 1970-01-01, by Howard Hinnant's algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn at(secs: u64) -> String {
        utc(UNIX_EPOCH + Duration::from_secs(secs))
    }

    #[test]
    fn formats_known_times() {
        assert_eq!(at(0), "1970-01-01T00:00:00Z");
        assert_eq!(at(1_731_405_600), "2024-11-12T10:00:00Z");
        assert_eq!(at(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(at(4_107_542_399), "2100-02-28T23:59:59Z");
    }

    #[test]
    fn unix_times_and_lengths_keep_the_milliseconds() {
        let t = UNIX_EPOCH + Duration::from_micros(1_794_498_589_123_456);
        assert_eq!(unix(t), 1_794_498_589.123);
        assert_eq!(utc(from_unix(1_794_498_589.123).unwrap()), utc(t));
        assert_eq!(from_unix(-1.0), None);
        assert_eq!(from_unix(f64::NAN), None);
        assert_eq!(seconds(Duration::from_micros(495_250_900)), 495.25);
        assert_eq!(between(10.5, 506.25), 495.75);
        assert_eq!(between(10.0, 9.0), 0.0);
        assert_eq!(between(0.1, 0.3), 0.2);
    }
}
