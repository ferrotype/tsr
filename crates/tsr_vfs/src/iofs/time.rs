//! An instant with Go's zero value, enough arithmetic for a test clock and the
//! RFC 3339 nanosecond rendering the pin's observations use.
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// `None` inside is Go's zero `time.Time`, which no real instant equals.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Time(Option<(i64, u32)>);
impl Time {
    pub const ZERO: Self = Self(None);
    pub fn from_unix(seconds: i64, nanos: u32) -> Self {
        Self(Some((seconds, nanos)))
    }
    pub fn now() -> Self {
        SystemTime::now().into()
    }
    pub fn is_zero(self) -> bool {
        self.0.is_none()
    }
    pub fn unix(self) -> Option<(i64, u32)> {
        self.0
    }
    /// Go's `After`; the zero time is before every instant.
    pub fn after(self, other: Self) -> bool {
        self > other
    }
    #[must_use]
    pub fn add_seconds(self, seconds: i64) -> Self {
        Self(self.0.map(|(s, n)| (s + seconds, n)))
    }
    pub fn to_system_time(self) -> Option<SystemTime> {
        let (seconds, nanos) = self.0?;
        let whole = Duration::new(seconds.unsigned_abs(), 0);
        let base = if seconds >= 0 {
            UNIX_EPOCH.checked_add(whole)
        } else {
            UNIX_EPOCH.checked_sub(whole)
        }?;
        base.checked_add(Duration::new(0, nanos))
    }
    /// `2006-01-02T15:04:05.999999999Z`, UTC, trailing zeros of the fraction removed.
    pub fn parse_rfc3339(text: &str) -> Option<Self> {
        let bytes = text.as_bytes();
        let number = |range: std::ops::Range<usize>| text.get(range)?.parse::<i64>().ok();
        if bytes.len() < 20 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' {
            return None;
        }
        let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
        let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
        let mut index = 19;
        let mut nanos = 0u32;
        if bytes.get(index) == Some(&b'.') {
            let start = index + 1;
            index = start;
            while bytes.get(index).is_some_and(u8::is_ascii_digit) {
                index += 1;
            }
            let digits = text.get(start..index)?;
            if digits.is_empty() || digits.len() > 9 {
                return None;
            }
            nanos = format!("{digits:0<9}").parse().ok()?;
        }
        let offset = match bytes.get(index)? {
            b'Z' if index + 1 == bytes.len() => 0,
            sign @ (b'+' | b'-') if index + 6 == bytes.len() && bytes[index + 3] == b':' => {
                let seconds =
                    number(index + 1..index + 3)? * 3600 + number(index + 4..index + 6)? * 60;
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
        let Some((seconds, nanos)) = self.0 else {
            return "0001-01-01T00:00:00Z".into();
        };
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
