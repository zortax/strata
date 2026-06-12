//! Route geometry and editing operations (plan §3 `route/`).
//!
//! Geometry is geodesic **on the sphere** (see `geo.rs` for the documented
//! ellipsoid-error bound); editing operations keep the leg-scoped fields on
//! [`RouteWaypoint`](crate::flight::RouteWaypoint) attached to the correct
//! geometric leg.

mod geo;
mod legs;
mod ops;
#[cfg(test)]
mod tests;

pub use geo::{
    EARTH_RADIUS_METERS, great_circle_distance, initial_true_track, intermediate_point, midpoint,
};
pub use legs::{Leg, LegGeometry, leg_geometry, legs, total_distance};
pub use ops::{RouteError, insert, normalize, reverse};
