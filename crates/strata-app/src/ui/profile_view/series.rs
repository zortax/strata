//! The profile view's input struct (plan §5.2: "the ComputedFlight's
//! profile series — pure data from strata-plan — the view does
//! layout/paint only"): everything the chart draws, flattened to plain
//! `(along-track m, altitude m AMSL)` numbers once per compute generation.
//!
//! [`ProfileSeries::build`] is the seam between the planning core and the
//! pixel pipeline: `strata_plan::profile` resolves the datum-honest values
//! (sloped AGL band edges, MSA, freezing levels), this module orders them
//! for drawing and derives the purely visual extras — conflict-emphasis
//! intervals, ETA checkpoints, the hover readout. No gpui in here; the
//! drawer's collapsed sparkline can reuse the same series.

use chrono::{DateTime, Utc};
use strata_data::domain::Airspace;
use strata_plan::FlightDoc;
use strata_plan::compute::ComputedFlight;
use strata_plan::conflict::{
    Conflict, ConflictKind, ConflictLocation, ConflictSeverity, ConflictThresholds,
};
use strata_plan::corridor::{AirspaceCrossing, Corridor};
use strata_plan::profile as plan_profile;
use strata_render::features::AirspaceStyleKey;

use crate::convert;
use crate::ui::airspace_kind_label;

use super::mapping::{leg_at, meters_to_nm};

/// One airspace crossing as a drawable band: the per-station resolved
/// vertical limits (AGL/GND edges follow the terrain — the design's sloped
/// band edges) plus the styling key and conflict emphasis.
#[derive(Debug, Clone)]
pub struct BandSeries {
    /// The crossed volume — kept whole so clicking the band can select it
    /// (the existing selection event → Inspect tab card).
    pub airspace: Airspace,
    /// Chart styling key — the same one the map uses, so band colors match
    /// the active map theme (ED-R/D/P, TMZ/RMZ distinct by convention).
    pub style: AirspaceStyleKey,
    /// Centered block label: kind/class + vertical band.
    pub label: String,
    pub entry_m: f64,
    pub exit_m: f64,
    /// Per-station band edges in along-track order; `ceiling_m == None` =
    /// unlimited (the view caps at the chart top). At least two entries
    /// (degenerate crossings are synthesized from the nearest station).
    pub stations: Vec<BandStation>,
    /// Worst penetration severity from the conflict engine, when this
    /// crossing produced an airspace conflict — emphasizes the band.
    pub conflict: Option<ConflictSeverity>,
}

/// Band edges at one station (already datum-resolved, meters AMSL).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BandStation {
    pub along_m: f64,
    pub floor_m: f64,
    pub ceiling_m: Option<f64>,
}

/// What the scrub readout shows at one along-track position.
#[derive(Debug, Clone, PartialEq)]
pub struct ScrubReadout {
    pub distance_nm: f64,
    /// Interpolated from the nav log's cumulative rows; `None` without a
    /// departure time.
    pub eta: Option<DateTime<Utc>>,
    /// Corridor worst-case terrain at the nearest station.
    pub terrain_m: Option<f64>,
    /// MSA of the containing leg.
    pub msa_m: Option<f64>,
    /// Airspace stack at this position, one label per crossed volume.
    pub stack: Vec<String>,
}

/// Everything the profile chart draws, in world units (along-track meters,
/// altitude meters AMSL). Built once per compute generation.
pub struct ProfileSeries {
    /// Route length (the X-axis extent).
    pub total_m: f64,
    /// Per-station corridor worst-case terrain (`None` = outside
    /// elevation coverage; the silhouette gaps there).
    pub terrain: Vec<(f64, Option<f64>)>,
    /// Tallest-obstacle tops: `(along_m, top_m)`.
    pub obstacles: Vec<(f64, f64)>,
    /// The planned-altitude polyline (climb / cruise plateaus / descent).
    pub planned: Vec<(f64, f64)>,
    pub toc: Option<(f64, f64)>,
    pub tod: Option<(f64, f64)>,
    /// Cumulative leg-end distances (`len == legs`).
    pub leg_ends_m: Vec<f64>,
    /// Waypoint marks incl. departure/destination: `(along_m, ident)`.
    pub waypoints: Vec<(f64, String)>,
    /// Minimum safe altitude per leg (corridor worst case + buffer).
    pub msa_m: Vec<Option<f64>>,
    /// Freezing-level estimate per leg (`None` without wind data).
    pub freezing_m: Vec<Option<f64>>,
    /// Forecast cloud-base estimate per leg, meters AMSL (design §3.3
    /// "forecast cloud-base / ceiling band"). **Data seam:** the gridded
    /// ICON `ceiling` sampling is not wired yet, so every entry is `None`
    /// today — the drawer's cloud-base toggle and the paint path are in
    /// place and start drawing the moment a producer fills this.
    pub cloud_base_m: Vec<Option<f64>>,
    /// Airspace bands, one per corridor crossing.
    pub bands: Vec<BandSeries>,
    /// Along-track intervals with terrain/obstacle clearance conflicts —
    /// the red emphasis between terrain and the planned line.
    pub emphasis: Vec<(f64, f64)>,
    /// Cumulative `(along_m, ETA)` checkpoints from the nav log.
    pub eta: Vec<(f64, DateTime<Utc>)>,
}

impl ProfileSeries {
    /// Flattens `computed` (+ the document's route, for the per-leg
    /// freezing levels) into drawable series. The MSA buffer is the
    /// conflict engine's default terrain clearance — the same 1000 ft the
    /// badges judge by.
    pub fn build(doc: &FlightDoc, computed: &ComputedFlight) -> Self {
        let corridor = &computed.corridor;
        let thresholds = ConflictThresholds::default();

        let leg_ends_m: Vec<f64> = computed
            .legs
            .iter()
            .scan(0.0, |cum, leg| {
                *cum += leg.distance.0;
                Some(*cum)
            })
            .collect();

        let total_m = computed
            .phases
            .segments
            .last()
            .map(|s| s.end_along_track.0)
            .or_else(|| leg_ends_m.last().copied())
            .unwrap_or(0.0);

        let terrain: Vec<(f64, Option<f64>)> = corridor
            .samples
            .iter()
            .map(|s| (s.station.along_track.0, s.max_terrain.map(|t| t.0)))
            .collect();

        let obstacles: Vec<(f64, f64)> = corridor
            .samples
            .iter()
            .filter_map(|s| {
                let top = s.tallest_obstacle.as_ref()?.elevation_top.0;
                Some((s.station.along_track.0, top))
            })
            .collect();

        let planned = planned_polyline(computed);
        let marker = |m: &strata_plan::perf::ProfileMarker| (m.along_track.0, m.altitude.0);
        let toc = computed.phases.toc.as_ref().map(marker);
        let tod = computed.phases.tod.as_ref().map(marker);

        let mut waypoints = Vec::with_capacity(computed.legs.len() + 1);
        if let Some(first) = computed.legs.first() {
            waypoints.push((0.0, first.from.clone()));
        }
        for (leg, end) in computed.legs.iter().zip(&leg_ends_m) {
            waypoints.push((*end, leg.to.clone()));
        }

        let msa_m = plan_profile::msa_per_leg(corridor, thresholds.terrain_clearance)
            .into_iter()
            .map(|msa| msa.map(|m| m.0))
            .collect();

        let freezing_m =
            plan_profile::freezing_levels(&doc.route, doc.cruise_altitude, &computed.winds)
                .into_iter()
                .map(|level| level.map(|m| m.0))
                .collect();

        // See the field docs: the per-leg forecast ceiling sampling is the
        // missing producer; the series carries the slot so the overlay
        // toggle and the scene's band paint are already real code paths.
        let cloud_base_m = vec![None; computed.legs.len()];

        let bands = corridor
            .crossings
            .iter()
            .map(|crossing| band_series(corridor, crossing, &computed.conflicts))
            .collect();

        let emphasis = emphasis_intervals(
            corridor,
            &computed.phases,
            &computed.conflicts,
            thresholds.terrain_clearance.0,
        );

        let eta = eta_checkpoints(computed);

        Self {
            total_m,
            terrain,
            obstacles,
            planned,
            toc,
            tod,
            leg_ends_m,
            waypoints,
            msa_m,
            freezing_m,
            cloud_base_m,
            bands,
            emphasis,
            eta,
        }
    }

    /// Hover-readout data at `along_m` (design §3.3 "hover shows distance,
    /// ETA, terrain elevation, MSA and the airspace stack").
    pub fn readout_at(&self, along_m: f64) -> ScrubReadout {
        let terrain_m = nearest_terrain(&self.terrain, along_m);
        let msa_m = leg_at(&self.leg_ends_m, along_m)
            .and_then(|leg| self.msa_m.get(leg).copied().flatten());
        let stack: Vec<String> = self
            .bands
            .iter()
            .filter(|band| band.entry_m <= along_m && along_m <= band.exit_m)
            .map(|band| band.label.clone())
            .collect();
        ScrubReadout {
            distance_nm: meters_to_nm(along_m),
            eta: eta_at(&self.eta, along_m),
            terrain_m,
            msa_m,
            stack,
        }
    }

    /// Planned altitude (m AMSL) at `along_m`, from the polyline.
    pub fn planned_at(&self, along_m: f64) -> Option<f64> {
        sample_polyline(&self.planned, along_m)
    }

    /// Terrain (m AMSL) at `along_m`, from the nearest corridor station.
    pub fn terrain_at(&self, along_m: f64) -> Option<f64> {
        nearest_terrain(&self.terrain, along_m)
    }
}

/// The planned-altitude polyline from the phase segments (contiguous by
/// construction: each segment starts where the previous ended).
fn planned_polyline(computed: &ComputedFlight) -> Vec<(f64, f64)> {
    let segments = &computed.phases.segments;
    let mut points = Vec::with_capacity(segments.len() + 1);
    if let Some(first) = segments.first() {
        points.push((first.start_along_track.0, first.start_altitude.0));
    }
    for segment in segments {
        points.push((segment.end_along_track.0, segment.end_altitude.0));
    }
    points
}

/// Linear interpolation along a polyline of ascending x; clamps beyond the
/// ends, `None` for an empty polyline.
fn sample_polyline(points: &[(f64, f64)], x: f64) -> Option<f64> {
    let first = points.first()?;
    if x <= first.0 {
        return Some(first.1);
    }
    for pair in points.windows(2) {
        let (x0, y0) = pair[0];
        let (x1, y1) = pair[1];
        if x <= x1 {
            if x1 - x0 <= f64::EPSILON {
                return Some(y1);
            }
            let t = (x - x0) / (x1 - x0);
            return Some(y0 + t * (y1 - y0));
        }
    }
    points.last().map(|&(_, y)| y)
}

/// One crossing → its drawable band. Crossings whose interval contains
/// fewer than two stations (short grazes, sub-spacing volumes) synthesize
/// flat entry/exit edges from the nearest station's resolved limits so the
/// block still has extent.
fn band_series(
    corridor: &Corridor,
    crossing: &AirspaceCrossing,
    conflicts: &[Conflict],
) -> BandSeries {
    let mut stations: Vec<BandStation> = plan_profile::crossing_bands(corridor, crossing)
        .into_iter()
        .map(|band| BandStation {
            along_m: band.along_track.0,
            floor_m: band.floor.0,
            ceiling_m: band.ceiling.map(|c| c.0),
        })
        .collect();

    if stations.len() < 2 {
        let nearest = stations.first().copied().or_else(|| {
            let mid = (crossing.entry_along_track.0 + crossing.exit_along_track.0) / 2.0;
            nearest_sample(corridor, mid)
                .map(|sample| plan_profile::station_band(crossing, sample))
                .map(|band| BandStation {
                    along_m: band.along_track.0,
                    floor_m: band.floor.0,
                    ceiling_m: band.ceiling.map(|c| c.0),
                })
        });
        stations = match nearest {
            Some(edge) => vec![
                BandStation {
                    along_m: crossing.entry_along_track.0,
                    ..edge
                },
                BandStation {
                    along_m: crossing.exit_along_track.0,
                    ..edge
                },
            ],
            // No corridor stations at all — an empty band the scene skips.
            None => Vec::new(),
        };
    }

    let airspace = &crossing.airspace;
    let label = format!(
        "{} · {} – {}",
        airspace_kind_label(&airspace.kind, airspace.class),
        airspace.lower,
        airspace.upper
    );

    BandSeries {
        airspace: airspace.clone(),
        style: convert::airspace_style_key(airspace.class, &airspace.kind),
        label,
        entry_m: crossing.entry_along_track.0,
        exit_m: crossing.exit_along_track.0,
        stations,
        conflict: crossing_conflict(crossing, conflicts),
    }
}

/// The worst airspace-conflict severity anchored inside this crossing's
/// interval *for this volume* (the engine anchors one conflict per
/// crossing at its first penetrating station; the message carries the
/// volume's name, disambiguating overlapping crossings).
fn crossing_conflict(
    crossing: &AirspaceCrossing,
    conflicts: &[Conflict],
) -> Option<ConflictSeverity> {
    conflicts
        .iter()
        .filter(|c| c.kind == ConflictKind::Airspace)
        .filter(|c| match c.location {
            ConflictLocation::Station { along_track, .. } => {
                crossing.entry_along_track.0 <= along_track.0
                    && along_track.0 <= crossing.exit_along_track.0
            }
            _ => false,
        })
        .filter(|c| c.message.contains(&crossing.airspace.name))
        .map(|c| c.severity)
        .max()
}

/// The corridor sample nearest to `along_m`.
fn nearest_sample(
    corridor: &Corridor,
    along_m: f64,
) -> Option<&strata_plan::corridor::CorridorSample> {
    corridor.samples.iter().min_by(|a, b| {
        let da = (a.station.along_track.0 - along_m).abs();
        let db = (b.station.along_track.0 - along_m).abs();
        da.total_cmp(&db)
    })
}

/// Terrain at the station nearest to `along_m`.
fn nearest_terrain(terrain: &[(f64, Option<f64>)], along_m: f64) -> Option<f64> {
    terrain
        .iter()
        .min_by(|a, b| (a.0 - along_m).abs().total_cmp(&(b.0 - along_m).abs()))
        .and_then(|&(_, elevation)| elevation)
}

/// Along-track intervals to paint with the red clearance emphasis: around
/// each terrain/obstacle conflict anchor, the contiguous run of stations
/// whose planned altitude clears the worst-case reference (terrain or
/// obstacle top) by less than `buffer_m`.
///
/// **Visual semantics, documented:** the *judgement* stays with the
/// conflict engine — only its anchors start an interval, so the climb-out
/// grace and the ramped buffer never produce false emphasis at the route
/// ends. The extent around a genuine anchor uses the full buffer (no
/// ramp), which can stretch the painted run slightly wider than the
/// engine's merged stations where it crosses a climb/descent — showing the
/// whole tight stretch is the honest picture for a drawn warning.
fn emphasis_intervals(
    corridor: &Corridor,
    phases: &strata_plan::perf::PhasePlan,
    conflicts: &[Conflict],
    buffer_m: f64,
) -> Vec<(f64, f64)> {
    let samples = &corridor.samples;
    if samples.is_empty() {
        return Vec::new();
    }

    // Violation test per station index, with the full (un-ramped) buffer.
    let violates = |index: usize| -> bool {
        let sample = &samples[index];
        let terrain = sample.max_terrain.map(|t| t.0);
        let obstacle = sample.tallest_obstacle.as_ref().map(|o| o.elevation_top.0);
        let Some(reference) = terrain.into_iter().chain(obstacle).reduce(f64::max) else {
            return false;
        };
        let Some(planned) = plan_profile::planned_altitude_at(phases, sample.station.along_track)
        else {
            return false;
        };
        planned.0 - reference < buffer_m
    };

    let anchors = conflicts.iter().filter_map(|c| match (c.kind, c.location) {
        (
            ConflictKind::Terrain | ConflictKind::Obstacle,
            ConflictLocation::Station { along_track, .. },
        ) => Some(along_track.0),
        _ => None,
    });

    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for anchor in anchors {
        // Nearest station to the anchor (the anchor *is* a station of the
        // same corridor, so this is exact up to float noise).
        let Some(center) = samples
            .iter()
            .enumerate()
            .min_by(|a, b| {
                let da = (a.1.station.along_track.0 - anchor).abs();
                let db = (b.1.station.along_track.0 - anchor).abs();
                da.total_cmp(&db)
            })
            .map(|(index, _)| index)
        else {
            continue;
        };
        if !violates(center) {
            // Stale conflict against a fresher corridor — skip rather than
            // paint emphasis the data no longer supports.
            continue;
        }
        let mut start = center;
        while start > 0 && violates(start - 1) {
            start -= 1;
        }
        let mut end = center;
        while end + 1 < samples.len() && violates(end + 1) {
            end += 1;
        }
        ranges.push((start, end));
    }

    // Merge overlapping/adjacent index ranges, then convert to meters.
    ranges.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (start, end) in ranges {
        match merged.last_mut() {
            Some((_, last_end)) if start <= *last_end + 1 => *last_end = (*last_end).max(end),
            _ => merged.push((start, end)),
        }
    }
    merged
        .into_iter()
        .map(|(start, end)| {
            (
                samples[start].station.along_track.0,
                samples[end].station.along_track.0,
            )
        })
        .collect()
}

/// Cumulative `(along_m, ETA)` checkpoints from the nav-log rows (rows are
/// in along-track order and include the TOC/TOD splits, so interpolation
/// between checkpoints respects the per-phase ground speeds).
fn eta_checkpoints(computed: &ComputedFlight) -> Vec<(f64, DateTime<Utc>)> {
    let mut checkpoints = Vec::new();
    let mut cum_m = 0.0;
    for row in &computed.navlog.rows {
        if let Some(distance) = row.distance {
            cum_m += distance.as_meters().0;
        }
        if let Some(eta) = row.eta {
            checkpoints.push((cum_m, eta));
        }
    }
    checkpoints
}

/// ETA at `along_m`, linearly interpolated between checkpoints (clamped to
/// the ends; `None` without any checkpoint).
fn eta_at(checkpoints: &[(f64, DateTime<Utc>)], along_m: f64) -> Option<DateTime<Utc>> {
    let first = checkpoints.first()?;
    if along_m <= first.0 {
        return Some(first.1);
    }
    for pair in checkpoints.windows(2) {
        let (x0, t0) = pair[0];
        let (x1, t1) = pair[1];
        if along_m <= x1 {
            if x1 - x0 <= f64::EPSILON {
                return Some(t1);
            }
            let f = (along_m - x0) / (x1 - x0);
            let span = (t1 - t0).num_seconds() as f64;
            return Some(t0 + chrono::Duration::seconds((span * f).round() as i64));
        }
    }
    checkpoints.last().map(|&(_, t)| t)
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone as _;
    use strata_data::domain::{
        AirspaceClass, AirspaceKind, LatLon, Meters, MetersAgl, MetersAmsl, Obstacle, ObstacleKind,
        Polygon, VerticalLimit,
    };
    use strata_plan::corridor::{CorridorParams, CorridorSample, Station};
    use strata_plan::perf::{PhaseKind, PhasePlan, PhaseSegment};
    use strata_plan::units::{Knots, Liters, Minutes};

    use super::*;

    // ── builders ─────────────────────────────────────────────────────────

    fn sample(index: usize, along: f64, terrain: Option<f64>) -> CorridorSample {
        CorridorSample {
            station: Station {
                index,
                leg_index: 0,
                along_track: Meters(along),
                position: LatLon::new(50.0, 8.0 + index as f64 * 0.01).unwrap(),
            },
            max_terrain: terrain.map(MetersAmsl),
            min_terrain: terrain.map(MetersAmsl),
            tallest_obstacle: None,
        }
    }

    fn with_obstacle(mut sample: CorridorSample, top: f64) -> CorridorSample {
        sample.tallest_obstacle = Some(Obstacle {
            name: Some("Mast".into()),
            kind: ObstacleKind::Mast,
            position: sample.station.position,
            height: MetersAgl(50.0),
            elevation_top: MetersAmsl(top),
            lighted: true,
        });
        sample
    }

    fn corridor(samples: Vec<CorridorSample>) -> Corridor {
        Corridor {
            params: CorridorParams::default(),
            samples,
            crossings: Vec::new(),
        }
    }

    fn airspace(name: &str, lower: VerticalLimit, upper: VerticalLimit) -> Airspace {
        Airspace {
            name: name.to_owned(),
            class: AirspaceClass::D,
            kind: AirspaceKind::Ctr,
            lower,
            upper,
            geometry: Polygon::new(
                vec![
                    LatLon::new(49.9, 7.9).unwrap(),
                    LatLon::new(50.1, 8.1).unwrap(),
                    LatLon::new(49.9, 8.3).unwrap(),
                ],
                vec![],
            )
            .unwrap(),
            airac: None,
        }
    }

    /// A level-cruise phase plan at `alt_m` over `total_m`.
    fn cruise_plan(alt_m: f64, total_m: f64) -> PhasePlan {
        PhasePlan {
            segments: vec![PhaseSegment {
                kind: PhaseKind::Cruise,
                start_along_track: Meters(0.0),
                end_along_track: Meters(total_m),
                start_altitude: MetersAmsl(alt_m),
                end_altitude: MetersAmsl(alt_m),
                tas: Knots(100.0),
                duration: Minutes(10.0),
                fuel: Liters(5.0),
            }],
            toc: None,
            tod: None,
            total_duration: Minutes(10.0),
            total_fuel: Liters(5.0),
        }
    }

    fn terrain_conflict_at(along: f64) -> Conflict {
        Conflict {
            kind: ConflictKind::Terrain,
            severity: ConflictSeverity::Warning,
            location: ConflictLocation::Station {
                along_track: Meters(along),
                position: LatLon::new(50.0, 8.0).unwrap(),
            },
            message: "terrain".into(),
        }
    }

    // ── band geometry over sloping terrain (the sloped-floor case) ───────

    #[test]
    fn band_floors_slope_with_the_terrain() {
        // Terrain climbing 100 m per station; an AGL floor must climb with
        // it while the AMSL ceiling stays flat.
        let mut corridor = corridor(
            (0..6)
                .map(|i| sample(i, i as f64 * 1000.0, Some(200.0 + 100.0 * i as f64)))
                .collect(),
        );
        let crossing = AirspaceCrossing {
            airspace: airspace(
                "EDGE CTR",
                VerticalLimit::agl(MetersAgl(300.0)),
                VerticalLimit::amsl(MetersAmsl(2500.0)),
            ),
            entry_along_track: Meters(1000.0),
            exit_along_track: Meters(4000.0),
        };
        corridor.crossings.push(crossing.clone());

        let band = band_series(&corridor, &crossing, &[]);
        assert_eq!(band.stations.len(), 4, "stations 1..=4 inclusive");
        let floors: Vec<f64> = band.stations.iter().map(|s| s.floor_m).collect();
        assert_eq!(floors, vec![600.0, 700.0, 800.0, 900.0], "sloped AGL floor");
        assert!(
            band.stations.iter().all(|s| s.ceiling_m == Some(2500.0)),
            "flat AMSL ceiling"
        );
        assert_eq!(band.style, AirspaceStyleKey::Ctr);
        assert!(band.label.starts_with("CTR D"), "{}", band.label);
        assert_eq!(band.conflict, None);
    }

    #[test]
    fn short_grazes_synthesize_a_flat_two_station_band() {
        // A crossing between stations (no station inside the interval).
        let corridor_data = corridor(vec![
            sample(0, 0.0, Some(100.0)),
            sample(1, 1000.0, Some(100.0)),
        ]);
        let crossing = AirspaceCrossing {
            airspace: airspace(
                "TINY",
                VerticalLimit::amsl(MetersAmsl(500.0)),
                VerticalLimit::unl(),
            ),
            entry_along_track: Meters(400.0),
            exit_along_track: Meters(600.0),
        };
        let band = band_series(&corridor_data, &crossing, &[]);
        assert_eq!(band.stations.len(), 2);
        assert_eq!(band.stations[0].along_m, 400.0);
        assert_eq!(band.stations[1].along_m, 600.0);
        assert_eq!(band.stations[0].floor_m, 500.0);
        assert_eq!(band.stations[0].ceiling_m, None, "UNL caps at chart top");
    }

    #[test]
    fn band_conflicts_match_by_interval_and_name() {
        let corridor_data = corridor(
            (0..5)
                .map(|i| sample(i, i as f64 * 1000.0, Some(100.0)))
                .collect(),
        );
        let crossing = AirspaceCrossing {
            airspace: airspace(
                "EDGGN CTR",
                VerticalLimit::gnd(),
                VerticalLimit::amsl(MetersAmsl(1500.0)),
            ),
            entry_along_track: Meters(1000.0),
            exit_along_track: Meters(3000.0),
        };
        let conflicts = [Conflict {
            kind: ConflictKind::Airspace,
            severity: ConflictSeverity::Caution,
            location: ConflictLocation::Station {
                along_track: Meters(2000.0),
                position: LatLon::new(50.0, 8.0).unwrap(),
            },
            message: "enters EDGGN CTR (CTR D) at 1.1 NM at 1200 ft — floor GND".into(),
        }];
        let band = band_series(&corridor_data, &crossing, &conflicts);
        assert_eq!(band.conflict, Some(ConflictSeverity::Caution));

        // A different volume's conflict in the same interval must not
        // emphasize this band.
        let other = [Conflict {
            message: "enters OTHER TMA at 1.1 NM".into(),
            ..conflicts[0].clone()
        }];
        assert_eq!(
            band_series(&corridor_data, &crossing, &other).conflict,
            None
        );

        // A conflict outside the interval is someone else's.
        let outside = [Conflict {
            location: ConflictLocation::Station {
                along_track: Meters(3500.0),
                position: LatLon::new(50.0, 8.0).unwrap(),
            },
            ..conflicts[0].clone()
        }];
        assert_eq!(
            band_series(&corridor_data, &crossing, &outside).conflict,
            None
        );
    }

    // ── emphasis intervals ────────────────────────────────────────────────

    #[test]
    fn emphasis_expands_around_the_anchor_and_merges() {
        // 11 stations every 1 km, cruise at 800 m, buffer 304.8 m (1000 ft).
        // A 600 m ridge over stations 3..=7 leaves 200 m clearance there
        // (violating); the 100 m terrain elsewhere clears by 700 m.
        let samples: Vec<CorridorSample> = (0..11)
            .map(|i| {
                let terrain = match i {
                    3..=7 => 600.0,
                    _ => 100.0,
                };
                sample(i, i as f64 * 1000.0, Some(terrain))
            })
            .collect();
        let corridor_data = corridor(samples);
        let phases = cruise_plan(800.0, 10_000.0);
        let buffer = 304.8;

        // Anchor at the worst station (5 km).
        let intervals = emphasis_intervals(
            &corridor_data,
            &phases,
            &[terrain_conflict_at(5000.0)],
            buffer,
        );
        assert_eq!(intervals, vec![(3000.0, 7000.0)], "expanded to the run");

        // Two anchors in the same run merge into one interval.
        let intervals = emphasis_intervals(
            &corridor_data,
            &phases,
            &[terrain_conflict_at(4000.0), terrain_conflict_at(6000.0)],
            buffer,
        );
        assert_eq!(intervals, vec![(3000.0, 7000.0)]);

        // No anchors → no emphasis, even though stations violate: the
        // judgement stays with the conflict engine (climb-out grace).
        assert!(emphasis_intervals(&corridor_data, &phases, &[], buffer).is_empty());
    }

    #[test]
    fn emphasis_uses_obstacle_tops_and_skips_stale_anchors() {
        let samples = vec![
            sample(0, 0.0, Some(100.0)),
            with_obstacle(sample(1, 1000.0, Some(100.0)), 700.0),
            sample(2, 2000.0, Some(100.0)),
        ];
        let corridor_data = corridor(samples);
        let phases = cruise_plan(800.0, 2000.0);

        // Obstacle top 700 m, planned 800 m → clearance 100 m < buffer.
        let conflict = Conflict {
            kind: ConflictKind::Obstacle,
            ..terrain_conflict_at(1000.0)
        };
        let intervals = emphasis_intervals(&corridor_data, &phases, &[conflict], 304.8);
        assert_eq!(intervals, vec![(1000.0, 1000.0)], "single-station run");

        // A stale anchor pointing at clear terrain paints nothing.
        let intervals = emphasis_intervals(
            &corridor_data,
            &phases,
            &[terrain_conflict_at(2000.0)],
            304.8,
        );
        assert!(intervals.is_empty());
    }

    // ── scrub readout lookup ─────────────────────────────────────────────

    fn minimal_series() -> ProfileSeries {
        let t0 = Utc.with_ymd_and_hms(2026, 6, 14, 10, 0, 0).unwrap();
        let t1 = Utc.with_ymd_and_hms(2026, 6, 14, 10, 30, 0).unwrap();
        ProfileSeries {
            total_m: 20_000.0,
            terrain: vec![
                (0.0, Some(100.0)),
                (10_000.0, Some(400.0)),
                (20_000.0, None),
            ],
            obstacles: Vec::new(),
            planned: vec![(0.0, 300.0), (5_000.0, 900.0), (20_000.0, 900.0)],
            toc: Some((5_000.0, 900.0)),
            tod: None,
            leg_ends_m: vec![8_000.0, 20_000.0],
            waypoints: vec![
                (0.0, "EDFE".into()),
                (8_000.0, "WP1".into()),
                (20_000.0, "EDQN".into()),
            ],
            msa_m: vec![Some(700.0), None],
            freezing_m: vec![None, Some(2500.0)],
            cloud_base_m: vec![None, None],
            bands: vec![BandSeries {
                airspace: airspace(
                    "BAND",
                    VerticalLimit::gnd(),
                    VerticalLimit::amsl(MetersAmsl(1500.0)),
                ),
                style: AirspaceStyleKey::Ctr,
                label: "CTR D · GND – 1500 m".into(),
                entry_m: 6_000.0,
                exit_m: 12_000.0,
                stations: vec![
                    BandStation {
                        along_m: 6_000.0,
                        floor_m: 0.0,
                        ceiling_m: Some(1500.0),
                    },
                    BandStation {
                        along_m: 12_000.0,
                        floor_m: 0.0,
                        ceiling_m: Some(1500.0),
                    },
                ],
                conflict: None,
            }],
            emphasis: Vec::new(),
            eta: vec![(0.0, t0), (20_000.0, t1)],
        }
    }

    #[test]
    fn readout_finds_station_leg_and_stack() {
        let series = minimal_series();

        // 7 km: nearest station 10 km (terrain 400), leg 0 (MSA 700),
        // inside the band.
        let readout = series.readout_at(7_000.0);
        assert!((readout.distance_nm - 7000.0 / 1852.0).abs() < 1e-9);
        assert_eq!(readout.terrain_m, Some(400.0));
        assert_eq!(readout.msa_m, Some(700.0));
        assert_eq!(readout.stack, vec!["CTR D · GND – 1500 m".to_string()]);
        // ETA: 7/20 of the 30 min → 10:10:30.
        let eta = readout.eta.expect("checkpoints exist");
        assert_eq!(eta, Utc.with_ymd_and_hms(2026, 6, 14, 10, 10, 30).unwrap());

        // 16 km: nearest station 20 km has no terrain data; leg 1 has no
        // MSA; outside the band.
        let readout = series.readout_at(16_000.0);
        assert_eq!(readout.terrain_m, None);
        assert_eq!(readout.msa_m, None);
        assert!(readout.stack.is_empty());

        // Band edges are inclusive.
        assert_eq!(series.readout_at(6_000.0).stack.len(), 1);
        assert_eq!(series.readout_at(12_000.0).stack.len(), 1);
    }

    #[test]
    fn planned_polyline_samples_and_clamps() {
        let series = minimal_series();
        assert_eq!(series.planned_at(0.0), Some(300.0));
        assert_eq!(series.planned_at(2_500.0), Some(600.0), "mid-climb");
        assert_eq!(series.planned_at(10_000.0), Some(900.0));
        assert_eq!(series.planned_at(99_000.0), Some(900.0), "clamps past end");
        assert_eq!(sample_polyline(&[], 5.0), None);
    }

    #[test]
    fn eta_interpolation_clamps_to_the_ends() {
        let series = minimal_series();
        let t0 = Utc.with_ymd_and_hms(2026, 6, 14, 10, 0, 0).unwrap();
        let t1 = Utc.with_ymd_and_hms(2026, 6, 14, 10, 30, 0).unwrap();
        assert_eq!(eta_at(&series.eta, -5.0), Some(t0));
        assert_eq!(eta_at(&series.eta, 25_000.0), Some(t1));
        assert_eq!(eta_at(&[], 5.0), None);
    }
}
