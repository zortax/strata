//! Per-station terrain statistics across the corridor width.

use strata_data::domain::{LatLon, MetersAmsl};

use crate::sources::{ElevationSource, SourceError};

/// Lowest and highest max-pooled terrain across one station's lateral
/// samples, as `(min, max)`.
///
/// `Ok(None)` only when **every** lateral sample is outside elevation
/// coverage; with partial coverage the extrema over the covered samples are
/// returned (conservative in the safe direction is impossible here — data
/// that does not exist cannot raise the profile — so partial coverage is a
/// data-completeness question, surfaced by the conflict engine, not hidden
/// by this function).
///
/// Both statistics are taken over *max-pooled* cell values, so the minimum
/// can still overestimate the true terrain within one pooling cell — fine
/// for its consumer (AGL floor normalization), which only needs the lowest
/// *plausible* corridor terrain, not a survey-grade minimum.
pub(super) fn terrain_extrema(
    samples: &[LatLon],
    elevation: &dyn ElevationSource,
) -> Result<Option<(MetersAmsl, MetersAmsl)>, SourceError> {
    let mut extrema: Option<(f64, f64)> = None;
    for &sample in samples {
        if let Some(MetersAmsl(value)) = elevation.max_elevation_at(sample)? {
            extrema = Some(match extrema {
                Some((min, max)) => (min.min(value), max.max(value)),
                None => (value, value),
            });
        }
    }
    Ok(extrema.map(|(min, max)| (MetersAmsl(min), MetersAmsl(max))))
}
