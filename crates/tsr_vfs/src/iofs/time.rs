//! An instant with Go's zero value, enough arithmetic for a test clock and the
//! RFC 3339 nanosecond rendering the pin's observations use.
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A UTC instant. Like Go's zero `time.Time`, the default is midnight on
/// January 1, year 1; it participates in ordinary comparison and arithmetic.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Time(i64, u32);
impl Default for Time {
    fn default() -> Self {
        Self::ZERO
    }
}
impl Time {
    pub const ZERO: Self = Self(-62_135_596_800, 0);
    pub fn from_unix(seconds: i64, nanos: u32) -> Self {
        Self(
            seconds.wrapping_add(i64::from(nanos / 1_000_000_000)),
            nanos % 1_000_000_000,
        )
    }
    pub fn now() -> Self {
        SystemTime::now().into()
    }
    pub fn is_zero(self) -> bool {
        self == Self::ZERO
    }
    pub fn unix(self) -> (i64, u32) {
        (self.0, self.1)
    }
    /// Go's `After`, comparing the represented instants.
    pub fn after(self, other: Self) -> bool {
        self > other
    }
    #[must_use]
    /// Advance a test clock; panics if the result exceeds the stored seconds range.
    pub fn add_seconds(self, seconds: i64) -> Self {
        Self(
            self.0
                .checked_add(seconds)
                .expect("timestamp seconds overflow"),
            self.1,
        )
    }
    pub fn to_system_time(self) -> Option<SystemTime> {
        let (seconds, nanos) = self.unix();
        let whole = Duration::new(seconds.unsigned_abs(), 0);
        let base = if seconds >= 0 {
            UNIX_EPOCH.checked_add(whole)
        } else {
            UNIX_EPOCH.checked_sub(whole)
        }?;
        base.checked_add(Duration::new(0, nanos))
    }
    /// Go's `time.Parse(time.RFC3339Nano, text)`, retaining the instant in UTC.
    /// This includes its one-digit hour, comma fraction, truncated subnanosecond
    /// digits and inclusive 24-hour/60-minute zone-offset allowances.
    pub fn parse_rfc3339(text: &str) -> Option<Self> {
        let bytes = text.as_bytes();
        let number = |range: std::ops::Range<usize>| {
            let digits = bytes.get(range)?;
            digits.iter().all(u8::is_ascii_digit).then_some(())?;
            Some(
                digits
                    .iter()
                    .fold(0i64, |value, digit| value * 10 + i64::from(digit - b'0')),
            )
        };
        if bytes.len() < 19 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' {
            return None;
        }
        let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
        let month_days = match month {
            2 => 28 + i64::from(year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)),
            4 | 6 | 9 | 11 => 30,
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            _ => return None,
        };
        let hour_end = if bytes[12] == b':' { 12 } else { 13 };
        if bytes.get(hour_end) != Some(&b':') || bytes.get(hour_end + 3) != Some(&b':') {
            return None;
        }
        let hour = number(11..hour_end)?;
        let minute = number(hour_end + 1..hour_end + 3)?;
        let second = number(hour_end + 4..hour_end + 6)?;
        if !(1..=month_days).contains(&day) || hour > 23 || minute > 59 || second > 59 {
            return None;
        }
        let mut index = hour_end + 6;
        let mut nanos = 0u32;
        if matches!(bytes.get(index), Some(b'.' | b',')) {
            let start = index + 1;
            index = start;
            while bytes.get(index).is_some_and(u8::is_ascii_digit) {
                index += 1;
            }
            let digits = text.get(start..index)?;
            if digits.is_empty() {
                return None;
            }
            let digits = &digits[..digits.len().min(9)];
            nanos = format!("{digits:0<9}").parse().ok()?;
        }
        let offset = match bytes.get(index)? {
            b'Z' if index + 1 == bytes.len() => 0,
            sign @ (b'+' | b'-') if index + 6 == bytes.len() && bytes[index + 3] == b':' => {
                let hour = number(index + 1..index + 3)?;
                let minute = number(index + 4..index + 6)?;
                if hour > 24 || minute > 60 {
                    return None;
                }
                let seconds = hour * 3600 + minute * 60;
                if *sign == b'+' {
                    seconds
                } else {
                    -seconds
                }
            }
            _ => return None,
        };
        let days = days_from_civil(year, month, day);
        Some(Self::from_unix(
            days * 86_400 + hour * 3600 + minute * 60 + second - offset,
            nanos,
        ))
    }
    pub fn format_rfc3339_nano(self) -> String {
        let (seconds, nanos) = self.unix();
        let (days, rest) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
        let (year, month, day) = civil_from_days(days);
        let mut text = format!(
            "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}",
            rest / 3600,
            rest % 3600 / 60,
            rest % 60
        );
        if nanos != 0 {
            let fraction = format!("{nanos:09}");
            text.push('.');
            text.push_str(fraction.trim_end_matches('0'));
        }
        text.push('Z');
        text
    }
}
impl From<SystemTime> for Time {
    fn from(value: SystemTime) -> Self {
        match value.duration_since(UNIX_EPOCH) {
            Ok(after) => Self::from_unix(
                i64::try_from(after.as_secs()).unwrap_or(i64::MAX),
                after.subsec_nanos(),
            ),
            Err(before) => {
                let before = before.duration();
                let seconds = -i64::try_from(before.as_secs()).unwrap_or(i64::MAX);
                if before.subsec_nanos() == 0 {
                    Self::from_unix(seconds, 0)
                } else {
                    Self::from_unix(seconds - 1, 1_000_000_000 - before.subsec_nanos())
                }
            }
        }
    }
}
// Howard Hinnant's civil calendar algorithms over the proleptic Gregorian calendar.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    (year_of_era + era * 400 + i64::from(month <= 2), month, day)
}

#[cfg(test)]
mod tests {
    use super::Time;

    // Observed with time.Parse(time.RFC3339Nano, ...) and time.Time{} on Go 1.27.1.
    #[test]
    fn zero_is_the_year_one_instant() {
        let parsed = Time::parse_rfc3339("0001-01-01T00:00:00Z").unwrap();
        assert_eq!(parsed, Time::ZERO);
        assert_eq!(Time::from_unix(-62_135_596_800, 0), Time::ZERO);
        assert!(parsed.is_zero());
        assert_eq!(Time::default(), parsed);
        assert_eq!(
            Time::ZERO.add_seconds(1).format_rfc3339_nano(),
            "0001-01-01T00:00:01Z"
        );
        assert!(Time::ZERO.after(Time::parse_rfc3339("0000-01-01T00:00:00Z").unwrap()));
        assert_eq!(
            Time::ZERO.to_system_time().map(Time::from),
            Some(Time::ZERO)
        );
    }

    #[test]
    fn rfc3339_rejects_invalid_dates_and_separators() {
        for text in [
            "2020-02-30T00:00:00Z",
            "1900-02-29T00:00:00Z",
            "2020-13-01T00:00:00Z",
            "2020-01-00T00:00:00Z",
            "2020-01-01T24:00:00Z",
            "2020-01-01T00:60:00Z",
            "2020-01-01T00:00:60Z",
            "2020-01-01T00x00x00Z",
            "2020-01-01T00:00:00+25:00",
            "2020-01-01T00:00:00+00:61",
            "2020-01-01T00:00:00.Z",
            "2020-+1-01T00:00:00Z",
        ] {
            assert!(Time::parse_rfc3339(text).is_none(), "{text}");
        }
    }

    #[test]
    fn rfc3339_keeps_the_pinned_parsers_permissive_cases() {
        for (text, expected) in [
            ("2000-02-29T00:00:00Z", "2000-02-29T00:00:00Z"),
            (
                "2020-01-01T00:00:00.123456789123Z",
                "2020-01-01T00:00:00.123456789Z",
            ),
            ("2020-01-01T00:00:00,5Z", "2020-01-01T00:00:00.5Z"),
            ("2020-01-01T1:00:00Z", "2020-01-01T01:00:00Z"),
            ("2020-01-01T00:00:00+24:60", "2019-12-30T23:00:00Z"),
        ] {
            assert_eq!(
                Time::parse_rfc3339(text).unwrap().format_rfc3339_nano(),
                expected
            );
        }
        assert_eq!(
            Time::from_unix(0, 1_500_000_000).format_rfc3339_nano(),
            "1970-01-01T00:00:01.5Z"
        );
    }
    #[test]
    fn rfc3339_round_trips_and_trims_the_fraction() {
        for text in [
            "2020-01-01T00:00:00Z",
            "1969-12-31T23:59:59.5Z",
            "2038-01-19T03:14:08.000000001Z",
        ] {
            assert_eq!(
                Time::parse_rfc3339(text).unwrap().format_rfc3339_nano(),
                text
            );
        }
        assert_eq!(
            Time::parse_rfc3339("2020-01-01T02:00:00+02:00"),
            Time::parse_rfc3339("2020-01-01T00:00:00Z")
        );
        assert!(Time::ZERO.is_zero() && !Time::from_unix(0, 0).after(Time::from_unix(0, 0)));
    }
}
