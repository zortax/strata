//! Domain model — AIXM-leaning vocabulary. WGS84 degrees + meters
//! internally; unit/datum conversions happen only at the edges.

pub mod airac;
pub mod airport;
pub mod airspace;
pub mod country;
pub mod geo;
pub mod gridded;
pub mod navaid;
pub mod notam;
pub mod obstacle;
pub mod point;
pub mod vertical;
pub mod weather;

pub use airac::AiracCycle;
pub use airport::{
    Airport, AirportKind, Frequency, FrequencyKind, IcaoCode, IcaoCodeError, RadioFrequency,
    Runway, RunwaySurface,
};
pub use airspace::{Airspace, AirspaceClass, AirspaceKind};
pub use country::{Country, UnknownCountry, weather_bboxes};
pub use geo::{
    BoundingBox, Degrees, GeoError, LatLon, Meters, Polygon, Polyline, PreparedPolygon, magvar,
};
pub use gridded::{
    GriddedError, GriddedTimeline, PressureLevel, RegularLatLonGrid, StepKind, TimelineStep,
    WeatherField, WeatherGrid,
};
pub use navaid::{Navaid, NavaidKind};
pub use notam::{
    Notam, NotamEnd, NotamId, NotamItems, NotamKind, NotamParseError, NotamValidity, QCode,
    QCondition, QLine, QSubject,
};
pub use obstacle::{Obstacle, ObstacleKind};
pub use point::ReportingPoint;
pub use vertical::{FEET_PER_METER, MetersAgl, MetersAmsl, VerticalLimit, VerticalReference};
pub use weather::{
    CloudAmount, CloudKind, CloudLayer, FlightCategory, Metar, MetarDecode, Qnh, Sigmet,
    SigmetHazard, Taf, TafChange, TafChangeKind, TafGroup, Trend, Visibility, Wind, WindDirection,
    WxDescriptor, WxIntensity, WxKind, WxPhenomenon,
};
