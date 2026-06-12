//! Pure DWD RV open-data layout logic: tarball naming/timestamp math, run
//! discovery candidates, valid-time → frame location, and timeline
//! assembly.
//!
//! Layout (verified live 2026-06-10): one tarball per analysis time at
//! `https://opendata.dwd.de/weather/radar/composite/rv/DE1200_RV{yymmddHHMM}.tar.bz2`,
//! every 5 minutes (published ~3 min after analysis), retained ~2 days.
//! Each tarball holds 25 RADOLAN frames `DE1200_RV{yymmddHHMM}_{mmm}`:
//! `_000` (the analysis) plus the `_005`..`_120` nowcast.

use chrono::{DateTime, Duration, Timelike, Utc};

use crate::domain::{GriddedTimeline, StepKind, TimelineStep};

use super::DwdRadarError;

/// Composite cadence: a new tarball every 5 minutes.
pub(super) const STEP_MINUTES: u32 = 5;

/// Nowcast lead covered by one tarball (`_005`..`_120`).
pub(super) const NOWCAST_MINUTES: u32 = 120;

/// Observed window advertised by the timeline: how far back past analysis
/// frames (from older tarballs) are offered. Retention on the server is
/// ~2 days, so 2 h is always safely available.
pub(super) const OBSERVED_WINDOW_MINUTES: u32 = 120;

/// How many recent composite times to probe before giving up: 6 steps =
/// 30 min, covering the ~3 min publication lag plus a few missed updates.
pub(super) const COMPOSITE_CANDIDATES: usize = 6;

/// Tarball file name for the composite published at `analysis`.
pub(super) fn tarball_name(analysis: DateTime<Utc>) -> String {
    format!("DE1200_RV{}.tar.bz2", analysis.format("%y%m%d%H%M"))
}

/// Full download URL of one composite tarball.
pub(super) fn tarball_url(base_url: &str, analysis: DateTime<Utc>) -> String {
    format!("{base_url}/{}", tarball_name(analysis))
}

/// Archive member name of the frame `forecast_minutes` after `analysis`.
pub(super) fn member_name(analysis: DateTime<Utc>, forecast_minutes: u32) -> String {
    format!(
        "DE1200_RV{}_{forecast_minutes:03}",
        analysis.format("%y%m%d%H%M")
    )
}

/// Floors `t` onto the 5-minute composite grid (seconds dropped).
pub(super) fn floor_to_step(t: DateTime<Utc>) -> DateTime<Utc> {
    let t = t
        .with_second(0)
        .and_then(|t| t.with_nanosecond(0))
        .unwrap_or(t);
    t - Duration::minutes(i64::from(t.minute() % STEP_MINUTES))
}

/// The newest possible composite time for `now` followed by older
/// fallbacks, newest first. The newest candidates may not be published yet
/// (~3 min lag) — the caller probes each in turn.
pub(super) fn composite_candidates(now: DateTime<Utc>) -> Vec<DateTime<Utc>> {
    let newest = floor_to_step(now);
    (0..COMPOSITE_CANDIDATES)
        .map(|n| newest - Duration::minutes(i64::from(STEP_MINUTES) * n as i64))
        .collect()
}

/// Where a valid time lives on the server: which tarball, which member.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FrameLocation {
    /// Analysis time of the tarball holding the frame.
    pub tarball_time: DateTime<Utc>,
    /// Member lead time within that tarball (0 = the analysis frame).
    pub forecast_minutes: u32,
}

/// Maps `valid_time` onto a fetchable frame given the newest published
/// analysis: past steps come from the `_000` analysis frame of the tarball
/// published *at* the valid time, the analysis and nowcast steps from the
/// newest tarball's members. Rejects times off the 5-minute grid or
/// outside the advertised window.
pub(super) fn locate_frame(
    analysis: DateTime<Utc>,
    valid_time: DateTime<Utc>,
) -> Result<FrameLocation, DwdRadarError> {
    let err = || DwdRadarError::InvalidValidTime {
        valid_time,
        analysis,
    };
    let delta = valid_time - analysis;
    let seconds = delta.num_seconds();
    if seconds % (60 * i64::from(STEP_MINUTES)) != 0 || delta.subsec_nanos() != 0 {
        return Err(err());
    }
    let minutes = seconds / 60;
    if minutes >= 0 {
        let forecast_minutes = u32::try_from(minutes).map_err(|_| err())?;
        if forecast_minutes > NOWCAST_MINUTES {
            return Err(err());
        }
        Ok(FrameLocation {
            tarball_time: analysis,
            forecast_minutes,
        })
    } else {
        if -minutes > i64::from(OBSERVED_WINDOW_MINUTES) {
            return Err(err());
        }
        Ok(FrameLocation {
            tarball_time: valid_time,
            forecast_minutes: 0,
        })
    }
}

/// The timeline advertised for the newest analysis: a 5-minute lattice
/// from `analysis − 2 h` ([`StepKind::Observed`], including the analysis
/// itself) through `analysis + 2 h` ([`StepKind::Forecast`] nowcast).
pub(super) fn timeline_for(analysis: DateTime<Utc>) -> GriddedTimeline {
    let first = -i64::from(OBSERVED_WINDOW_MINUTES);
    let last = i64::from(NOWCAST_MINUTES);
    let steps = (first..=last)
        .step_by(STEP_MINUTES as usize)
        .map(|m| TimelineStep {
            valid_time: analysis + Duration::minutes(m),
            kind: if m <= 0 {
                StepKind::Observed
            } else {
                StepKind::Forecast
            },
        })
        .collect();
    GriddedTimeline {
        run_time: analysis,
        steps,
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn t(d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, d, h, mi, 0).unwrap()
    }

    #[test]
    fn tarball_name_matches_the_published_pattern() {
        // Verified against the live server listing on 2026-06-10.
        assert_eq!(tarball_name(t(10, 17, 10)), "DE1200_RV2606101710.tar.bz2");
        assert_eq!(
            tarball_url("https://example.test/rv", t(8, 17, 15)),
            "https://example.test/rv/DE1200_RV2606081715.tar.bz2"
        );
    }

    #[test]
    fn member_names_carry_the_lead_minutes() {
        assert_eq!(member_name(t(10, 17, 10), 0), "DE1200_RV2606101710_000");
        assert_eq!(member_name(t(10, 17, 10), 75), "DE1200_RV2606101710_075");
        assert_eq!(member_name(t(10, 17, 10), 120), "DE1200_RV2606101710_120");
    }

    #[test]
    fn floor_to_step_snaps_to_the_5_minute_grid() {
        let odd = Utc.with_ymd_and_hms(2026, 6, 10, 17, 13, 42).unwrap();
        assert_eq!(floor_to_step(odd), t(10, 17, 10));
        assert_eq!(floor_to_step(t(10, 17, 15)), t(10, 17, 15));
    }

    #[test]
    fn composite_candidates_walk_back_in_5_minute_steps() {
        let now = Utc.with_ymd_and_hms(2026, 6, 10, 17, 13, 42).unwrap();
        assert_eq!(
            composite_candidates(now),
            vec![
                t(10, 17, 10),
                t(10, 17, 5),
                t(10, 17, 0),
                t(10, 16, 55),
                t(10, 16, 50),
                t(10, 16, 45),
            ]
        );
    }

    #[test]
    fn composite_candidates_roll_over_midnight() {
        let candidates = composite_candidates(t(10, 0, 10));
        assert_eq!(candidates[0], t(10, 0, 10));
        assert_eq!(candidates[3], t(9, 23, 55));
    }

    #[test]
    fn locate_frame_splits_past_and_nowcast() {
        let analysis = t(10, 17, 10);
        // The analysis itself: member _000 of the newest tarball.
        assert_eq!(
            locate_frame(analysis, analysis).unwrap(),
            FrameLocation {
                tarball_time: analysis,
                forecast_minutes: 0
            }
        );
        // Nowcast: a member of the newest tarball.
        assert_eq!(
            locate_frame(analysis, t(10, 18, 25)).unwrap(),
            FrameLocation {
                tarball_time: analysis,
                forecast_minutes: 75
            }
        );
        assert_eq!(
            locate_frame(analysis, t(10, 19, 10))
                .unwrap()
                .forecast_minutes,
            120
        );
        // Past: the analysis frame of the older tarball at that time.
        assert_eq!(
            locate_frame(analysis, t(10, 16, 35)).unwrap(),
            FrameLocation {
                tarball_time: t(10, 16, 35),
                forecast_minutes: 0
            }
        );
        assert_eq!(
            locate_frame(analysis, t(10, 15, 10)).unwrap().tarball_time,
            t(10, 15, 10)
        );
    }

    #[test]
    fn locate_frame_rejects_off_grid_and_out_of_window_times() {
        let analysis = t(10, 17, 10);
        // Not on the 5-minute lattice.
        assert!(locate_frame(analysis, t(10, 17, 12)).is_err());
        assert!(
            locate_frame(
                analysis,
                Utc.with_ymd_and_hms(2026, 6, 10, 17, 15, 30).unwrap()
            )
            .is_err()
        );
        // Beyond the nowcast horizon.
        assert!(locate_frame(analysis, t(10, 19, 15)).is_err());
        // Before the observed window.
        assert!(locate_frame(analysis, t(10, 15, 5)).is_err());
    }

    #[test]
    fn timeline_spans_minus_2h_observed_to_plus_2h_forecast() {
        let analysis = t(10, 17, 10);
        let tl = timeline_for(analysis);
        assert_eq!(tl.run_time, analysis);
        assert_eq!(tl.steps.len(), 49);
        assert_eq!(tl.steps[0].valid_time, t(10, 15, 10));
        assert_eq!(tl.steps[0].kind, StepKind::Observed);
        assert_eq!(tl.steps[24].valid_time, analysis);
        assert_eq!(tl.steps[24].kind, StepKind::Observed);
        assert_eq!(tl.steps[25].valid_time, t(10, 17, 15));
        assert_eq!(tl.steps[25].kind, StepKind::Forecast);
        assert_eq!(tl.steps[48].valid_time, t(10, 19, 10));
        assert!(
            tl.steps
                .windows(2)
                .all(|w| w[1].valid_time - w[0].valid_time == chrono::Duration::minutes(5))
        );
        // Every advertised step must be locatable.
        for step in &tl.steps {
            assert!(locate_frame(analysis, step.valid_time).is_ok());
        }
    }
}
