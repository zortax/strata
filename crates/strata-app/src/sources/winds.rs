//! [`WindsAloftSampler`] over prefetched gridded ICON pressure-level winds
//! and temperatures, plus the freezing-level lookup over the same frames.
//!
//! The data side ([`WindsAloftFrames`]) is plain immutable data (`Send +
//! Sync`, grids behind `Arc`s) filled by the winds prefetch in
//! `state::flight` — the 4-level U/V/T grids and the `hzerocl`
//! freezing-level grid for the timeline steps bracketing the flight
//! window. The sampler is constructed on the compute thread from an `Arc`
//! snapshot of it.
//!
//! **Approximations, documented:**
//!
//! - Sampling uses the **nearest fetched step** (the minimal prefetch keeps
//!   only the window-bracketing steps, no temporal interpolation); a query
//!   farther than [`MAX_STEP_DISTANCE_SECS`] from every fetched step
//!   returns `Ok(None)` — `strata-plan` then applies its documented
//!   calm-ISA fallback per leg ([`LegWindOrigin::IsaFallback`]).
//! - **Temperature fallback chain, per level:** the fetched
//!   `Temperature(level)` grid where it samples ([`Provenance::Real`]);
//!   else ISA at the level's ISA altitude ([`Provenance::Isa`] — a missing
//!   or failed temperature fetch never blocks the wind sample). After
//!   vertical interpolation a value is `Real` only when both bracketing
//!   levels were (see [`strata_plan::wind::interpolate_levels`]).
//! - **Freezing-level fallback chain** ([`WindsAloftFrames::freezing_level`]):
//!   the fetched `hzerocl` grid (the model's own 0 °C isotherm height);
//!   else the 0 °C crossing interpolated from the *real* per-level
//!   temperatures; else `None` — the caller then falls back to its
//!   labelled ISA estimate (see `ui::context_tabs::weather`).
//! - Vertical interpolation between the pressure levels is
//!   [`strata_plan::wind::interpolate_levels`] (ISA level altitudes,
//!   component-wise, clamped outside the span — see its docs).
//!
//! [`LegWindOrigin::IsaFallback`]: strata_plan::wind::LegWindOrigin::IsaFallback

use std::sync::Arc;

use chrono::{DateTime, Utc};
use strata_data::domain::{LatLon, MetersAmsl, PressureLevel, WeatherGrid};
use strata_plan::perf::{ISA_LAPSE_CELSIUS_PER_METER, isa_temperature};
use strata_plan::sources::{Provenance, SourceError, WindsAloft, WindsAloftSampler};
use strata_plan::units::Celsius;
use strata_plan::wind::{PressureLevelSample, interpolate_levels};

/// Maximum |query time − step valid time| served by the nearest-step rule,
/// in seconds. ICON-D2 steps are hourly and the prefetch brackets the
/// flight window, so covered queries always find a step within ~1 h; beyond
/// 90 min the data honestly does not cover the time.
pub const MAX_STEP_DISTANCE_SECS: i64 = 90 * 60;

/// One pressure level's grids at one valid time.
#[derive(Debug, Clone)]
pub struct LevelWinds {
    pub level: PressureLevel,
    /// Eastward component, m/s ([`WeatherField::WindU`]).
    ///
    /// [`WeatherField::WindU`]: strata_data::domain::WeatherField::WindU
    pub u: Arc<WeatherGrid>,
    /// Northward component, m/s ([`WeatherField::WindV`]).
    ///
    /// [`WeatherField::WindV`]: strata_data::domain::WeatherField::WindV
    pub v: Arc<WeatherGrid>,
    /// Air temperature, °C ([`WeatherField::Temperature`]); `None` while
    /// the grid has not landed — the sampler then pins the level at its
    /// ISA temperature, honestly labelled [`Provenance::Isa`].
    ///
    /// [`WeatherField::Temperature`]: strata_data::domain::WeatherField::Temperature
    pub temperature: Option<Arc<WeatherGrid>>,
}

/// All levels fetched for one timeline step.
#[derive(Debug, Clone)]
pub struct WindsTimeStep {
    pub valid_time: DateTime<Utc>,
    pub levels: Vec<LevelWinds>,
    /// The `hzerocl` 0 °C-isotherm height grid, m AMSL
    /// ([`WeatherField::FreezingLevel`]); `None` while not fetched.
    ///
    /// [`WeatherField::FreezingLevel`]: strata_data::domain::WeatherField::FreezingLevel
    pub freezing_level: Option<Arc<WeatherGrid>>,
}

/// Where a freezing-level value came from (the documented fallback chain —
/// callers append their own labelled ISA estimate as the final rung).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreezingLevelSource {
    /// The model's own `hzerocl` grid — real forecast data.
    Forecast,
    /// Interpolated 0 °C crossing of the real per-level temperatures.
    FromTemperatures,
}

/// Immutable snapshot of the prefetched winds-aloft grids.
#[derive(Debug, Clone, Default)]
pub struct WindsAloftFrames {
    /// Ascending by valid time.
    steps: Vec<WindsTimeStep>,
}

impl WindsAloftFrames {
    pub fn new(mut steps: Vec<WindsTimeStep>) -> Self {
        steps.sort_by_key(|s| s.valid_time);
        Self { steps }
    }

    // Introspection for tests and the weather-tab surfaces; the compute
    // path itself only samples.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    #[allow(dead_code)]
    pub fn step_count(&self) -> usize {
        self.steps.len()
    }

    /// The step nearest to `t` within [`MAX_STEP_DISTANCE_SECS`], among
    /// the steps satisfying `usable` (wind sampling needs levels, the
    /// freezing-level lookup needs temperature or `hzerocl` data — a step
    /// that only carries the other kind must not shadow a usable one).
    fn nearest_step(
        &self,
        t: DateTime<Utc>,
        usable: impl Fn(&WindsTimeStep) -> bool,
    ) -> Option<&WindsTimeStep> {
        let distance = |s: &WindsTimeStep| (s.valid_time - t).num_seconds().abs();
        self.steps
            .iter()
            .filter(|s| usable(s))
            .min_by_key(|s| distance(s))
            .filter(|s| distance(s) <= MAX_STEP_DISTANCE_SECS)
    }

    /// The freezing level at `position`/`valid_time` per the documented
    /// chain: the `hzerocl` grid where fetched and covering
    /// ([`FreezingLevelSource::Forecast`]); else the 0 °C crossing of the
    /// *real* per-level temperatures, linear in the ISA level altitudes and
    /// extrapolated with the ISA lapse rate beyond the level span
    /// ([`FreezingLevelSource::FromTemperatures`]); else `None` — never an
    /// unlabelled ISA guess.
    pub fn freezing_level(
        &self,
        position: LatLon,
        valid_time: DateTime<Utc>,
    ) -> Option<(MetersAmsl, FreezingLevelSource)> {
        let step = self.nearest_step(valid_time, |s| {
            s.freezing_level.is_some() || s.levels.iter().any(|l| l.temperature.is_some())
        })?;
        if let Some(meters) = step
            .freezing_level
            .as_ref()
            .and_then(|grid| grid.sample(position))
        {
            return Some((
                MetersAmsl(f64::from(meters)),
                FreezingLevelSource::Forecast,
            ));
        }
        // Real temperatures at their ISA level altitudes, ascending.
        let mut profile: Vec<(f64, f64)> = step
            .levels
            .iter()
            .filter_map(|level| {
                let t = level.temperature.as_ref()?.sample(position)?;
                Some((level.level.isa_altitude().0, f64::from(t)))
            })
            .collect();
        profile.sort_by(|a, b| a.0.total_cmp(&b.0));
        let crossing = zero_crossing(&profile)?;
        Some((
            MetersAmsl(crossing.max(0.0)),
            FreezingLevelSource::FromTemperatures,
        ))
    }
}

/// The altitude (m) where a temperature profile `(altitude_m, °C)` —
/// ascending, ≥ 1 point — crosses 0 °C: linear between bracketing points,
/// ISA-lapse extrapolation beyond the span (entirely sub-zero profiles
/// extrapolate downward, entirely positive ones upward). `None` for an
/// empty or isothermal-at-zero-gradient profile that never crosses.
fn zero_crossing(profile: &[(f64, f64)]) -> Option<f64> {
    let (first, last) = (profile.first()?, profile.last()?);
    if first.1 <= 0.0 {
        // Already freezing at the lowest level: extrapolate down the ISA
        // lapse (temperature rises descending).
        return Some(first.0 + first.1 / ISA_LAPSE_CELSIUS_PER_METER);
    }
    for pair in profile.windows(2) {
        let ((h0, t0), (h1, t1)) = (pair[0], pair[1]);
        if t1 <= 0.0 {
            // Sign change in this band (t0 > 0 ≥ t1).
            return Some(h0 + (h1 - h0) * t0 / (t0 - t1));
        }
    }
    // Still positive at the top level: extrapolate up the ISA lapse.
    Some(last.0 + last.1 / ISA_LAPSE_CELSIUS_PER_METER)
}

/// The app's [`WindsAloftSampler`]: nearest fetched step, bilinear in the
/// horizontal, ISA-pinned linear interpolation in the vertical; real
/// temperatures where the grids landed (see the module docs for the
/// fallback chain).
pub struct GriddedWindsAloftSampler {
    frames: Arc<WindsAloftFrames>,
}

impl GriddedWindsAloftSampler {
    pub fn new(frames: Arc<WindsAloftFrames>) -> Self {
        Self { frames }
    }
}

impl WindsAloftSampler for GriddedWindsAloftSampler {
    fn sample(
        &self,
        position: LatLon,
        altitude: MetersAmsl,
        valid_time: DateTime<Utc>,
    ) -> Result<Option<WindsAloft>, SourceError> {
        let Some(step) = self
            .frames
            .nearest_step(valid_time, |s| !s.levels.is_empty())
        else {
            return Ok(None);
        };
        let mut samples = Vec::with_capacity(step.levels.len());
        for level in &step.levels {
            // A level with either component missing at this position
            // (outside the model extent) contributes nothing; if every
            // level drops out the result is honestly None.
            let (Some(u), Some(v)) = (level.u.sample(position), level.v.sample(position)) else {
                continue;
            };
            // Real temperature where the grid landed and covers the
            // position; ISA at the level's ISA altitude otherwise —
            // labelled per level so interpolation can stay honest.
            let (temperature, temperature_provenance) = match level
                .temperature
                .as_ref()
                .and_then(|grid| grid.sample(position))
            {
                Some(t) => (Celsius(f64::from(t)), Provenance::Real),
                None => (
                    isa_temperature(level.level.isa_altitude()),
                    Provenance::Isa,
                ),
            };
            samples.push(PressureLevelSample {
                level: level.level,
                wind_u: f64::from(u),
                wind_v: f64::from(v),
                temperature,
                temperature_provenance,
            });
        }
        Ok(interpolate_levels(&samples, altitude))
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone as _;
    use strata_data::domain::{RegularLatLonGrid, WeatherField};

    use super::*;

    fn t(hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 14, hour, 0, 0).unwrap()
    }

    /// A uniform 2×2 grid over Germany-ish extent.
    fn grid(field: WeatherField, valid: DateTime<Utc>, value: f32) -> Arc<WeatherGrid> {
        Arc::new(WeatherGrid {
            field,
            run_time: valid,
            valid_time: valid,
            grid: RegularLatLonGrid::new(
                LatLon::new(46.0, 5.0).unwrap(),
                10.0,
                10.0,
                2,
                2,
                vec![value; 4],
            )
            .unwrap(),
        })
    }

    fn step(valid: DateTime<Utc>, u: f32, v: f32) -> WindsTimeStep {
        WindsTimeStep {
            valid_time: valid,
            levels: PressureLevel::ALL
                .into_iter()
                .map(|level| LevelWinds {
                    level,
                    u: grid(WeatherField::WindU(level), valid, u),
                    v: grid(WeatherField::WindV(level), valid, v),
                    temperature: None,
                })
                .collect(),
            freezing_level: None,
        }
    }

    /// A step with real temperatures: `t950` at 950 hPa, lapsing by
    /// `lapse_per_level` per level upward.
    fn step_with_temps(
        valid: DateTime<Utc>,
        u: f32,
        v: f32,
        t950: f32,
        lapse_per_level: f32,
    ) -> WindsTimeStep {
        WindsTimeStep {
            valid_time: valid,
            levels: PressureLevel::ALL
                .into_iter()
                .enumerate()
                .map(|(i, level)| LevelWinds {
                    level,
                    u: grid(WeatherField::WindU(level), valid, u),
                    v: grid(WeatherField::WindV(level), valid, v),
                    temperature: Some(grid(
                        WeatherField::Temperature(level),
                        valid,
                        t950 - lapse_per_level * i as f32,
                    )),
                })
                .collect(),
            freezing_level: None,
        }
    }

    fn p() -> LatLon {
        LatLon::new(50.0, 10.0).unwrap()
    }

    #[test]
    fn samples_nearest_step_and_interpolates_levels() {
        // 10 m/s pure westerly (u=+10, v=0) at 09Z; calm at 12Z.
        let frames = Arc::new(WindsAloftFrames::new(vec![
            step(t(12), 0.0, 0.0),
            step(t(9), 10.0, 0.0),
        ]));
        let sampler = GriddedWindsAloftSampler::new(frames);

        let wind = sampler
            .sample(p(), MetersAmsl::from_feet(4500.0), t(10))
            .unwrap()
            .expect("covered time");
        // From the west = blows from 270° true; 10 m/s ≈ 19.4 kt.
        assert!((wind.direction.0 - 270.0).abs() < 1e-6, "{wind:?}");
        assert!((wind.speed.0 - 19.438).abs() < 0.01, "{wind:?}");
        // No temperature grids: ISA OAT at 4500 ft ≈ 15 − 1372 m ×
        // 6.5 °C/km ≈ 6.1 °C, honestly labelled.
        assert!((wind.temperature.0 - 6.08).abs() < 0.2, "{wind:?}");
        assert_eq!(wind.temperature_provenance, Provenance::Isa);

        // 11:30 is nearer to 12Z: calm.
        let calm = sampler
            .sample(
                p(),
                MetersAmsl::from_feet(4500.0),
                t(11) + chrono::Duration::minutes(30),
            )
            .unwrap()
            .expect("covered time");
        assert_eq!(calm.speed.0, 0.0);
    }

    #[test]
    fn real_temperature_grids_override_isa_with_real_provenance() {
        // 20 °C at 950 hPa, −10 °C per level: 950→20, 850→10, 700→0,
        // 500→−10. Markedly warmer than ISA, so the source is decidable.
        let frames = Arc::new(WindsAloftFrames::new(vec![step_with_temps(
            t(9),
            10.0,
            0.0,
            20.0,
            10.0,
        )]));
        let sampler = GriddedWindsAloftSampler::new(frames);

        // Exactly at the 850 hPa ISA altitude: the 850 grid value.
        let at850 = sampler
            .sample(p(), PressureLevel::P850.isa_altitude(), t(9))
            .unwrap()
            .expect("covered");
        assert!((at850.temperature.0 - 10.0).abs() < 1e-6, "{at850:?}");
        assert_eq!(at850.temperature_provenance, Provenance::Real);

        // Midway between 850 and 700 hPa: the arithmetic mean, 5 °C.
        let h_mid = (PressureLevel::P850.isa_altitude().0
            + PressureLevel::P700.isa_altitude().0)
            / 2.0;
        let mid = sampler
            .sample(p(), MetersAmsl(h_mid), t(9))
            .unwrap()
            .expect("covered");
        assert!((mid.temperature.0 - 5.0).abs() < 1e-6, "{mid:?}");
        assert_eq!(mid.temperature_provenance, Provenance::Real);
    }

    #[test]
    fn partially_missing_temperature_grids_degrade_to_isa_per_level() {
        let mut step = step_with_temps(t(9), 10.0, 0.0, 20.0, 10.0);
        // The 700 hPa temperature fetch failed.
        step.levels[2].temperature = None;
        let sampler = GriddedWindsAloftSampler::new(Arc::new(WindsAloftFrames::new(vec![step])));

        // 950–850 band: both levels real.
        let low = sampler
            .sample(p(), MetersAmsl(1000.0), t(9))
            .unwrap()
            .expect("covered");
        assert_eq!(low.temperature_provenance, Provenance::Real);

        // 850–700 band: mixed → honestly Isa.
        let h_mid = (PressureLevel::P850.isa_altitude().0
            + PressureLevel::P700.isa_altitude().0)
            / 2.0;
        let mid = sampler
            .sample(p(), MetersAmsl(h_mid), t(9))
            .unwrap()
            .expect("covered");
        assert_eq!(mid.temperature_provenance, Provenance::Isa);
    }

    #[test]
    fn far_times_and_empty_frames_sample_as_none() {
        let sampler = GriddedWindsAloftSampler::new(Arc::new(WindsAloftFrames::default()));
        assert!(
            sampler
                .sample(p(), MetersAmsl(1000.0), t(9))
                .unwrap()
                .is_none(),
            "no frames at all"
        );

        let frames = Arc::new(WindsAloftFrames::new(vec![step(t(9), 10.0, 0.0)]));
        let sampler = GriddedWindsAloftSampler::new(frames);
        assert!(
            sampler
                .sample(p(), MetersAmsl(1000.0), t(12))
                .unwrap()
                .is_none(),
            "3 h from the only step exceeds MAX_STEP_DISTANCE"
        );
        assert!(
            sampler
                .sample(p(), MetersAmsl(1000.0), t(10))
                .unwrap()
                .is_some(),
            "1 h away is served"
        );
    }

    #[test]
    fn positions_outside_the_model_extent_sample_as_none() {
        let frames = Arc::new(WindsAloftFrames::new(vec![step(t(9), 10.0, 0.0)]));
        let sampler = GriddedWindsAloftSampler::new(frames);
        // The synthetic grid spans 46–56°N, 5–15°E; Lisbon is outside.
        let lisbon = LatLon::new(38.7, -9.1).unwrap();
        assert!(
            sampler
                .sample(lisbon, MetersAmsl(1000.0), t(9))
                .unwrap()
                .is_none()
        );
    }

    // --- freezing level -------------------------------------------------

    #[test]
    fn freezing_level_prefers_the_hzerocl_grid() {
        let mut with_temps = step_with_temps(t(9), 10.0, 0.0, 20.0, 10.0);
        with_temps.freezing_level = Some(grid(WeatherField::FreezingLevel, t(9), 2800.0));
        let frames = WindsAloftFrames::new(vec![with_temps]);

        let (level, source) = frames.freezing_level(p(), t(9)).expect("covered");
        assert_eq!(source, FreezingLevelSource::Forecast);
        assert!((level.0 - 2800.0).abs() < 1e-3, "{level:?}");
    }

    #[test]
    fn freezing_level_interpolates_real_temperatures_without_hzerocl() {
        // 950→20 °C, 850→10, 700→0, 500→−10: the 0 °C crossing sits exactly
        // at the 700 hPa ISA altitude (~3012 m).
        let frames = WindsAloftFrames::new(vec![step_with_temps(t(9), 10.0, 0.0, 20.0, 10.0)]);
        let (level, source) = frames.freezing_level(p(), t(9)).expect("covered");
        assert_eq!(source, FreezingLevelSource::FromTemperatures);
        assert!(
            (level.0 - PressureLevel::P700.isa_altitude().0).abs() < 1.0,
            "{level:?}"
        );
    }

    #[test]
    fn freezing_level_extrapolates_outside_the_level_span() {
        // Entirely positive profile (winter inversion inverted: warm
        // everywhere): extrapolates above the 500 hPa level by t/lapse.
        let warm = WindsAloftFrames::new(vec![step_with_temps(t(9), 0.0, 0.0, 20.0, 2.0)]);
        let (level, source) = warm.freezing_level(p(), t(9)).expect("covered");
        assert_eq!(source, FreezingLevelSource::FromTemperatures);
        let top = PressureLevel::P500.isa_altitude().0;
        let expected = top + 14.0 / ISA_LAPSE_CELSIUS_PER_METER;
        assert!((level.0 - expected).abs() < 1.0, "{level:?}");

        // Entirely sub-zero: extrapolates below the 950 hPa level, clamped
        // at the surface.
        let cold = WindsAloftFrames::new(vec![step_with_temps(t(9), 0.0, 0.0, -2.0, 5.0)]);
        let (level, _) = cold.freezing_level(p(), t(9)).expect("covered");
        let bottom = PressureLevel::P950.isa_altitude().0;
        let expected = (bottom - 2.0 / ISA_LAPSE_CELSIUS_PER_METER).max(0.0);
        assert!((level.0 - expected).abs() < 1.0, "{level:?}");
    }

    #[test]
    fn freezing_level_is_honestly_none_without_data() {
        // U/V only — no temperatures, no hzerocl: the chain ends, the
        // caller labels its ISA estimate itself.
        let frames = WindsAloftFrames::new(vec![step(t(9), 10.0, 0.0)]);
        assert_eq!(frames.freezing_level(p(), t(9)), None);
        // Outside the model extent.
        let with_temps = WindsAloftFrames::new(vec![step_with_temps(t(9), 0.0, 0.0, 20.0, 10.0)]);
        let lisbon = LatLon::new(38.7, -9.1).unwrap();
        assert_eq!(with_temps.freezing_level(lisbon, t(9)), None);
        // Beyond the serving distance.
        assert_eq!(with_temps.freezing_level(p(), t(12)), None);
    }
}
