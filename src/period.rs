//! Resolution of a reporting window from a period name and the current time.
//!
//! All functions here are pure: the clock and the time zone are supplied by
//! the caller, so week and month boundaries are testable across zones,
//! daylight-saving transitions and year ends.

use std::fmt;
use std::str::FromStr;

use anyhow::{Context, Result, bail};
use jiff::civil::{Date, Weekday};
use jiff::{Span, Zoned};
use serde::Deserialize;

/// A half-open window `[start, end)`.
#[derive(Debug, Clone, PartialEq)]
pub struct TimeRange {
    pub start: Zoned,
    pub end: Zoned,
    /// True when `end` is the current moment rather than a fixed boundary.
    /// An open-ended window is never cached against its end, which would
    /// otherwise change on every invocation.
    pub open_ended: bool,
}

impl TimeRange {
    /// Formats the bounds as GitHub search accepts them: second-precision
    /// local timestamps with an explicit UTC offset.
    pub fn as_search_bounds(&self) -> (String, String) {
        let fmt = |z: &Zoned| z.strftime("%Y-%m-%dT%H:%M:%S%:z").to_string();
        (fmt(&self.start), fmt(&self.end))
    }

    pub fn label(&self) -> String {
        format!(
            "{} .. {}",
            self.start.strftime("%Y-%m-%d"),
            self.end.strftime("%Y-%m-%d %H:%M")
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Period {
    Day,
    #[default]
    Week,
    Month,
}

impl FromStr for Period {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "day" | "d" => Ok(Self::Day),
            "week" | "w" => Ok(Self::Week),
            "month" | "m" => Ok(Self::Month),
            other => bail!("unknown period `{other}` (expected `day`, `week` or `month`)"),
        }
    }
}

impl fmt::Display for Period {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
        })
    }
}

/// The day a reporting week starts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DayName {
    #[default]
    Mon,
    Tue,
    Wed,
    Thu,
    Fri,
    Sat,
    Sun,
}

impl DayName {
    fn to_weekday(self) -> Weekday {
        match self {
            Self::Mon => Weekday::Monday,
            Self::Tue => Weekday::Tuesday,
            Self::Wed => Weekday::Wednesday,
            Self::Thu => Weekday::Thursday,
            Self::Fri => Weekday::Friday,
            Self::Sat => Weekday::Saturday,
            Self::Sun => Weekday::Sunday,
        }
    }
}

impl FromStr for DayName {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "mon" | "monday" => Ok(Self::Mon),
            "tue" | "tuesday" => Ok(Self::Tue),
            "wed" | "wednesday" => Ok(Self::Wed),
            "thu" | "thursday" => Ok(Self::Thu),
            "fri" | "friday" => Ok(Self::Fri),
            "sat" | "saturday" => Ok(Self::Sat),
            "sun" | "sunday" => Ok(Self::Sun),
            other => bail!("unknown day `{other}` (expected mon, tue, wed, thu, fri, sat or sun)"),
        }
    }
}

/// Resolves the window covered by `period` relative to `now`.
///
/// The current period runs from its first instant up to `now`, so a report is
/// meaningful at any point during the period. With `previous`, the window is
/// the whole preceding period instead, ending where the current one begins.
pub fn resolve(
    now: &Zoned,
    period: Period,
    first_day: DayName,
    previous: bool,
) -> Result<TimeRange> {
    let today = now.date();
    let current_start = match period {
        Period::Day => today,
        Period::Week => start_of_week(today, first_day)?,
        Period::Month => today.first_of_month(),
    };

    let start_of = |date: Date| -> Result<Zoned> {
        date.to_zoned(now.time_zone().clone())
            .with_context(|| format!("no valid start of day for {date}"))
    };

    if !previous {
        return Ok(TimeRange {
            start: start_of(current_start)?,
            end: now.clone(),
            open_ended: true,
        });
    }

    let back = match period {
        Period::Day => Span::new().days(1),
        Period::Week => Span::new().days(7),
        Period::Month => Span::new().months(1),
    };
    let previous_start = current_start
        .checked_sub(back)
        .with_context(|| format!("cannot step one {period} back from {current_start}"))?;

    Ok(TimeRange {
        start: start_of(previous_start)?,
        end: start_of(current_start)?,
        open_ended: false,
    })
}

/// Builds a window from explicit dates. `until` defaults to `now`, and is
/// treated as inclusive by extending to the end of that day.
pub fn explicit(now: &Zoned, since: Date, until: Option<Date>) -> Result<TimeRange> {
    let tz = now.time_zone().clone();
    let start = since
        .to_zoned(tz.clone())
        .with_context(|| format!("no valid start of day for {since}"))?;

    let open_ended = until.is_none();
    let end = match until {
        None => now.clone(),
        Some(date) => {
            let next = date
                .tomorrow()
                .with_context(|| format!("cannot advance past {date}"))?;
            next.to_zoned(tz)
                .with_context(|| format!("no valid start of day for {next}"))?
        }
    };

    if end < start {
        bail!("--until must not precede --since");
    }
    Ok(TimeRange {
        start,
        end,
        open_ended,
    })
}

fn start_of_week(date: Date, first_day: DayName) -> Result<Date> {
    let offset =
        date.weekday().to_monday_zero_offset() - first_day.to_weekday().to_monday_zero_offset();
    let back = i64::from(offset.rem_euclid(7));
    date.checked_sub(Span::new().days(back))
        .with_context(|| format!("cannot step {back} days back from {date}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> Zoned {
        s.parse().unwrap()
    }

    fn date(s: &str) -> Date {
        s.parse().unwrap()
    }

    #[test]
    fn week_starts_on_the_configured_day_and_ends_now() {
        // 2026-09-15 is a Tuesday.
        let now = at("2026-09-15T14:30:00+02:00[Europe/Oslo]");
        let r = resolve(&now, Period::Week, DayName::Mon, false).unwrap();
        assert_eq!(
            r.start.to_string(),
            "2026-09-14T00:00:00+02:00[Europe/Oslo]"
        );
        assert_eq!(r.end, now);
    }

    #[test]
    fn week_queried_on_its_first_day_starts_that_morning() {
        let now = at("2026-09-14T09:00:00+02:00[Europe/Oslo]");
        let r = resolve(&now, Period::Week, DayName::Mon, false).unwrap();
        assert_eq!(
            r.start.to_string(),
            "2026-09-14T00:00:00+02:00[Europe/Oslo]"
        );
    }

    #[test]
    fn week_respects_a_non_monday_first_day() {
        let now = at("2026-09-15T14:30:00+02:00[Europe/Oslo]");
        let r = resolve(&now, Period::Week, DayName::Sun, false).unwrap();
        assert_eq!(r.start.date(), date("2026-09-13"));
    }

    #[test]
    fn previous_week_is_the_whole_preceding_week() {
        let now = at("2026-09-15T14:30:00+02:00[Europe/Oslo]");
        let r = resolve(&now, Period::Week, DayName::Mon, true).unwrap();
        assert_eq!(r.start.date(), date("2026-09-07"));
        assert_eq!(r.end.date(), date("2026-09-14"));
    }

    #[test]
    fn day_runs_from_midnight_up_to_now() {
        let now = at("2026-09-15T14:30:00+02:00[Europe/Oslo]");
        let r = resolve(&now, Period::Day, DayName::Mon, false).unwrap();
        assert_eq!(
            r.start.to_string(),
            "2026-09-15T00:00:00+02:00[Europe/Oslo]"
        );
        assert_eq!(r.end, now);
        assert!(r.open_ended);
    }

    #[test]
    fn previous_day_is_the_whole_calendar_day_before() {
        let now = at("2026-09-14T08:00:00+02:00[Europe/Oslo]");
        let r = resolve(&now, Period::Day, DayName::Mon, true).unwrap();
        assert_eq!(
            r.start.to_string(),
            "2026-09-13T00:00:00+02:00[Europe/Oslo]"
        );
        assert_eq!(r.end.to_string(), "2026-09-14T00:00:00+02:00[Europe/Oslo]");
        assert!(!r.open_ended);
    }

    #[test]
    fn previous_day_spans_a_short_day_across_a_spring_forward_transition() {
        // Norway springs forward on 2026-03-29.
        let now = at("2026-03-30T08:00:00+02:00[Europe/Oslo]");
        let r = resolve(&now, Period::Day, DayName::Mon, true).unwrap();
        assert_eq!(
            r.start.to_string(),
            "2026-03-29T00:00:00+01:00[Europe/Oslo]"
        );
        assert_eq!(r.end.to_string(), "2026-03-30T00:00:00+02:00[Europe/Oslo]");
    }

    #[test]
    fn month_starts_on_the_first() {
        let now = at("2026-09-15T14:30:00+02:00[Europe/Oslo]");
        let r = resolve(&now, Period::Month, DayName::Mon, false).unwrap();
        assert_eq!(
            r.start.to_string(),
            "2026-09-01T00:00:00+02:00[Europe/Oslo]"
        );
    }

    #[test]
    fn previous_month_crosses_the_year_boundary() {
        let now = at("2026-01-09T08:00:00+01:00[Europe/Oslo]");
        let r = resolve(&now, Period::Month, DayName::Mon, true).unwrap();
        assert_eq!(r.start.date(), date("2025-12-01"));
        assert_eq!(r.end.date(), date("2026-01-01"));
    }

    #[test]
    fn week_start_survives_a_spring_forward_transition() {
        // Norway springs forward on 2026-03-29, a Sunday.
        let now = at("2026-03-31T10:00:00+02:00[Europe/Oslo]");
        let r = resolve(&now, Period::Week, DayName::Sun, false).unwrap();
        assert_eq!(r.start.date(), date("2026-03-29"));
    }

    #[test]
    fn search_bounds_carry_an_explicit_offset() {
        let now = at("2026-09-15T14:30:00+02:00[Europe/Oslo]");
        let r = resolve(&now, Period::Week, DayName::Mon, false).unwrap();
        let (start, end) = r.as_search_bounds();
        assert_eq!(start, "2026-09-14T00:00:00+02:00");
        assert_eq!(end, "2026-09-15T14:30:00+02:00");
    }

    #[test]
    fn explicit_until_includes_the_whole_named_day() {
        let now = at("2026-09-15T14:30:00+02:00[Europe/Oslo]");
        let r = explicit(&now, date("2026-09-01"), Some(date("2026-09-07"))).unwrap();
        assert_eq!(r.end.date(), date("2026-09-08"));
    }

    #[test]
    fn explicit_rejects_an_inverted_range() {
        let now = at("2026-09-15T14:30:00+02:00[Europe/Oslo]");
        assert!(explicit(&now, date("2026-09-10"), Some(date("2026-09-01"))).is_err());
    }

    #[test]
    fn a_week_starts_on_its_first_day_whatever_day_it_is_queried() {
        let first_days = [
            DayName::Mon,
            DayName::Tue,
            DayName::Wed,
            DayName::Thu,
            DayName::Fri,
            DayName::Sat,
            DayName::Sun,
        ];
        // 2026-09-14 is a Monday; the dates cover a full week.
        for offset in 0..7 {
            let today = date("2026-09-14")
                .checked_add(Span::new().days(offset))
                .unwrap();
            for first_day in first_days {
                let start = start_of_week(today, first_day).unwrap();
                let back = (today - start).get_days();
                assert_eq!(
                    start.weekday(),
                    first_day.to_weekday(),
                    "{today} {first_day:?}"
                );
                assert!(
                    (0..7).contains(&back),
                    "{today} {first_day:?}: {back} days back"
                );
            }
        }
    }

    #[test]
    fn only_the_current_period_is_open_ended() {
        let now = at("2026-09-15T14:30:00+02:00[Europe/Oslo]");
        assert!(
            resolve(&now, Period::Week, DayName::Mon, false)
                .unwrap()
                .open_ended
        );
        assert!(
            !resolve(&now, Period::Week, DayName::Mon, true)
                .unwrap()
                .open_ended
        );
    }

    #[test]
    fn explicit_without_until_runs_up_to_now() {
        let now = at("2026-09-15T14:30:00+02:00[Europe/Oslo]");
        let r = explicit(&now, date("2026-09-01"), None).unwrap();
        assert_eq!(r.end, now);
        assert!(r.open_ended);
    }

    #[test]
    fn explicit_accepts_a_single_day() {
        let now = at("2026-09-15T14:30:00+02:00[Europe/Oslo]");
        let r = explicit(&now, date("2026-09-07"), Some(date("2026-09-07"))).unwrap();
        assert_eq!(r.start.date(), date("2026-09-07"));
        assert_eq!(r.end.date(), date("2026-09-08"));
        assert!(!r.open_ended);
    }

    #[test]
    fn the_label_shows_the_start_date_and_the_end_minute() {
        let now = at("2026-09-15T14:30:00+02:00[Europe/Oslo]");
        let r = resolve(&now, Period::Week, DayName::Mon, false).unwrap();
        assert_eq!(r.label(), "2026-09-14 .. 2026-09-15 14:30");
    }

    #[test]
    fn periods_parse_from_their_single_letter_forms_and_display_in_full() {
        assert_eq!("D".parse::<Period>().unwrap(), Period::Day);
        assert_eq!("W".parse::<Period>().unwrap(), Period::Week);
        assert_eq!("m".parse::<Period>().unwrap(), Period::Month);
        assert!("year".parse::<Period>().is_err());
        assert_eq!(Period::Day.to_string(), "day");
        assert_eq!(Period::Week.to_string(), "week");
        assert_eq!(Period::Month.to_string(), "month");
    }

    #[test]
    fn day_and_period_names_parse_from_their_short_forms() {
        assert_eq!("Mon".parse::<DayName>().unwrap(), DayName::Mon);
        assert_eq!("sunday".parse::<DayName>().unwrap(), DayName::Sun);
        assert_eq!("month".parse::<Period>().unwrap(), Period::Month);
        assert!("noneday".parse::<DayName>().is_err());
    }
}
