//! Gridded weather fields: regular lat-lon rasters of a single scalar
//! quantity at one valid time, plus the timeline metadata describing which
//! valid times a source can currently deliver.
//!
//! Grids are transient (fetched live, never persisted as authoritative) and
//! use `NaN` for no-data, so they intentionally carry no serde derives.

use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::domain::geo::{BoundingBox, GeoError, LatLon};
use crate::domain::vertical::MetersAmsl;

/// Errors constructing gridded-weather primitives.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum GriddedError {
    #[error("grid needs at least 2x2 points, got {ni}x{nj}")]
    GridTooSmall { ni: usize, nj: usize },
    #[error(
        "grid spacing must be finite and positive, got {lat_spacing_deg} x {lon_spacing_deg} deg"
    )]
    InvalidSpacing {
        lat_spacing_deg: f64,
        lon_spacing_deg: f64,
    },
    #[error("{got} values do not match {ni}x{nj} grid")]
    ValueCountMismatch { got: usize, ni: usize, nj: usize },
    #[error("grid extent leaves valid coordinates: {0}")]
    Extent(#[from] GeoError),
}

/// A pressure level carried by upper-air gridded fields (winds aloft,
/// temperature aloft). The four levels span the VFR altitude band over
/// Germany.
///
/// Serde note: serializes as the bare variant name (`"P850"`); the
/// containing [`WeatherField`] tuple variants therefore serialize as e.g.
/// `{"WindU":"P850"}` in JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PressureLevel {
    /// 950 hPa — ≈ 540 m / 1 770 ft ISA.
    P950,
    /// 850 hPa — ≈ 1 457 m / 4 780 ft ISA.
    P850,
    /// 700 hPa — ≈ 3 012 m / 9 880 ft ISA.
    P700,
    /// 500 hPa — ≈ 5 574 m / 18 290 ft ISA.
    P500,
}

impl PressureLevel {
    /// Every level, descending pressure (ascending altitude).
    pub const ALL: [PressureLevel; 4] = [Self::P950, Self::P850, Self::P700, Self::P500];

    /// Pressure in hectopascals.
    pub fn hpa(&self) -> u16 {
        match self {
            Self::P950 => 950,
            Self::P850 => 850,
            Self::P700 => 700,
            Self::P500 => 500,
        }
    }

    /// ICAO standard-atmosphere (ISA) pressure altitude of this level:
    /// `h = T₀/L · (1 − (p/p₀)^(R·L/(g·M)))` with the tropospheric
    /// constants T₀ = 288.15 K, L = 6.5 K/km, p₀ = 1013.25 hPa and
    /// exponent R·L/(g·M) ≈ 0.190263.
    ///
    /// This is an **approximation**: the true geometric altitude of a
    /// pressure surface varies with the actual surface pressure and
    /// temperature profile (order ±100–300 m in mid-latitude weather).
    /// Good enough for choosing/interpolating between levels when sampling
    /// winds aloft for planning — the conventional approach — but never an
    /// altimetry source.
    pub fn isa_altitude(&self) -> MetersAmsl {
        const ISA_SEA_LEVEL_K: f64 = 288.15;
        const ISA_LAPSE_K_PER_M: f64 = 0.0065;
        const ISA_SEA_LEVEL_HPA: f64 = 1013.25;
        const ISA_EXPONENT: f64 = 0.190_263;
        let ratio = f64::from(self.hpa()) / ISA_SEA_LEVEL_HPA;
        MetersAmsl(ISA_SEA_LEVEL_K / ISA_LAPSE_K_PER_M * (1.0 - ratio.powf(ISA_EXPONENT)))
    }
}

impl fmt::Display for PressureLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} hPa", self.hpa())
    }
}

/// A scalar weather quantity available as a gridded field.
///
/// Each variant fixes the unit of the grid values (see [`Self::unit`]):
///
/// | Variant | Values |
/// |---|---|
/// | `CloudCover*` | cover in percent, 0–100 |
/// | `PrecipRate` | precipitation rate in mm/h |
/// | `ThunderstormPotential` | Lightning Potential Index in J/kg (raw model values; ~0 calm, >~5 indicates storm potential) |
/// | `Cape` | mixed-layer CAPE in J/kg |
/// | `Ceiling` | cloud ceiling in meters above ground — wrap as `MetersAgl(f64::from(v))` at the consumer edge |
/// | `Visibility` | horizontal visibility in meters |
/// | `WindU`/`WindV` | wind components at a pressure level in m/s (u: toward east, v: toward north) |
/// | `Temperature` | air temperature at a pressure level in °C (providers convert from Kelvin) |
/// | `FreezingLevel` | height of the 0 °C isotherm in meters AMSL — wrap as `MetersAmsl(f64::from(v))` at the consumer edge |
///
/// `Ceiling` and `Visibility` are ingested for the future profile view /
/// visibility cone and are not visualized yet. The pressure-level fields
/// and `FreezingLevel` feed flight planning (winds-aloft sampling).
///
/// Serde compatibility: the enum is additive-only — the original unit
/// variants keep their bare-string form (`"CloudCover"`), the
/// pressure-level variants are externally tagged (`{"WindU":"P850"}`).
/// Nothing persists this type today (grids are transient); the derives
/// exist for consumers embedding the field in their own keys/configs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WeatherField {
    /// Total cloud cover, percent 0–100.
    CloudCover,
    /// Low-level cloud cover (surface to ~2 km), percent 0–100.
    CloudCoverLow,
    /// Mid-level cloud cover (~2–7 km), percent 0–100.
    CloudCoverMid,
    /// High-level cloud cover (above ~7 km), percent 0–100.
    CloudCoverHigh,
    /// Precipitation rate, mm/h.
    PrecipRate,
    /// Thunderstorm potential as Lightning Potential Index, J/kg.
    ThunderstormPotential,
    /// Convective available potential energy (mixed layer), J/kg.
    Cape,
    /// Cloud ceiling height, meters above ground (`MetersAgl` datum).
    Ceiling,
    /// Horizontal visibility, meters.
    Visibility,
    /// Wind u-component (toward east) at a pressure level, m/s.
    WindU(PressureLevel),
    /// Wind v-component (toward north) at a pressure level, m/s.
    WindV(PressureLevel),
    /// Air temperature at a pressure level, °C.
    Temperature(PressureLevel),
    /// Height of the 0 °C isotherm, meters AMSL (`MetersAmsl` datum).
    FreezingLevel,
}

impl WeatherField {
    /// Every field, in display order (pressure-level fields grouped by
    /// quantity, ascending altitude).
    pub const ALL: [WeatherField; 22] = [
        Self::CloudCover,
        Self::CloudCoverLow,
        Self::CloudCoverMid,
        Self::CloudCoverHigh,
        Self::PrecipRate,
        Self::ThunderstormPotential,
        Self::Cape,
        Self::Ceiling,
        Self::Visibility,
        Self::WindU(PressureLevel::P950),
        Self::WindU(PressureLevel::P850),
        Self::WindU(PressureLevel::P700),
        Self::WindU(PressureLevel::P500),
        Self::WindV(PressureLevel::P950),
        Self::WindV(PressureLevel::P850),
        Self::WindV(PressureLevel::P700),
        Self::WindV(PressureLevel::P500),
        Self::Temperature(PressureLevel::P950),
        Self::Temperature(PressureLevel::P850),
        Self::Temperature(PressureLevel::P700),
        Self::Temperature(PressureLevel::P500),
        Self::FreezingLevel,
    ];

    /// Display unit of the grid values.
    pub fn unit(&self) -> &'static str {
        match self {
            Self::CloudCover | Self::CloudCoverLow | Self::CloudCoverMid | Self::CloudCoverHigh => {
                "%"
            }
            Self::PrecipRate => "mm/h",
            Self::ThunderstormPotential | Self::Cape => "J/kg",
            Self::Ceiling => "m AGL",
            Self::Visibility => "m",
            Self::WindU(_) | Self::WindV(_) => "m/s",
            Self::Temperature(_) => "°C",
            Self::FreezingLevel => "m AMSL",
        }
    }

    /// The pressure level of an upper-air field, `None` for surface/column
    /// fields.
    pub fn pressure_level(&self) -> Option<PressureLevel> {
        match self {
            Self::WindU(level) | Self::WindV(level) | Self::Temperature(level) => Some(*level),
            _ => None,
        }
    }
}

impl fmt::Display for WeatherField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CloudCover => f.write_str("cloud cover"),
            Self::CloudCoverLow => f.write_str("low cloud cover"),
            Self::CloudCoverMid => f.write_str("mid cloud cover"),
            Self::CloudCoverHigh => f.write_str("high cloud cover"),
            Self::PrecipRate => f.write_str("precipitation rate"),
            Self::ThunderstormPotential => f.write_str("thunderstorm potential"),
            Self::Cape => f.write_str("CAPE"),
            Self::Ceiling => f.write_str("ceiling"),
            Self::Visibility => f.write_str("visibility"),
            Self::WindU(level) => write!(f, "wind u {level}"),
            Self::WindV(level) => write!(f, "wind v {level}"),
            Self::Temperature(level) => write!(f, "temperature {level}"),
            Self::FreezingLevel => f.write_str("freezing level"),
        }
    }
}

/// A regular (equiangular) WGS84 lat-lon raster of `f32` samples.
///
/// **Storage convention** (providers normalize whatever scan order the
/// source uses into this): row-major from the south-west grid node —
/// `values[j * ni + i]` where `i` counts west→east columns and `j` counts
/// south→north rows. `NaN` marks no-data; everything else is real data.
#[derive(Debug, Clone, PartialEq)]
pub struct RegularLatLonGrid {
    /// South-west grid node.
    origin: LatLon,
    lat_spacing_deg: f64,
    lon_spacing_deg: f64,
    ni: usize,
    nj: usize,
    /// Node extent (south-west node to north-east node), precomputed.
    extent: BoundingBox,
    values: Vec<f32>,
}

impl RegularLatLonGrid {
    /// Builds a grid; `values` must hold exactly `ni * nj` samples laid out
    /// in the documented convention and the north-east node must stay
    /// within valid coordinates.
    pub fn new(
        origin: LatLon,
        lat_spacing_deg: f64,
        lon_spacing_deg: f64,
        ni: usize,
        nj: usize,
        values: Vec<f32>,
    ) -> Result<Self, GriddedError> {
        if ni < 2 || nj < 2 {
            return Err(GriddedError::GridTooSmall { ni, nj });
        }
        let spacing_ok = |s: f64| s.is_finite() && s > 0.0;
        if !spacing_ok(lat_spacing_deg) || !spacing_ok(lon_spacing_deg) {
            return Err(GriddedError::InvalidSpacing {
                lat_spacing_deg,
                lon_spacing_deg,
            });
        }
        if values.len() != ni * nj {
            return Err(GriddedError::ValueCountMismatch {
                got: values.len(),
                ni,
                nj,
            });
        }
        let north_east = LatLon::new(
            origin.lat() + (nj - 1) as f64 * lat_spacing_deg,
            origin.lon() + (ni - 1) as f64 * lon_spacing_deg,
        )?;
        let extent = BoundingBox::from_corners(origin, north_east)?;
        Ok(Self {
            origin,
            lat_spacing_deg,
            lon_spacing_deg,
            ni,
            nj,
            extent,
            values,
        })
    }

    /// South-west grid node.
    pub fn origin(&self) -> LatLon {
        self.origin
    }

    /// Latitude step between adjacent rows, degrees (positive).
    pub fn lat_spacing_deg(&self) -> f64 {
        self.lat_spacing_deg
    }

    /// Longitude step between adjacent columns, degrees (positive).
    pub fn lon_spacing_deg(&self) -> f64 {
        self.lon_spacing_deg
    }

    /// Number of columns (west→east).
    pub fn ni(&self) -> usize {
        self.ni
    }

    /// Number of rows (south→north).
    pub fn nj(&self) -> usize {
        self.nj
    }

    /// All samples in the documented `values[j * ni + i]` layout.
    pub fn values(&self) -> &[f32] {
        &self.values
    }

    /// The sample at column `i`, row `j`; `None` outside the grid. The
    /// returned value may be `NaN` (no-data).
    pub fn value_at(&self, i: usize, j: usize) -> Option<f32> {
        (i < self.ni && j < self.nj).then(|| self.values[j * self.ni + i])
    }

    /// Geographic extent spanned by the grid nodes (south-west node to
    /// north-east node, inclusive).
    pub fn extent(&self) -> BoundingBox {
        self.extent
    }

    /// Bilinear sample at `p`.
    ///
    /// Returns `None` outside [`Self::extent`]. `NaN` corners are dropped
    /// and the remaining corner weights renormalized, so points near a
    /// no-data edge still sample from the valid side; when no corner with
    /// positive weight holds data (e.g. exactly on a no-data node, or a
    /// fully masked cell) the result is `None`.
    pub fn sample(&self, p: LatLon) -> Option<f32> {
        let x = (p.lon() - self.origin.lon()) / self.lon_spacing_deg;
        let y = (p.lat() - self.origin.lat()) / self.lat_spacing_deg;
        if x < 0.0 || y < 0.0 || x > (self.ni - 1) as f64 || y > (self.nj - 1) as f64 {
            return None;
        }
        // Clamp so the north/east edges fall into the last cell with
        // fractional coordinate 1.0.
        let i0 = (x.floor() as usize).min(self.ni - 2);
        let j0 = (y.floor() as usize).min(self.nj - 2);
        let fx = x - i0 as f64;
        let fy = y - j0 as f64;
        let corners = [
            (i0, j0, (1.0 - fx) * (1.0 - fy)),
            (i0 + 1, j0, fx * (1.0 - fy)),
            (i0, j0 + 1, (1.0 - fx) * fy),
            (i0 + 1, j0 + 1, fx * fy),
        ];
        let mut value_sum = 0.0_f64;
        let mut weight_sum = 0.0_f64;
        for (i, j, w) in corners {
            let v = self.values[j * self.ni + i];
            if w > 0.0 && !v.is_nan() {
                value_sum += f64::from(v) * w;
                weight_sum += w;
            }
        }
        (weight_sum > 0.0).then(|| (value_sum / weight_sum) as f32)
    }

    /// The sub-grid covering `bbox`: the requested box is clamped to
    /// [`Self::extent`] and expanded outward to whole grid nodes (floor
    /// west/south, ceil east/north), keeping at least 2×2 nodes so the
    /// result stays bilinearly sampleable. [`Self::sample`] over any point
    /// inside the clamped box is identical on the crop and the original.
    /// `None` when `bbox` misses the grid entirely.
    ///
    /// The motivating consumer is the flight winds prefetch, which crops
    /// full-domain ICON-D2 grids (3.6 MB each, ~190 MB per fetch window)
    /// down to the route corridor.
    pub fn crop(&self, bbox: BoundingBox) -> Option<Self> {
        // Clamp to the node extent.
        let west = bbox.west().max(self.extent.west());
        let east = bbox.east().min(self.extent.east());
        let south = bbox.south().max(self.extent.south());
        let north = bbox.north().min(self.extent.north());
        if west > east || south > north {
            return None;
        }
        // Expand to whole node indices (the clamp keeps them in range;
        // the extra clamp below guards float edge cases only).
        let node = |delta: f64, spacing: f64, round_up: bool, max: usize| -> usize {
            let x = delta / spacing;
            let idx = if round_up { x.ceil() } else { x.floor() };
            (idx.max(0.0) as usize).min(max)
        };
        let mut i0 = node(
            west - self.origin.lon(),
            self.lon_spacing_deg,
            false,
            self.ni - 1,
        );
        let mut i1 = node(
            east - self.origin.lon(),
            self.lon_spacing_deg,
            true,
            self.ni - 1,
        );
        let mut j0 = node(
            south - self.origin.lat(),
            self.lat_spacing_deg,
            false,
            self.nj - 1,
        );
        let mut j1 = node(
            north - self.origin.lat(),
            self.lat_spacing_deg,
            true,
            self.nj - 1,
        );
        // Keep at least two nodes per axis (`ni >= 2` by construction).
        if i0 == i1 {
            if i1 + 1 < self.ni {
                i1 += 1;
            } else {
                i0 -= 1;
            }
        }
        if j0 == j1 {
            if j1 + 1 < self.nj {
                j1 += 1;
            } else {
                j0 -= 1;
            }
        }
        let ni = i1 - i0 + 1;
        let nj = j1 - j0 + 1;
        let mut values = Vec::with_capacity(ni * nj);
        for j in j0..=j1 {
            let start = j * self.ni + i0;
            values.extend_from_slice(&self.values[start..start + ni]);
        }
        // The origin shifts by whole spacing multiples, so cropped node
        // positions coincide exactly with the original's.
        let origin = LatLon::new(
            self.origin.lat() + j0 as f64 * self.lat_spacing_deg,
            self.origin.lon() + i0 as f64 * self.lon_spacing_deg,
        )
        .ok()?;
        Self::new(
            origin,
            self.lat_spacing_deg,
            self.lon_spacing_deg,
            ni,
            nj,
            values,
        )
        .ok()
    }
}

/// One [`WeatherField`] raster at one valid time.
#[derive(Debug, Clone, PartialEq)]
pub struct WeatherGrid {
    pub field: WeatherField,
    /// Model run (forecast) or observation analysis time the data belongs to.
    pub run_time: DateTime<Utc>,
    /// The instant the values are valid for.
    pub valid_time: DateTime<Utc>,
    pub grid: RegularLatLonGrid,
}

impl WeatherGrid {
    /// Bilinear sample at `p`; see [`RegularLatLonGrid::sample`].
    pub fn sample(&self, p: LatLon) -> Option<f32> {
        self.grid.sample(p)
    }

    /// This grid cropped to `bbox` (see [`RegularLatLonGrid::crop`]);
    /// `None` when the bbox misses the grid entirely.
    pub fn cropped(&self, bbox: BoundingBox) -> Option<Self> {
        Some(Self {
            field: self.field,
            run_time: self.run_time,
            valid_time: self.valid_time,
            grid: self.grid.crop(bbox)?,
        })
    }
}

/// Whether a timeline step is measured past data or model future.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StepKind {
    /// Observed/analyzed data (e.g. a past radar composite frame).
    Observed,
    /// Model or nowcast forecast.
    Forecast,
}

/// One fetchable valid time of a gridded source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TimelineStep {
    pub valid_time: DateTime<Utc>,
    pub kind: StepKind,
}

/// What a gridded source can currently deliver: the run/analysis the data
/// belongs to plus every fetchable valid time, ascending. A pure-forecast
/// model advertises only [`StepKind::Forecast`] steps from the run onward;
/// an observation source advertises a past [`StepKind::Observed`] window,
/// optionally followed by a short nowcast.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GriddedTimeline {
    /// Model run / observation analysis time.
    pub run_time: DateTime<Utc>,
    /// Fetchable valid times, ascending.
    pub steps: Vec<TimelineStep>,
}

impl GriddedTimeline {
    /// First and last fetchable valid time; `None` for an empty timeline.
    pub fn valid_range(&self) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
        let first = self.steps.first()?;
        let last = self.steps.last()?;
        Some((first.valid_time, last.valid_time))
    }

    /// The step whose valid time is closest to `t` (earlier step wins
    /// ties); `None` for an empty timeline.
    pub fn nearest_step(&self, t: DateTime<Utc>) -> Option<TimelineStep> {
        self.steps
            .iter()
            .min_by_key(|s| (s.valid_time - t).abs())
            .copied()
    }

    /// The steps bracketing `t` for slider interpolation/crossfade: the
    /// latest step at-or-before and the earliest step at-or-after. Both are
    /// the same step when `t` hits a step exactly; either side is `None`
    /// when `t` lies outside the timeline.
    pub fn bracketing_steps(
        &self,
        t: DateTime<Utc>,
    ) -> (Option<TimelineStep>, Option<TimelineStep>) {
        let before = self
            .steps
            .iter()
            .filter(|s| s.valid_time <= t)
            .max_by_key(|s| s.valid_time)
            .copied();
        let after = self
            .steps
            .iter()
            .filter(|s| s.valid_time >= t)
            .min_by_key(|s| s.valid_time)
            .copied();
        (before, after)
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn ll(lat: f64, lon: f64) -> LatLon {
        LatLon::new(lat, lon).unwrap()
    }

    /// 3 columns x 2 rows around (50N, 10E), 1 deg spacing:
    ///
    /// ```text
    /// j=1 (51N):  4 5 6
    /// j=0 (50N):  1 2 3
    ///             10E 11E 12E
    /// ```
    fn grid() -> RegularLatLonGrid {
        RegularLatLonGrid::new(
            ll(50.0, 10.0),
            1.0,
            1.0,
            3,
            2,
            vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
        )
        .unwrap()
    }

    #[test]
    fn constructor_validates() {
        assert!(matches!(
            RegularLatLonGrid::new(ll(50.0, 10.0), 1.0, 1.0, 1, 2, vec![1.0, 2.0]),
            Err(GriddedError::GridTooSmall { ni: 1, nj: 2 })
        ));
        assert!(matches!(
            RegularLatLonGrid::new(ll(50.0, 10.0), 0.0, 1.0, 2, 2, vec![0.0; 4]),
            Err(GriddedError::InvalidSpacing { .. })
        ));
        assert!(matches!(
            RegularLatLonGrid::new(ll(50.0, 10.0), 1.0, 1.0, 2, 2, vec![0.0; 3]),
            Err(GriddedError::ValueCountMismatch {
                got: 3,
                ni: 2,
                nj: 2
            })
        ));
        // North-east node would sit at 92N.
        assert!(matches!(
            RegularLatLonGrid::new(ll(89.0, 10.0), 1.0, 1.0, 2, 4, vec![0.0; 8]),
            Err(GriddedError::Extent(_))
        ));
    }

    #[test]
    fn extent_spans_the_node_lattice() {
        let extent = grid().extent();
        assert_eq!(extent.west(), 10.0);
        assert_eq!(extent.south(), 50.0);
        assert_eq!(extent.east(), 12.0);
        assert_eq!(extent.north(), 51.0);
    }

    #[test]
    fn value_at_indexes_row_major_from_south_west() {
        let g = grid();
        assert_eq!(g.value_at(0, 0), Some(1.0));
        assert_eq!(g.value_at(2, 0), Some(3.0));
        assert_eq!(g.value_at(0, 1), Some(4.0));
        assert_eq!(g.value_at(3, 0), None);
        assert_eq!(g.value_at(0, 2), None);
    }

    #[test]
    fn sample_at_nodes_returns_node_values() {
        let g = grid();
        assert_eq!(g.sample(ll(50.0, 10.0)), Some(1.0));
        assert_eq!(g.sample(ll(50.0, 12.0)), Some(3.0)); // east edge
        assert_eq!(g.sample(ll(51.0, 12.0)), Some(6.0)); // north-east corner
    }

    #[test]
    fn sample_interpolates_bilinearly() {
        let g = grid();
        // Center of the first cell: mean of 1, 2, 4, 5.
        assert_eq!(g.sample(ll(50.5, 10.5)), Some(3.0));
        // Halfway along the southern edge of the first cell.
        assert_eq!(g.sample(ll(50.0, 10.5)), Some(1.5));
        // Quarter of the way up the western edge.
        let v = g.sample(ll(50.25, 10.0)).unwrap();
        assert!((v - 1.75).abs() < 1e-6);
    }

    #[test]
    fn sample_outside_extent_is_none() {
        let g = grid();
        assert_eq!(g.sample(ll(49.99, 10.5)), None);
        assert_eq!(g.sample(ll(50.5, 12.01)), None);
        assert_eq!(g.sample(ll(51.01, 10.5)), None);
    }

    #[test]
    fn crop_preserves_samples_and_expands_to_whole_nodes() {
        // 5×4 grid at (50N, 10E), 1° spacing, values = j·5 + i.
        let g = RegularLatLonGrid::new(
            ll(50.0, 10.0),
            1.0,
            1.0,
            5,
            4,
            (0..20).map(|v| v as f32).collect(),
        )
        .unwrap();
        let bbox = BoundingBox::new(11.4, 50.6, 12.7, 52.2).unwrap();
        let c = g.crop(bbox).unwrap();
        // floor/ceil to whole nodes: lon 11..=13, lat 50..=53.
        assert_eq!(c.origin(), ll(50.0, 11.0));
        assert_eq!((c.ni(), c.nj()), (3, 4));
        // Node values coincide with the original lattice.
        assert_eq!(c.value_at(0, 0), g.value_at(1, 0));
        assert_eq!(c.value_at(2, 3), g.value_at(3, 3));
        // Bilinear samples inside the crop bbox are identical.
        for &(lat, lon) in &[(50.6, 11.4), (51.0, 12.0), (52.2, 12.7), (51.37, 12.41)] {
            assert_eq!(
                c.sample(ll(lat, lon)),
                g.sample(ll(lat, lon)),
                "({lat}, {lon})"
            );
        }
    }

    #[test]
    fn crop_keeps_a_bilinearly_sampleable_minimum() {
        // A degenerate box on the north-east corner node still yields a
        // 2×2 grid (widened inward at the domain edge).
        let g = grid();
        let bbox = BoundingBox::new(12.0, 51.0, 12.0, 51.0).unwrap();
        let c = g.crop(bbox).unwrap();
        assert_eq!((c.ni(), c.nj()), (2, 2));
        assert_eq!(c.sample(ll(51.0, 12.0)), g.sample(ll(51.0, 12.0)));
    }

    #[test]
    fn crop_clamps_to_the_domain_and_misses_are_none() {
        let g = grid();
        assert!(
            g.crop(BoundingBox::new(0.0, 0.0, 5.0, 5.0).unwrap())
                .is_none()
        );
        // Partial overlap clamps to the shared part.
        let c = g
            .crop(BoundingBox::new(11.5, 50.5, 20.0, 60.0).unwrap())
            .unwrap();
        assert_eq!((c.ni(), c.nj()), (2, 2));
        assert_eq!(c.value_at(1, 1), g.value_at(2, 1));
    }

    #[test]
    fn sample_renormalizes_around_nan_corners() {
        // South-west corner is no-data.
        let g = RegularLatLonGrid::new(
            ll(50.0, 10.0),
            1.0,
            1.0,
            2,
            2,
            vec![f32::NAN, 2.0, 4.0, 6.0],
        )
        .unwrap();
        // Cell center: NaN corner dropped, weights renormalized over 2, 4, 6.
        assert_eq!(g.sample(ll(50.5, 10.5)), Some(4.0));
        // Exactly on the NaN node: no valid corner carries weight.
        assert_eq!(g.sample(ll(50.0, 10.0)), None);
        // On the southern edge next to the NaN corner: only the eastern
        // corner has data, so its value wins outright.
        assert_eq!(g.sample(ll(50.0, 10.25)), Some(2.0));
    }

    #[test]
    fn sample_fully_masked_cell_is_none() {
        let g = RegularLatLonGrid::new(ll(50.0, 10.0), 1.0, 1.0, 2, 2, vec![f32::NAN; 4]).unwrap();
        assert_eq!(g.sample(ll(50.5, 10.5)), None);
    }

    #[test]
    fn weather_field_units() {
        assert_eq!(WeatherField::CloudCover.unit(), "%");
        assert_eq!(WeatherField::PrecipRate.unit(), "mm/h");
        assert_eq!(WeatherField::ThunderstormPotential.unit(), "J/kg");
        assert_eq!(WeatherField::Cape.unit(), "J/kg");
        assert_eq!(WeatherField::Ceiling.unit(), "m AGL");
        assert_eq!(WeatherField::Visibility.unit(), "m");
        assert_eq!(WeatherField::WindU(PressureLevel::P850).unit(), "m/s");
        assert_eq!(WeatherField::WindV(PressureLevel::P500).unit(), "m/s");
        assert_eq!(WeatherField::Temperature(PressureLevel::P700).unit(), "°C");
        assert_eq!(WeatherField::FreezingLevel.unit(), "m AMSL");
        assert_eq!(WeatherField::ALL.len(), 22);
    }

    #[test]
    fn weather_field_pressure_level() {
        assert_eq!(
            WeatherField::WindU(PressureLevel::P950).pressure_level(),
            Some(PressureLevel::P950)
        );
        assert_eq!(
            WeatherField::Temperature(PressureLevel::P500).pressure_level(),
            Some(PressureLevel::P500)
        );
        assert_eq!(WeatherField::FreezingLevel.pressure_level(), None);
        assert_eq!(WeatherField::CloudCover.pressure_level(), None);
    }

    #[test]
    fn weather_field_display_names() {
        assert_eq!(WeatherField::CloudCover.to_string(), "cloud cover");
        assert_eq!(
            WeatherField::WindU(PressureLevel::P850).to_string(),
            "wind u 850 hPa"
        );
        assert_eq!(
            WeatherField::Temperature(PressureLevel::P500).to_string(),
            "temperature 500 hPa"
        );
        assert_eq!(WeatherField::FreezingLevel.to_string(), "freezing level");
    }

    #[test]
    fn pressure_levels_expose_hpa() {
        let hpa: Vec<u16> = PressureLevel::ALL.iter().map(PressureLevel::hpa).collect();
        assert_eq!(hpa, [950, 850, 700, 500]);
    }

    #[test]
    fn isa_altitudes_match_the_standard_atmosphere_tables() {
        // Published ISA pressure altitudes (meters), e.g. the ICAO standard
        // atmosphere tables: 950 hPa → 540 m, 850 → 1457 m, 700 → 3012 m,
        // 500 → 5574 m.
        let expect = [
            (PressureLevel::P950, 540.0),
            (PressureLevel::P850, 1457.0),
            (PressureLevel::P700, 3012.0),
            (PressureLevel::P500, 5574.0),
        ];
        for (level, meters) in expect {
            assert!(
                (level.isa_altitude().0 - meters).abs() < 5.0,
                "{level}: got {}, want ~{meters}",
                level.isa_altitude().0
            );
        }
    }

    #[test]
    fn weather_field_serde_stays_compatible() {
        // Pre-existing unit variants keep their bare-string form …
        assert_eq!(
            serde_json::to_string(&WeatherField::CloudCover).unwrap(),
            "\"CloudCover\""
        );
        assert_eq!(
            serde_json::from_str::<WeatherField>("\"PrecipRate\"").unwrap(),
            WeatherField::PrecipRate
        );
        // … and the new variants are additive (externally tagged).
        assert_eq!(
            serde_json::to_string(&WeatherField::WindU(PressureLevel::P850)).unwrap(),
            "{\"WindU\":\"P850\"}"
        );
        assert_eq!(
            serde_json::from_str::<WeatherField>("{\"Temperature\":\"P700\"}").unwrap(),
            WeatherField::Temperature(PressureLevel::P700)
        );
        assert_eq!(
            serde_json::to_string(&WeatherField::FreezingLevel).unwrap(),
            "\"FreezingLevel\""
        );
    }

    fn t(h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 10, h, m, 0).unwrap()
    }

    fn timeline() -> GriddedTimeline {
        GriddedTimeline {
            run_time: t(12, 0),
            steps: (0..=3)
                .map(|h| TimelineStep {
                    valid_time: t(12 + h, 0),
                    kind: StepKind::Forecast,
                })
                .collect(),
        }
    }

    #[test]
    fn timeline_valid_range() {
        assert_eq!(timeline().valid_range(), Some((t(12, 0), t(15, 0))));
        let empty = GriddedTimeline {
            run_time: t(12, 0),
            steps: vec![],
        };
        assert_eq!(empty.valid_range(), None);
        assert_eq!(empty.nearest_step(t(12, 0)), None);
        assert_eq!(empty.bracketing_steps(t(12, 0)), (None, None));
    }

    #[test]
    fn timeline_nearest_step() {
        let tl = timeline();
        assert_eq!(tl.nearest_step(t(12, 20)).unwrap().valid_time, t(12, 0));
        assert_eq!(tl.nearest_step(t(12, 40)).unwrap().valid_time, t(13, 0));
        // Before the first / after the last step clamps.
        assert_eq!(tl.nearest_step(t(1, 0)).unwrap().valid_time, t(12, 0));
        assert_eq!(tl.nearest_step(t(23, 0)).unwrap().valid_time, t(15, 0));
    }

    #[test]
    fn timeline_bracketing_steps() {
        let tl = timeline();
        let (before, after) = tl.bracketing_steps(t(13, 30));
        assert_eq!(before.unwrap().valid_time, t(13, 0));
        assert_eq!(after.unwrap().valid_time, t(14, 0));
        // Exactly on a step: both sides are that step.
        let (before, after) = tl.bracketing_steps(t(14, 0));
        assert_eq!(before.unwrap().valid_time, t(14, 0));
        assert_eq!(after.unwrap().valid_time, t(14, 0));
        // Outside the timeline.
        assert_eq!(tl.bracketing_steps(t(1, 0)).0, None);
        assert_eq!(tl.bracketing_steps(t(23, 0)).1, None);
    }
}
