//! Message times in the user's local time zone, the way Discord shows them:
//! "Today at 19:05", "Yesterday at 19:05", otherwise "09/11/2026 19:05", and
//! "9 November 2026" on the divider between days.
//!
//! Discord sends ISO-8601 UTC timestamps; they're parsed once into unix
//! seconds (`Message::time`) and converted with `chrono::Local`, which reads
//! the OS time zone on Windows, macOS and Linux.

use chrono::{DateTime, Local, NaiveDate, TimeZone, Utc};

/// Unix seconds from Discord's ISO-8601 timestamp, or 0 if it doesn't parse.
pub fn parse_iso(ts: &str) -> i64 {
    DateTime::parse_from_rfc3339(ts)
        .map(|t| t.timestamp())
        .unwrap_or(0)
}

/// Unix seconds as a local date-time (0 / invalid → None).
pub fn local(unix: i64) -> Option<DateTime<Local>> {
    if unix <= 0 {
        return None;
    }
    Utc.timestamp_opt(unix, 0)
        .single()
        .map(|t| t.with_timezone(&Local))
}

/// Local calendar day of a timestamp (for date dividers).
pub fn local_day(unix: i64) -> Option<NaiveDate> {
    local(unix).map(|t| t.date_naive())
}

/// "HH:MM" in local time.
pub fn clock(unix: i64) -> String {
    local(unix)
        .map(|t| t.format("%H:%M").to_string())
        .unwrap_or_default()
}

/// "Today at 19:05" / "Yesterday at 19:05" / "09/11/2026 19:05".
pub fn full(unix: i64) -> String {
    match local(unix) {
        Some(t) => full_at(t, Local::now()),
        None => String::new(),
    }
}

/// `full` with an explicit "now" (any time zone), for tests.
pub fn full_at<Tz: TimeZone>(t: DateTime<Tz>, now: DateTime<Tz>) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let day = t.date_naive();
    let today = now.date_naive();
    let time = t.format("%H:%M");
    if day == today {
        format!("Today at {time}")
    } else if today.pred_opt() == Some(day) {
        format!("Yesterday at {time}")
    } else {
        format!("{} {time}", t.format("%d/%m/%Y"))
    }
}

/// "Monday, 9 November 2026 19:05" (timestamp tooltip).
pub fn long(unix: i64) -> String {
    local(unix)
        .map(|t| t.format("%A, %-d %B %Y %H:%M").to_string())
        .unwrap_or_default()
}

/// "9 November 2026" for the line between days.
pub fn day_label(day: NaiveDate) -> String {
    day.format("%-d %B %Y").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    #[test]
    fn parses_discord_timestamps() {
        assert_eq!(parse_iso("1970-01-01T00:01:00.000000+00:00"), 60);
        assert_eq!(parse_iso("2026-11-09T12:00:00Z"), 1_794_225_600);
        assert_eq!(parse_iso(""), 0);
        assert_eq!(parse_iso("nonsense"), 0);
        assert!(local(0).is_none());
    }

    #[test]
    fn relative_labels_use_the_local_day() {
        // UTC+2: 23:30 UTC on the 8th is 01:30 on the 9th locally.
        let tz = FixedOffset::east_opt(2 * 3600).unwrap();
        let now = tz.with_ymd_and_hms(2026, 11, 9, 19, 0, 0).unwrap();
        let utc_late = Utc
            .with_ymd_and_hms(2026, 11, 8, 23, 30, 0)
            .unwrap()
            .with_timezone(&tz);
        assert_eq!(full_at(utc_late, now), "Today at 01:30");
        let y = tz.with_ymd_and_hms(2026, 11, 8, 19, 5, 0).unwrap();
        assert_eq!(full_at(y, now), "Yesterday at 19:05");
        let old = tz.with_ymd_and_hms(2026, 10, 31, 7, 9, 0).unwrap();
        assert_eq!(full_at(old, now), "31/10/2026 07:09");
        // New year: yesterday crosses the year boundary.
        let ny = tz.with_ymd_and_hms(2027, 1, 1, 10, 0, 0).unwrap();
        let nye = tz.with_ymd_and_hms(2026, 12, 31, 23, 59, 0).unwrap();
        assert_eq!(full_at(nye, ny), "Yesterday at 23:59");
    }

    #[test]
    fn day_labels() {
        let d = NaiveDate::from_ymd_opt(2026, 11, 9).unwrap();
        assert_eq!(day_label(d), "9 November 2026");
    }
}
