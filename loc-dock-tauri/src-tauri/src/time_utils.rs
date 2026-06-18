use chrono::{DateTime, Datelike, Duration, Timelike};
use chrono_tz::Tz;

/// Compute the start of "today" given the user's configured day-start hour.
/// If the current time is before that hour, "today" means yesterday at that hour.
pub fn day_start(now: &DateTime<Tz>, hour: u32) -> DateTime<Tz> {
    let today = now
        .with_hour(hour)
        .and_then(|t| t.with_minute(0))
        .and_then(|t| t.with_second(0))
        .and_then(|t| t.with_nanosecond(0))
        .unwrap_or(*now);
    if *now < today {
        today - Duration::days(1)
    } else {
        today
    }
}

/// Compute the start of the current week given day-start hour and week-start day (0=Mon).
pub fn week_start(now: &DateTime<Tz>, hour: u32, week_start_day: u32) -> DateTime<Tz> {
    let ds = day_start(now, hour);
    let days_since = (ds.weekday().num_days_from_monday() as i32 - week_start_day as i32)
        .rem_euclid(7) as i64;
    ds - Duration::days(days_since)
}

/// Compute the start of the current month at the given day-start hour.
pub fn month_start(now: &DateTime<Tz>, hour: u32) -> DateTime<Tz> {
    let ds = day_start(now, hour);
    ds
        .with_day(1)
        .and_then(|d| d.with_hour(hour))
        .and_then(|d| d.with_minute(0))
        .and_then(|d| d.with_second(0))
        .unwrap_or(ds)
}

/// Compute the start of the current year at the given day-start hour.
pub fn year_start(now: &DateTime<Tz>, hour: u32) -> DateTime<Tz> {
    let ds = day_start(now, hour);
    ds
        .with_month(1)
        .and_then(|d| d.with_day(1))
        .and_then(|d| d.with_hour(hour))
        .and_then(|d| d.with_minute(0))
        .and_then(|d| d.with_second(0))
        .unwrap_or(ds)
}
