//! Pure ICON-D2 open-data layout logic: field-name mapping, file/URL
//! construction, run-hour candidates, and valid-time → step math.
//!
//! Layout (verified live 2026-06-10, pressure-level files 2026-06-11):
//!
//! ```text
//! https://opendata.dwd.de/weather/nwp/icon-d2/grib/{HH}/{field}/
//!   icon-d2_germany_regular-lat-lon_single-level_{YYYYMMDDHH}_{SSS}_2d_{field}.grib2.bz2
//!   icon-d2_germany_regular-lat-lon_pressure-level_{YYYYMMDDHH}_{SSS}_{level}_{field}.grib2.bz2
//! ```
//!
//! with `HH` ∈ {00, 03, …, 21} (a new run appears ~1 h after run time) and
//! 49 hourly steps `000..=048` per field — for both kinds; e.g. verified
//! live on 2026-06-11:
//!
//! ```text
//! …/00/u/icon-d2_germany_regular-lat-lon_pressure-level_2026061100_001_850_u.grib2.bz2
//! …/00/hzerocl/icon-d2_germany_regular-lat-lon_single-level_2026061100_048_2d_hzerocl.grib2.bz2
//! ```
//!
//! `{level}` is the unpadded pressure in hPa; the server publishes
//! 1000/975/950/850/700/600/500/400/300/250/200 for `u`, `v`, `t` — we use
//! the [`crate::domain::PressureLevel`] subset. The run directories also
//! hold icosahedral-grid and model-level files — the
//! `regular-lat-lon_{single,pressure}-level` file-name prefix is the filter.

use chrono::{DateTime, Duration, Timelike, Utc};

use crate::domain::WeatherField;

use super::DwdIconError;

/// Steps per run: hourly `000..=048`.
pub(super) const FORECAST_HOURS: u32 = 48;

/// Hours between runs (00, 03, …, 21 UTC).
const RUN_INTERVAL_HOURS: u32 = 3;

/// How many recent runs to probe before giving up (newest plus two
/// fallbacks covers the ~1 h publication lag and a missed run).
pub(super) const RUN_CANDIDATES: usize = 3;

/// DWD open-data field directory / file-name token for `field`.
pub(super) fn dwd_field_name(field: WeatherField) -> &'static str {
    match field {
        WeatherField::CloudCover => "clct",
        WeatherField::CloudCoverLow => "clcl",
        WeatherField::CloudCoverMid => "clcm",
        WeatherField::CloudCoverHigh => "clch",
        // The rate is derived by differencing consecutive run-start
        // accumulations; see `decode::accumulation_rate_mm_h`.
        WeatherField::PrecipRate => "tot_prec",
        WeatherField::ThunderstormPotential => "lpi",
        WeatherField::Cape => "cape_ml",
        WeatherField::Ceiling => "ceiling",
        WeatherField::Visibility => "vis",
        WeatherField::WindU(_) => "u",
        WeatherField::WindV(_) => "v",
        WeatherField::Temperature(_) => "t",
        WeatherField::FreezingLevel => "hzerocl",
    }
}

/// File name for one step of one field of one run: pressure-level fields
/// use the `pressure-level_…_{hPa}_{field}` pattern, everything else the
/// `single-level_…_2d_{field}` pattern (both verified live, see module
/// docs).
pub(super) fn file_name(field: WeatherField, run: DateTime<Utc>, step: u32) -> String {
    let run = run.format("%Y%m%d%H");
    let name = dwd_field_name(field);
    match field.pressure_level() {
        Some(level) => format!(
            "icon-d2_germany_regular-lat-lon_pressure-level_{run}_{step:03}_{}_{name}.grib2.bz2",
            level.hpa(),
        ),
        None => format!(
            "icon-d2_germany_regular-lat-lon_single-level_{run}_{step:03}_2d_{name}.grib2.bz2"
        ),
    }
}

/// Full download URL for one step of one field of one run.
pub(super) fn step_url(
    base_url: &str,
    field: WeatherField,
    run: DateTime<Utc>,
    step: u32,
) -> String {
    format!(
        "{base_url}/{:02}/{}/{}",
        run.hour(),
        dwd_field_name(field),
        file_name(field, run, step),
    )
}

/// The newest possible run for `now` followed by older fallbacks, newest
/// first. The newest candidate may not be published yet (~1 h lag) — the
/// caller probes each in turn.
pub(super) fn run_candidates(now: DateTime<Utc>) -> Vec<DateTime<Utc>> {
    let newest = now
        .with_minute(0)
        .and_then(|t| t.with_second(0))
        .and_then(|t| t.with_nanosecond(0))
        .unwrap_or(now)
        - Duration::hours(i64::from(now.hour() % RUN_INTERVAL_HOURS));
    (0..RUN_CANDIDATES)
        .map(|n| newest - Duration::hours(i64::from(RUN_INTERVAL_HOURS) * n as i64))
        .collect()
}

/// The first fetchable step of a run for `field`: precipitation rates need
/// the previous accumulation to diff against, so step 0 has no rate.
pub(super) fn first_step(field: WeatherField) -> u32 {
    match field {
        WeatherField::PrecipRate => 1,
        _ => 0,
    }
}

/// Maps `valid_time` onto the hourly step of `run`, rejecting times that
/// are not a fetchable step for `field`.
pub(super) fn step_for(
    field: WeatherField,
    run: DateTime<Utc>,
    valid_time: DateTime<Utc>,
) -> Result<u32, DwdIconError> {
    let err = || DwdIconError::InvalidValidTime {
        field,
        valid_time,
        run,
    };
    let delta = valid_time - run;
    if delta < Duration::zero() || delta.num_seconds() % 3600 != 0 {
        return Err(err());
    }
    let step = u32::try_from(delta.num_hours()).map_err(|_| err())?;
    if step < first_step(field) || step > FORECAST_HOURS {
        return Err(err());
    }
    Ok(step)
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::domain::PressureLevel;

    fn t(d: u32, h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, d, h, m, 0).unwrap()
    }

    #[test]
    fn field_names_match_the_dwd_directories() {
        let expect = [
            (WeatherField::CloudCover, "clct"),
            (WeatherField::CloudCoverLow, "clcl"),
            (WeatherField::CloudCoverMid, "clcm"),
            (WeatherField::CloudCoverHigh, "clch"),
            (WeatherField::PrecipRate, "tot_prec"),
            (WeatherField::ThunderstormPotential, "lpi"),
            (WeatherField::Cape, "cape_ml"),
            (WeatherField::Ceiling, "ceiling"),
            (WeatherField::Visibility, "vis"),
            (WeatherField::WindU(PressureLevel::P950), "u"),
            (WeatherField::WindV(PressureLevel::P850), "v"),
            (WeatherField::Temperature(PressureLevel::P700), "t"),
            (WeatherField::FreezingLevel, "hzerocl"),
        ];
        for (field, name) in expect {
            assert_eq!(dwd_field_name(field), name, "{field}");
        }
    }

    #[test]
    fn file_name_matches_the_published_pattern() {
        // Verified against the live server listing on 2026-06-10.
        assert_eq!(
            file_name(WeatherField::ThunderstormPotential, t(10, 15, 0), 1),
            "icon-d2_germany_regular-lat-lon_single-level_2026061015_001_2d_lpi.grib2.bz2"
        );
        assert_eq!(
            file_name(WeatherField::CloudCover, t(10, 0, 0), 48),
            "icon-d2_germany_regular-lat-lon_single-level_2026061000_048_2d_clct.grib2.bz2"
        );
        // Verified against the live server listing on 2026-06-11: hzerocl
        // is a plain single-level field …
        assert_eq!(
            file_name(WeatherField::FreezingLevel, t(11, 0, 0), 48),
            "icon-d2_germany_regular-lat-lon_single-level_2026061100_048_2d_hzerocl.grib2.bz2"
        );
        // … while u/v/t use the pressure-level pattern with the unpadded
        // hPa value between step and field name.
        assert_eq!(
            file_name(WeatherField::WindU(PressureLevel::P850), t(11, 0, 0), 1),
            "icon-d2_germany_regular-lat-lon_pressure-level_2026061100_001_850_u.grib2.bz2"
        );
        assert_eq!(
            file_name(WeatherField::WindV(PressureLevel::P500), t(11, 0, 0), 7),
            "icon-d2_germany_regular-lat-lon_pressure-level_2026061100_007_500_v.grib2.bz2"
        );
        assert_eq!(
            file_name(
                WeatherField::Temperature(PressureLevel::P950),
                t(11, 12, 0),
                0
            ),
            "icon-d2_germany_regular-lat-lon_pressure-level_2026061112_000_950_t.grib2.bz2"
        );
    }

    #[test]
    fn step_url_includes_run_hour_and_field_dir() {
        assert_eq!(
            step_url(
                "https://example.test/grib",
                WeatherField::Cape,
                t(10, 9, 0),
                7
            ),
            "https://example.test/grib/09/cape_ml/icon-d2_germany_regular-lat-lon_single-level_2026061009_007_2d_cape_ml.grib2.bz2"
        );
        // Pressure-level fields live in the bare u/v/t directories.
        assert_eq!(
            step_url(
                "https://example.test/grib",
                WeatherField::Temperature(PressureLevel::P700),
                t(11, 0, 0),
                3
            ),
            "https://example.test/grib/00/t/icon-d2_germany_regular-lat-lon_pressure-level_2026061100_003_700_t.grib2.bz2"
        );
    }

    #[test]
    fn run_candidates_floor_to_the_3h_grid() {
        assert_eq!(
            run_candidates(t(10, 16, 57)),
            vec![t(10, 15, 0), t(10, 12, 0), t(10, 9, 0)]
        );
        // Exactly on a run hour.
        assert_eq!(
            run_candidates(t(10, 12, 0)),
            vec![t(10, 12, 0), t(10, 9, 0), t(10, 6, 0)]
        );
    }

    #[test]
    fn run_candidates_roll_over_midnight() {
        assert_eq!(
            run_candidates(t(10, 1, 30)),
            vec![t(10, 0, 0), t(9, 21, 0), t(9, 18, 0)]
        );
    }

    #[test]
    fn step_for_accepts_whole_hours_within_the_run() {
        let run = t(10, 12, 0);
        assert_eq!(
            step_for(WeatherField::CloudCover, run, t(10, 12, 0)).unwrap(),
            0
        );
        assert_eq!(
            step_for(WeatherField::CloudCover, run, t(10, 15, 0)).unwrap(),
            3
        );
        assert_eq!(
            step_for(WeatherField::CloudCover, run, t(12, 12, 0)).unwrap(),
            48
        );
    }

    #[test]
    fn step_for_rejects_out_of_range_and_sub_hour_times() {
        let run = t(10, 12, 0);
        assert!(step_for(WeatherField::CloudCover, run, t(10, 11, 0)).is_err()); // before run
        assert!(step_for(WeatherField::CloudCover, run, t(10, 13, 30)).is_err()); // not hourly
        assert!(step_for(WeatherField::CloudCover, run, t(12, 13, 0)).is_err()); // step 49
    }

    #[test]
    fn precip_rate_has_no_step_zero() {
        let run = t(10, 12, 0);
        assert!(step_for(WeatherField::PrecipRate, run, run).is_err());
        assert_eq!(
            step_for(WeatherField::PrecipRate, run, t(10, 13, 0)).unwrap(),
            1
        );
        assert_eq!(first_step(WeatherField::PrecipRate), 1);
        assert_eq!(first_step(WeatherField::CloudCover), 0);
    }
}
