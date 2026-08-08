//! The envelope's `at`: RFC 3339, UTC, milliseconds — as a value, never as a clock read.
//!
//! §2 of the contract says the interpreter contains no clock. So a [`Timestamp`] is a number a
//! driver hands in, and this module is only the arithmetic that turns it into the text RFC 3339
//! asks for. Nothing here calls [`std::time::SystemTime::now`], and nothing in this crate does.
//!
//! The calendar arithmetic is the proleptic Gregorian day-count conversion: days since
//! 1970-01-01 to and from a civil year/month/day, with the year shifted to start in March so
//! that the leap day falls at the end of a year and every month length becomes one linear
//! formula. It is exact for every date this contract will ever carry and needs no table.

use std::fmt;

/// Milliseconds in a day, and the smaller units it decomposes into.
const MS_PER_SECOND: i64 = 1_000;
const SECONDS_PER_MINUTE: i64 = 60;
const SECONDS_PER_HOUR: i64 = 3_600;
const SECONDS_PER_DAY: i64 = 86_400;

/// An instant, as Unix milliseconds, rendered as RFC 3339 UTC with milliseconds.
///
/// Deliberately not `Instant` or `SystemTime`: both of those are read from a clock, and a type in
/// this crate that could be *obtained* rather than *supplied* would be the sans-IO rule leaking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Timestamp(i64);

impl Timestamp {
    /// From Unix milliseconds — the driver's clock reading, passed in.
    #[must_use]
    pub fn from_unix_millis(millis: i64) -> Self {
        Self(millis)
    }

    /// Its Unix milliseconds.
    #[must_use]
    pub fn unix_millis(self) -> i64 {
        self.0
    }

    /// `YYYY-MM-DDThh:mm:ss.sssZ` (RFC 3339, §5.1's `at`).
    #[must_use]
    pub fn to_rfc3339(self) -> String {
        // Euclidean division, so a pre-epoch instant floors towards the earlier day rather than
        // towards zero — otherwise 1969 would render an hour of negative time-of-day.
        let millis_of_day = self.0.rem_euclid(MS_PER_SECOND * SECONDS_PER_DAY);
        let days = self.0.div_euclid(MS_PER_SECOND * SECONDS_PER_DAY);
        let (year, month, day) = civil_from_days(days);
        let seconds_of_day = millis_of_day / MS_PER_SECOND;
        let millis = millis_of_day % MS_PER_SECOND;
        format!(
            "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
            seconds_of_day / SECONDS_PER_HOUR,
            (seconds_of_day % SECONDS_PER_HOUR) / SECONDS_PER_MINUTE,
            seconds_of_day % SECONDS_PER_MINUTE,
        )
    }

    /// Read one back, in the shape [`Self::to_rfc3339`] writes.
    ///
    /// Only UTC (`Z`) and only milliseconds: RFC 3339 permits an offset and any number of
    /// fractional digits, and §5.1 of the contract narrows both. Returns `None` for anything
    /// else, rather than guessing at a local time nobody named.
    #[must_use]
    pub fn from_rfc3339(text: &str) -> Option<Self> {
        let bytes = text.as_bytes();
        if bytes.len() != 24 || bytes.last() != Some(&b'Z') {
            return None;
        }
        let field = |from: usize, to: usize| -> Option<i64> {
            let slice = text.get(from..to)?;
            if !slice.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            slice.parse::<i64>().ok()
        };
        let separators = [
            (4, b'-'),
            (7, b'-'),
            (10, b'T'),
            (13, b':'),
            (16, b':'),
            (19, b'.'),
        ];
        for (at, expected) in separators {
            if bytes.get(at) != Some(&expected) {
                return None;
            }
        }
        let year = field(0, 4)?;
        let month = field(5, 7)?;
        let day = field(8, 10)?;
        let hour = field(11, 13)?;
        let minute = field(14, 16)?;
        let second = field(17, 19)?;
        let millis = field(20, 23)?;
        if !(1..=12).contains(&month)
            || !(1..=31).contains(&day)
            || hour > 23
            || minute > 59
            // A leap second is a legal RFC 3339 value even though no `at` this host writes will
            // carry one; refusing it would make a round trip of somebody else's timestamp fail.
            || second > 60
        {
            return None;
        }
        let days = days_from_civil(year, month, day);
        Some(Self(
            ((days * SECONDS_PER_DAY)
                + hour * SECONDS_PER_HOUR
                + minute * SECONDS_PER_MINUTE
                + second)
                * MS_PER_SECOND
                + millis,
        ))
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_rfc3339())
    }
}

/// Days since 1970-01-01 → civil year, month, day (proleptic Gregorian).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    // Shift the epoch to 0000-03-01, so the leap day is the last day of the shifted year.
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097; // [0, 146096]
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153; // [0, 11], 0 = March
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// Civil year, month, day → days since 1970-01-01. The inverse of [`civil_from_days`].
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400; // [0, 399]
    let shifted_month = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    /// §5.1's own example instant.
    #[test]
    fn the_spec_s_example_timestamp_round_trips() {
        let text = "2026-07-28T09:15:04.221Z";
        let stamp = Timestamp::from_rfc3339(text).unwrap();
        assert_eq!(stamp.to_rfc3339(), text);
    }

    #[test]
    fn the_epoch_and_the_dates_around_it() {
        assert_eq!(
            Timestamp::from_unix_millis(0).to_rfc3339(),
            "1970-01-01T00:00:00.000Z"
        );
        assert_eq!(
            Timestamp::from_unix_millis(-1).to_rfc3339(),
            "1969-12-31T23:59:59.999Z"
        );
        assert_eq!(
            Timestamp::from_unix_millis(1_000).to_rfc3339(),
            "1970-01-01T00:00:01.000Z"
        );
    }

    /// The two cases a hand-rolled calendar gets wrong: a leap day, and a century that is not one.
    #[test]
    fn leap_years_are_the_gregorian_ones() {
        assert_eq!(
            Timestamp::from_rfc3339("2024-02-29T12:00:00.000Z")
                .unwrap()
                .to_rfc3339(),
            "2024-02-29T12:00:00.000Z"
        );
        assert_eq!(
            Timestamp::from_rfc3339("2000-02-29T00:00:00.000Z")
                .unwrap()
                .to_rfc3339(),
            "2000-02-29T00:00:00.000Z"
        );
        // 1900 was not a leap year, so 1900-03-01 is the day after 1900-02-28.
        let feb28 = Timestamp::from_rfc3339("1900-02-28T00:00:00.000Z").unwrap();
        let mar01 = Timestamp::from_rfc3339("1900-03-01T00:00:00.000Z").unwrap();
        assert_eq!(mar01.unix_millis() - feb28.unix_millis(), 86_400_000);
    }

    /// Every day for a decade, both ways. A calendar that is right about one date and wrong about
    /// the next is the failure mode worth ruling out exhaustively rather than by sampling.
    #[test]
    fn every_day_of_a_decade_survives_both_directions() {
        let start = Timestamp::from_rfc3339("2020-01-01T00:00:00.000Z")
            .unwrap()
            .unix_millis();
        for day in 0..3_653i64 {
            let stamp = Timestamp::from_unix_millis(start + day * 86_400_000 + 43_200_123);
            let text = stamp.to_rfc3339();
            assert_eq!(
                Timestamp::from_rfc3339(&text),
                Some(stamp),
                "{text} did not survive"
            );
        }
    }

    #[test]
    fn what_is_not_this_shape_is_refused_rather_than_guessed() {
        for text in [
            "",
            "2026-07-28T09:15:04Z",
            "2026-07-28T09:15:04.221+02:00",
            "2026-07-28 09:15:04.221Z",
            "2026-13-28T09:15:04.221Z",
            "2026-07-28T24:15:04.221Z",
            "202x-07-28T09:15:04.221Z",
            "2026-07-28T09:15:04.221z",
        ] {
            assert_eq!(
                Timestamp::from_rfc3339(text),
                None,
                "{text} should be refused"
            );
        }
    }
}
