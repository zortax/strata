//! AIRAC cycles: the 28-day aeronautical data revision calendar.

use chrono::{Datelike, Days, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

/// Cycle length in days.
const CYCLE_DAYS: i64 = 28;

/// Reference cycle: AIRAC 2001 became effective on 2020-01-02. Every cycle
/// effective date is on the 28-day grid anchored here.
fn epoch() -> NaiveDate {
    // Compile-time-known valid date.
    NaiveDate::from_ymd_opt(2020, 1, 2).expect("valid constant date")
}

/// One AIRAC cycle, identified chart-style (`"2506"` = 6th cycle of 2025).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AiracCycle {
    id: String,
    effective: NaiveDate,
}

impl AiracCycle {
    /// Builds a cycle from already-known values (e.g. read back from the
    /// store's meta table). Prefer [`AiracCycle::current_for`] for deriving.
    pub fn new(id: impl Into<String>, effective: NaiveDate) -> Self {
        Self {
            id: id.into(),
            effective,
        }
    }

    /// The cycle in effect on `date`.
    pub fn current_for(date: NaiveDate) -> Self {
        let days = (date - epoch()).num_days();
        let n = days.div_euclid(CYCLE_DAYS);
        let effective = grid_date(n);
        // Ordinal within the effective year: count cycles since the first
        // cycle whose effective date falls in that year.
        let jan1 = NaiveDate::from_ymd_opt(effective.year(), 1, 1)
            .expect("January 1st always exists");
        let mut first_of_year = grid_date((jan1 - epoch()).num_days().div_euclid(CYCLE_DAYS));
        if first_of_year < jan1 {
            first_of_year = first_of_year + Days::new(CYCLE_DAYS as u64);
        }
        let ordinal = (effective - first_of_year).num_days() / CYCLE_DAYS + 1;
        let id = format!("{:02}{:02}", effective.year().rem_euclid(100), ordinal);
        Self { id, effective }
    }

    /// The cycle in effect right now (UTC).
    pub fn current() -> Self {
        Self::current_for(Utc::now().date_naive())
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn effective_date(&self) -> NaiveDate {
        self.effective
    }

    /// First day on which this cycle is superseded.
    pub fn supersession_date(&self) -> NaiveDate {
        self.effective + Days::new(CYCLE_DAYS as u64)
    }

    /// Whether a newer cycle is effective on `date`.
    pub fn is_stale_at(&self, date: NaiveDate) -> bool {
        date >= self.supersession_date()
    }

    /// Whether a newer cycle is effective today (UTC).
    pub fn is_stale(&self) -> bool {
        self.is_stale_at(Utc::now().date_naive())
    }
}

/// The `n`-th effective date on the 28-day grid (`n` may be negative).
fn grid_date(n: i64) -> NaiveDate {
    if n >= 0 {
        epoch() + Days::new((n * CYCLE_DAYS) as u64)
    } else {
        epoch() - Days::new((-n * CYCLE_DAYS) as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn epoch_cycle() {
        let c = AiracCycle::current_for(d(2020, 1, 2));
        assert_eq!(c.id(), "2001");
        assert_eq!(c.effective_date(), d(2020, 1, 2));
    }

    #[test]
    fn cycle_2506() {
        // AIRAC 2506 became effective 2025-06-12.
        let c = AiracCycle::current_for(d(2025, 6, 15));
        assert_eq!(c.id(), "2506");
        assert_eq!(c.effective_date(), d(2025, 6, 12));
    }

    #[test]
    fn effective_day_itself_belongs_to_the_new_cycle() {
        let c = AiracCycle::current_for(d(2025, 6, 12));
        assert_eq!(c.id(), "2506");
        let before = AiracCycle::current_for(d(2025, 6, 11));
        assert_eq!(before.id(), "2505");
        assert_eq!(before.effective_date(), d(2025, 5, 15));
    }

    #[test]
    fn year_boundary_keeps_previous_years_numbering() {
        // 2025 has 13 cycles; 2513 (eff 2025-12-25) runs into January 2026.
        let c = AiracCycle::current_for(d(2026, 1, 10));
        assert_eq!(c.id(), "2513");
        assert_eq!(c.effective_date(), d(2025, 12, 25));
        // First 2026 cycle.
        let c = AiracCycle::current_for(d(2026, 1, 22));
        assert_eq!(c.id(), "2601");
    }

    #[test]
    fn staleness() {
        let c = AiracCycle::current_for(d(2025, 6, 12));
        assert!(!c.is_stale_at(d(2025, 6, 12)));
        assert!(!c.is_stale_at(d(2025, 7, 9))); // last effective day
        assert!(c.is_stale_at(d(2025, 7, 10))); // 2507 effective
        assert_eq!(c.supersession_date(), d(2025, 7, 10));
    }
}
