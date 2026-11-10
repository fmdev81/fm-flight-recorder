//! The server time zone. Event times arrive in UTC; "today", hours and days
//! are computed in one zone for the whole server (config `tz`, else the
//! zone of this machine).

use chrono::{DateTime, Duration, LocalResult, NaiveDate, Offset, TimeZone, Utc};
use chrono_tz::Tz;

#[derive(Debug, Clone, Copy)]
pub struct Zone {
    pub tz: Tz,
}

pub const HOUR_MS: i64 = 3_600_000;
pub const DAY_MS: i64 = 86_400_000;

impl Zone {
    /// `name` is an IANA name. Empty = the zone of this machine (UTC if it
    /// cannot be read).
    pub fn new(name: &str) -> Result<Zone, String> {
        let name = if name.trim().is_empty() {
            iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".into())
        } else {
            name.trim().to_string()
        };
        name.parse::<Tz>()
            .map(|tz| Zone { tz })
            .map_err(|_| format!("unknown time zone: {name}"))
    }

    pub fn name(&self) -> &'static str {
        self.tz.name()
    }

    fn at(&self, ms: i64) -> DateTime<Tz> {
        Utc.timestamp_millis_opt(ms)
            .single()
            .unwrap_or_default()
            .with_timezone(&self.tz)
    }

    /// The local calendar date of a UTC time.
    pub fn date_of(&self, ms: i64) -> NaiveDate {
        self.at(ms).date_naive()
    }

    /// UTC ms of the local midnight that starts `d`. If the zone skips
    /// midnight (a DST change at 00:00), the first local time of the day.
    pub fn day_start(&self, d: NaiveDate) -> i64 {
        let mut t = d.and_hms_opt(0, 0, 0).unwrap();
        for _ in 0..4 {
            match self.tz.from_local_datetime(&t) {
                LocalResult::Single(x) => return x.timestamp_millis(),
                LocalResult::Ambiguous(a, _) => return a.timestamp_millis(),
                LocalResult::None => t += Duration::minutes(30),
            }
        }
        Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0).unwrap())
            .timestamp_millis()
    }

    /// UTC ms of the end of `d` (the start of the next day).
    pub fn day_end(&self, d: NaiveDate) -> i64 {
        self.day_start(d.succ_opt().unwrap_or(d))
    }

    /// Length of the local day in hours (23, 24 or 25).
    pub fn day_hours(&self, d: NaiveDate) -> i64 {
        (self.day_end(d) - self.day_start(d)) / HOUR_MS
    }

    /// The starts of the hours from `start` (a local midnight) up to `end`.
    /// On a DST day there are 23 or 25 of them for the whole day.
    pub fn hour_starts(&self, start: i64, end: i64) -> Vec<i64> {
        let mut v = Vec::new();
        let mut t = start;
        while t < end {
            v.push(t);
            t += HOUR_MS;
        }
        v
    }

    pub fn fmt(&self, ms: i64, pattern: &str) -> String {
        self.at(ms).format(pattern).to_string()
    }

    /// "HH:MM:SS" local.
    pub fn hms(&self, ms: i64) -> String {
        self.fmt(ms, "%H:%M:%S")
    }

    /// The zone abbreviation at `ms`: "CST", "CDT". Zones with no letters
    /// give "UTC−06:00".
    pub fn abbr(&self, ms: i64) -> String {
        let a = self.fmt(ms, "%Z");
        if a.chars().any(|c| c.is_ascii_alphabetic()) {
            a
        } else {
            self.offset_label(ms)
        }
    }

    /// The one time format of the pages: "Today 15:54:22" for today,
    /// "Nov 9, 15:54:22" for another day of this year, "Dec 30 2025, 15:54:22"
    /// for an older year. Local time of the server zone, 24 h.
    pub fn stamp(&self, ms: i64, now: i64) -> String {
        let (d, today) = (self.date_of(ms), self.date_of(now));
        if d == today {
            format!("Today {}", self.hms(ms))
        } else if d.format("%Y").to_string() == today.format("%Y").to_string() {
            self.fmt(ms, "%b %-d, %H:%M:%S")
        } else {
            self.fmt(ms, "%b %-d %Y, %H:%M:%S")
        }
    }

    /// `stamp` with the zone: "Today 15:54:22 CST" (tooltips).
    pub fn stamp_tz(&self, ms: i64, now: i64) -> String {
        format!("{} {}", self.stamp(ms, now), self.abbr(ms))
    }

    /// The full time with the date and zone: "Tue, Nov 10, 2026, 15:54:22 CST".
    pub fn full(&self, ms: i64) -> String {
        format!("{} {}", self.fmt(ms, "%a, %b %-d, %Y, %H:%M:%S"), self.abbr(ms))
    }

    /// "UTC−06:00" at `ms` (with a real minus sign).
    pub fn offset_label(&self, ms: i64) -> String {
        let secs = self.at(ms).offset().fix().local_minus_utc();
        let sign = if secs < 0 { '\u{2212}' } else { '+' };
        let a = secs.abs();
        format!("UTC{sign}{:02}:{:02}", a / 3600, (a % 3600) / 60)
    }
}

/// Parse an ISO-8601 UTC time ("2026-11-08T14:03:07.215Z") to Unix ms.
pub fn parse_ts(s: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.timestamp_millis())
}

pub fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

pub fn iso(ms: i64) -> String {
    fm_flight_recorder::eventlog::iso_from_unix_ms(ms)
}

/// "just now", "45 s ago", "12 min ago", "3 h 05 min ago", "2 d ago".
pub fn ago(ms: i64, now: i64) -> String {
    let s = (now - ms).max(0) / 1000;
    if s < 10 {
        "just now".into()
    } else if s < 60 {
        format!("{s} s ago")
    } else if s < 3600 {
        format!("{} min ago", s / 60)
    } else if s < 86_400 {
        format!("{} h {:02} min ago", s / 3600, (s % 3600) / 60)
    } else {
        format!("{} d ago", s / 86_400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn dst_days_have_23_and_25_hours() {
        let z = Zone::new("America/Chicago").unwrap();
        assert_eq!(z.day_hours(d(2026, 3, 8)), 23);
        assert_eq!(z.day_hours(d(2026, 11, 1)), 25);
        assert_eq!(z.day_hours(d(2026, 11, 2)), 24);
        let s = z.day_start(d(2026, 11, 1));
        assert_eq!(z.hour_starts(s, z.day_end(d(2026, 11, 1))).len(), 25);
        let s = z.day_start(d(2026, 3, 8));
        assert_eq!(z.hour_starts(s, z.day_end(d(2026, 3, 8))).len(), 23);
    }

    #[test]
    fn day_boundaries_follow_the_zone() {
        let z = Zone::new("America/Chicago").unwrap();
        // 2026-11-09T05:30:00Z is 23:30 on Nov 8 in Chicago (UTC-6).
        let t = parse_ts("2026-11-09T05:30:00.000Z").unwrap();
        assert_eq!(z.date_of(t), d(2026, 11, 8));
        assert_eq!(
            z.day_start(d(2026, 11, 9)),
            parse_ts("2026-11-09T06:00:00Z").unwrap()
        );
        assert_eq!(z.offset_label(t), "UTC\u{2212}06:00");
        let utc = Zone::new("UTC").unwrap();
        assert_eq!(utc.date_of(t), d(2026, 11, 9));
        assert_eq!(utc.offset_label(t), "UTC+00:00");
    }

    #[test]
    fn local_hours_on_a_dst_day() {
        let z = Zone::new("America/Chicago").unwrap();
        let s = z.day_start(d(2026, 11, 1));
        let labels: Vec<String> = z
            .hour_starts(s, s + 4 * HOUR_MS)
            .iter()
            .map(|t| z.fmt(*t, "%H"))
            .collect();
        // 01:00 comes twice on the day DST ends.
        assert_eq!(labels, ["00", "01", "01", "02"]);
    }

    #[test]
    fn one_time_format_with_the_zone() {
        let z = Zone::new("America/Chicago").unwrap();
        let now = parse_ts("2026-11-10T21:54:22Z").unwrap();
        assert_eq!(z.stamp(now, now), "Today 15:54:22");
        assert_eq!(z.stamp_tz(now, now), "Today 15:54:22 CST");
        let y = parse_ts("2026-11-09T15:04:05Z").unwrap();
        assert_eq!(z.stamp(y, now), "Nov 9, 09:04:05");
        let old = parse_ts("2025-12-30T18:00:00Z").unwrap();
        assert_eq!(z.stamp(old, now), "Dec 30 2025, 12:00:00");
        assert_eq!(z.full(now), "Tue, Nov 10, 2026, 15:54:22 CST");
        let summer = parse_ts("2026-07-01T12:00:00Z").unwrap();
        assert_eq!(z.abbr(summer), "CDT");
        assert_eq!(ago(now - 5_000, now), "just now");
        assert_eq!(ago(now - 45_000, now), "45 s ago");
        assert_eq!(ago(now - 125_000, now), "2 min ago");
        assert_eq!(ago(now - 3_900_000, now), "1 h 05 min ago");
    }

    #[test]
    fn bad_zone_is_an_error() {
        assert!(Zone::new("Mars/Base").is_err());
        assert!(Zone::new("").is_ok());
    }
}
