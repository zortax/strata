//! Render-side feature vocabulary.
//!
//! Plain value types the application converts its domain model into
//! (`strata-data` and `strata-render` know nothing about each other — these
//! types are the contract). Coordinates are WGS84 `[lon, lat]` degree pairs;
//! layers convert to world space when tessellating.

use crate::geo::LatLon;
use crate::layer::LayerId;

/// ICAO airspace class A–G.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IcaoClass {
    A,
    B,
    C,
    D,
    E,
    F,
    G,
}

/// Style selector for an airspace polygon — mirrors the chart-relevant
/// class/kind combinations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AirspaceStyleKey {
    IcaoClass(IcaoClass),
    Ctr,
    Rmz,
    Tmz,
    Danger,
    Restricted,
    Prohibited,
    GliderSector,
    ParaJump,
    Other,
}

/// METAR flight category color (green / blue / red / magenta on charts).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FlightCategoryColor {
    Vfr,
    Mvfr,
    Ifr,
    Lifr,
}

/// Symbol selector for point features.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PointKind {
    AirportIntl,
    AirportRegional,
    Airfield,
    GliderSite,
    Heliport,
    UltraLight,
    Vor,
    VorDme,
    Dme,
    Ndb,
    Tacan,
    ReportingPointMandatory,
    ReportingPointVoluntary,
    Obstacle,
    WeatherStation(FlightCategoryColor),
}

impl PointKind {
    /// The toggleable layer this point kind belongs to.
    pub fn layer(self) -> LayerId {
        match self {
            Self::AirportIntl
            | Self::AirportRegional
            | Self::Airfield
            | Self::GliderSite
            | Self::Heliport
            | Self::UltraLight => LayerId::Airports,
            Self::Vor | Self::VorDme | Self::Dme | Self::Ndb | Self::Tacan => LayerId::Navaids,
            Self::ReportingPointMandatory | Self::ReportingPointVoluntary => {
                LayerId::ReportingPoints
            }
            Self::Obstacle => LayerId::Obstacles,
            Self::WeatherStation(_) => LayerId::Weather,
        }
    }
}

/// An airspace polygon with chart styling and vertical-band labels.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderAirspace {
    /// App-side feature id, echoed back for hit-test correlation.
    pub id: u64,
    pub style: AirspaceStyleKey,
    /// Exterior ring, `[lon, lat]` degrees. Closing vertex optional.
    pub polygon: Vec<[f64; 2]>,
    /// Interior rings (holes), same convention.
    pub holes: Vec<Vec<[f64; 2]>>,
    /// Lower vertical limit, chart style (e.g. `"2500 MSL"`, `"GND"`).
    pub lower_label: String,
    /// Upper vertical limit, chart style (e.g. `"FL 100"`).
    pub upper_label: String,
    pub name: String,
}

/// A point feature (airport, navaid, reporting point, obstacle, METAR
/// station) drawn as a screen-space symbol with an optional text label.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderPointFeature {
    /// App-side feature id, echoed back for hit-test correlation.
    pub id: u64,
    pub kind: PointKind,
    pub position: LatLon,
    /// Label next to the symbol (ident or name), zoom-gated by the layer.
    pub label: Option<String>,
    /// Screen rotation of the symbol mesh in degrees clockwise from north
    /// (true heading; map north is always up). `None` = un-rotated. Used for
    /// airport runway ticks, whose canonical mesh orientation is north-south.
    pub rotation_deg: Option<f32>,
}

/// Role of one route point — selects the on-map handle shape (departure
/// square, waypoint circle, destination square-flag, alternate hollow) and
/// whether the point is part of the flown track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RoutePointKind {
    Departure,
    Waypoint,
    Destination,
    Alternate,
}

impl RoutePointKind {
    /// Alternates hang off the destination; everything else is flown in
    /// `points` order (the "main track").
    pub fn is_alternate(self) -> bool {
        matches!(self, Self::Alternate)
    }
}

/// One point of a [`RenderRoute`].
#[derive(Debug, Clone, PartialEq)]
pub struct RouteVertex {
    /// App-side waypoint id, echoed back so the app can hit-test handles in
    /// screen space itself (it has the camera snapshot). The renderer never
    /// interprets it.
    pub id: u64,
    /// `[lon, lat]` degrees (the ring/polygon convention).
    pub pos: [f64; 2],
    pub kind: RoutePointKind,
}

/// The planned flight route as drawn by the route layer
/// ([`crate::renderer::MapRenderer::set_route`]).
///
/// The **main track** is every non-[`Alternate`](RoutePointKind::Alternate)
/// point in `points` order; consecutive main-track points form the *legs*.
/// Alternate points each get a dashed link from the last main-track point
/// (the destination) plus their own hollow handle. All along-route
/// distances (`toc`, `tod`, `scrub_along_m`) are geodesic meters measured
/// from the departure along the main track.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RenderRoute {
    /// Route points in order: departure, waypoints, destination, then any
    /// alternates.
    pub points: Vec<RouteVertex>,
    /// Per-leg conflict flag (length = main-track legs); `true` tints the
    /// leg with the theme's conflict color. A short vector treats missing
    /// legs as conflict-free.
    pub leg_conflict: Vec<bool>,
    /// Top of climb as `(meters along the main track, [lon, lat] degrees)`.
    /// The marker is placed by interpolating the along-track distance (so it
    /// sits exactly on the drawn polyline); the position is the contract for
    /// app-side hit-testing and the fallback for degenerate tracks.
    pub toc: Option<(f64, [f64; 2])>,
    /// Top of descent; same convention as [`toc`](Self::toc).
    pub tod: Option<(f64, [f64; 2])>,
    /// Per-leg text labels (parallel to the main-track legs like
    /// [`leg_conflict`](Self::leg_conflict); `None` entries and missing
    /// trailing legs draw no label). Rendered near the leg midpoint, offset
    /// off the line, zoom-gated and decluttered by the text system —
    /// e.g. `"MH 053 · 135 kt · 4500"`.
    pub leg_labels: Vec<Option<String>>,
    /// Snap-indication ring: `[lon, lat]` degrees of the feature a waypoint
    /// drag currently snaps onto. Drawn as a small pulsing ring; `None`
    /// (the resting state) draws nothing and demands no redraws.
    pub snap_ring: Option<[f64; 2]>,
    /// Hover-emphasized route point (the flight panel's row hover): the
    /// [`RouteVertex::id`] whose handle draws enlarged with a static
    /// accent glow ring (distinct from the pulsing snap ring). Rides the
    /// instance/uniform fast path — never a re-tessellation. `None` (or an
    /// id the route does not carry) draws no emphasis.
    pub highlight: Option<u64>,
    /// Half-width of the terrain/obstacle corridor in ground meters. When
    /// set, the corridor is drawn as a translucent ground-fixed stroke
    /// around the main track (its on-screen width follows the zoom).
    pub corridor_halfwidth_m: Option<f64>,
    /// Profile-drawer scrub cursor in meters along the main track; drawn as
    /// an emphasized marker interpolated onto the polyline.
    pub scrub_along_m: Option<f64>,
}

/// A SIGMET area drawn as a hatched overlay polygon.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderSigmet {
    /// Outline, `[lon, lat]` degrees. Closing vertex optional.
    pub polygon: Vec<[f64; 2]>,
    /// Short hazard text (e.g. `"SEV TURB"`).
    pub hazard_label: String,
}

/// Scalar weather field rendered as a gridded, colormapped overlay
/// (one toggleable [`LayerId`] each).
///
/// Units are part of the contract — the colormaps in
/// [`crate::map_theme::WeatherTheme`] are authored against them:
///
/// * [`CloudCover`](Self::CloudCover): total cloud cover in **percent**
///   (0–100).
/// * [`PrecipRate`](Self::PrecipRate): precipitation rate in **mm/h**.
/// * [`ThunderstormPotential`](Self::ThunderstormPotential): lightning
///   potential index in **J/kg** (ICON-D2 `lpi` scale; ~1 = threshold,
///   ~15 = severe).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GriddedField {
    CloudCover,
    PrecipRate,
    ThunderstormPotential,
}

impl GriddedField {
    pub const COUNT: usize = 3;

    /// All fields, in the draw order of their layers (bottom first).
    pub const ALL: [GriddedField; Self::COUNT] = [
        GriddedField::CloudCover,
        GriddedField::PrecipRate,
        GriddedField::ThunderstormPotential,
    ];

    /// Stable dense index.
    pub fn index(self) -> usize {
        match self {
            Self::CloudCover => 0,
            Self::PrecipRate => 1,
            Self::ThunderstormPotential => 2,
        }
    }

    /// The toggleable layer this field is drawn under.
    pub fn layer(self) -> LayerId {
        match self {
            Self::CloudCover => LayerId::CloudCover,
            Self::PrecipRate => LayerId::Precipitation,
            Self::ThunderstormPotential => LayerId::Thunderstorms,
        }
    }
}

/// One time step of a gridded scalar weather field on a **regular lat-lon
/// grid** (the renderer never sees source projections — providers reproject
/// radar composites etc. before handing frames over).
///
/// Memory note: the renderer keeps the working set as-is on the CPU and
/// uploads only the frames bracketing the current weather time to the GPU,
/// so the app controls the memory/temporal-resolution trade-off via the
/// size of the set it pushes.
#[derive(Debug, Clone, PartialEq)]
pub struct WeatherGridFrame {
    pub field: GriddedField,
    /// Valid time of this frame, unix seconds (UTC).
    pub valid_time: i64,
    /// Grid extent as `(min_lat, min_lon, max_lat, max_lon)` degrees —
    /// the coordinates of the **corner grid points** (cell centers), not
    /// outer cell edges.
    pub extent: (f64, f64, f64, f64),
    /// Number of grid points per row (west → east). Must be ≥ 2.
    pub ni: u32,
    /// Number of rows (south → north). Must be ≥ 2.
    pub nj: u32,
    /// Row-major values, **row 0 = southernmost row**, each row scanning
    /// west → east (`values[j * ni + i]`, GRIB scan mode `0x40`). `NaN` =
    /// no data (rendered fully transparent). Length must be `ni * nj`.
    pub values: Vec<f32>,
}
