//! Resolution of METAR/TAF day/hour tokens (which carry no month or year)
//! to absolute UTC datetimes near an anchor instant.

use chrono::{DateTime, Datelike, Days, NaiveDate, Utc};

/// `ddhhmmZ` issue/observation time token → `(day, hour, minute)`.
pub(crate) fn parse_day_time_z(token: &str) -> Option<(u32, u32, u32)> {
    let body = token.strip_suffix('Z')?;
    if body.len() != 6 || !body.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((
        body[0..2].parse().ok()?,
        body[2..4].parse().ok()?,
        body[4..6].parse().ok()?,
    ))
}

/// Resolves a day-of-month + hour to the UTC instant closest to `anchor`
/// (handles month/year boundaries). Hour 24 means midnight of the next day,
/// as used in TAF validity periods.
pub(crate) fn resolve_day_hour(day: u32, hour: u32, anchor: DateTime<Utc>) -> Option<DateTime<Utc>> {
    resolve_day_hour_minute(day, hour, 0, anchor)
}

pub(crate) fn resolve_day_hour_minute(
    day: u32,
    hour: u32,
    minute: u32,
    anchor: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    let (day_carry, hour) = if hour == 24 { (1u64, 0) } else { (0, hour) };
    if !(1..=31).contains(&day) || hour > 23 || minute > 59 {
        return None;
    }
    let mut best: Option<DateTime<Utc>> = None;
    for month_offset in [-1i32, 0, 1] {
        let (year, month) = shifted_month(anchor.date_naive(), month_offset);
        let Some(date) = NaiveDate::from_ymd_opt(year, month, day) else {
            continue; // e.g. day 31 in a 30-day month
        };
        let Some(datetime) = date.and_hms_opt(hour, minute, 0) else {
            continue;
        };
        let Some(datetime) = datetime.checked_add_days(Days::new(day_carry)) else {
            continue;
        };
        let candidate = datetime.and_utc();
        if best.is_none_or(|b| (candidate - anchor).abs() < (b - anchor).abs()) {
            best = Some(candidate);
        }
    }
    best
}

fn shifted_month(anchor: NaiveDate, offset: i32) -> (i32, u32) {
    let months = anchor.year() * 12 + anchor.month0() as i32 + offset;
    (months.div_euclid(12), months.rem_euclid(12) as u32 + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        NaiveDate::from_ymd_opt(y, mo, d)
            .expect("date")
            .and_hms_opt(h, mi, 0)
            .expect("time")
            .and_utc()
    }

    #[test]
    fn day_time_z_token() {
        assert_eq!(parse_day_time_z("092325Z"), Some((9, 23, 25)));
        assert_eq!(parse_day_time_z("092325"), None);
        assert_eq!(parse_day_time_z("0923Z"), None);
        assert_eq!(parse_day_time_z("ABCDEFZ"), None);
    }

    #[test]
    fn resolves_within_same_month() {
        let anchor = utc(2026, 6, 9, 23, 0);
        assert_eq!(resolve_day_hour(10, 0, anchor), Some(utc(2026, 6, 10, 0, 0)));
        assert_eq!(resolve_day_hour(9, 23, anchor), Some(utc(2026, 6, 9, 23, 0)));
    }

    #[test]
    fn hour_24_is_midnight_next_day() {
        let anchor = utc(2026, 6, 9, 23, 0);
        assert_eq!(resolve_day_hour(10, 24, anchor), Some(utc(2026, 6, 11, 0, 0)));
    }

    #[test]
    fn crosses_month_boundary_forward() {
        let anchor = utc(2026, 6, 30, 23, 0);
        assert_eq!(resolve_day_hour(1, 6, anchor), Some(utc(2026, 7, 1, 6, 0)));
    }

    #[test]
    fn crosses_month_boundary_backward() {
        let anchor = utc(2026, 7, 1, 0, 30);
        assert_eq!(resolve_day_hour(30, 22, anchor), Some(utc(2026, 6, 30, 22, 0)));
    }

    #[test]
    fn crosses_year_boundary() {
        let anchor = utc(2026, 1, 1, 0, 30);
        assert_eq!(
            resolve_day_hour_minute(31, 23, 50, anchor),
            Some(utc(2025, 12, 31, 23, 50))
        );
    }

    #[test]
    fn rejects_invalid_components() {
        let anchor = utc(2026, 6, 9, 23, 0);
        assert_eq!(resolve_day_hour(0, 12, anchor), None);
        assert_eq!(resolve_day_hour(32, 12, anchor), None);
        assert_eq!(resolve_day_hour(10, 25, anchor), None);
    }
}
