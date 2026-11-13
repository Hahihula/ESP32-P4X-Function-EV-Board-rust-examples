//! UTC calendar time: [`DateTime`] turns Unix seconds, for example from an
//! SNTP query, into a calendar date and time.
//!
//! Hardware-independent: builds and is unit-tested on a PC.

use core::fmt;

/// A UTC date and time. Displays as `2026-11-10 14:27:39 UTC`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs, reason = "the field names say it all")]
pub struct DateTime {
    pub year: i32,
    /// 1..=12
    pub month: u8,
    /// 1..=31
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

impl DateTime {
    /// Converts Unix seconds (since 1970-01-01 00:00:00 UTC) to a date and
    /// time, with the proleptic Gregorian calendar.
    pub fn from_unix(seconds: u64) -> Self {
        let days = seconds / 86_400;
        let time = seconds % 86_400;
        let (year, month, day) = civil_from_days(days as i64);
        Self {
            year,
            month,
            day,
            hour: (time / 3600) as u8,
            minute: (time / 60 % 60) as u8,
            second: (time % 60) as u8,
        }
    }
}

impl fmt::Display for DateTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }
}

/// Year, month and day of the day `days` after 1970-01-01. Howard
/// Hinnant's algorithm: count in 400-year eras that start on 1 March, so
/// the leap day falls at the end of the year.
fn civil_from_days(days: i64) -> (i32, u8, u8) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_from_march = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_from_march + 2) / 5 + 1;
    let month = if month_from_march < 10 {
        month_from_march + 3
    } else {
        month_from_march - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year as i32, month as u8, day as u8)
}

#[cfg(test)]
mod tests {
    extern crate std;
    use std::string::ToString;

    use super::*;

    #[test]
    fn calendar_dates() {
        assert_eq!(
            DateTime::from_unix(0).to_string(),
            "1970-01-01 00:00:00 UTC"
        );
        assert_eq!(
            DateTime::from_unix(1_794_320_859).to_string(),
            "2026-11-10 14:27:39 UTC"
        );
        // Leap day, and the day after.
        assert_eq!(
            DateTime::from_unix(951_782_400).to_string(),
            "2000-02-29 00:00:00 UTC"
        );
        assert_eq!(
            DateTime::from_unix(951_868_799).to_string(),
            "2000-02-29 23:59:59 UTC"
        );
        assert_eq!(
            DateTime::from_unix(951_868_800).to_string(),
            "2000-03-01 00:00:00 UTC"
        );
    }
}
