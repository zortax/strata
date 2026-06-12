//! Stage 1 of the profile paint pipeline: the **world-space scene** —
//! everything the chart draws, resolved into resolution-independent
//! along-track-meters × altitude-meters geometry, plus the style params it
//! was resolved under (the cache key). Cached per compute generation +
//! params; a drawer drag-resize or window resize never touches it — stage
//! 2 ([`super::layout`]) remaps this geometry to pixels every frame.
//!
//! No `Window` in here: label *contents* are stored as strings, shaping
//! happens downstream against the shaped-text cache.

use gpui::{Hsla, PathBuilder, SharedString, point, px};
use strata_plan::conflict::ConflictSeverity;

use super::series::ProfileSeries;

/// The virtual side length of the normalized chart space the cached fill
/// meshes are tessellated in (comfortably above lyon's working precision;
/// the per-frame transform divides it back out).
pub(crate) const NORM_SCALE: f32 = 1024.0;

/// Simplification tolerance in normalized units — ≈0.3 px on a 3200 px
/// wide chart, conservatively invisible at any realistic size.
const SIMPLIFY_TOLERANCE_NORM: f64 = 1e-4;

/// A fill mesh in normalized chart space: flat triangle vertices
/// (`len % 3 == 0`), x/y in `0..=NORM_SCALE`, y up. Tessellated once per
/// generation; the per-frame layout only affine-maps the vertices.
pub(crate) type NormMesh = Vec<(f32, f32)>;

/// Everything the scene needs from the UI/map themes, resolved by the
/// view's render pass. `PartialEq` makes it part of the cache key, so a
/// theme switch rebuilds the world scene.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Palette {
    pub axis_text: Hsla,
    pub grid: Hsla,
    pub terrain_fill: Hsla,
    pub terrain_stroke: Hsla,
    pub obstacle: Hsla,
    /// The map theme's route line — the profile's planned line matches the
    /// map's "my route" color.
    pub planned: Hsla,
    pub marker_fill: Hsla,
    pub msa: Hsla,
    pub freezing: Hsla,
    pub cloud_base: Hsla,
    pub danger: Hsla,
    pub warning: Hsla,
}

/// Per-band styling resolved from the active map theme (same colors and
/// stroke grammar as the map's airspace layer).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BandStyle {
    pub fill: Hsla,
    pub border: Hsla,
    pub border_width: f32,
    pub dash: Option<(f32, f32)>,
    pub label: Hsla,
}

/// The cache key + build input of one world scene.
pub(crate) struct SceneParams {
    /// Monotonic compute generation of the input series.
    pub generation: u64,
    pub palette: Palette,
    pub band_styles: Vec<BandStyle>,
    pub show_freezing: bool,
    pub show_cloud_base: bool,
}

impl SceneParams {
    pub fn matches(&self, other: &Self) -> bool {
        self.generation == other.generation
            && self.palette == other.palette
            && self.band_styles == other.band_styles
            && self.show_freezing == other.show_freezing
            && self.show_cloud_base == other.show_cloud_base
    }
}

/// One airspace band, world-space: the closed outline plus the label
/// anchors. Pixel-dependent decisions (thinness culling, label fit) stay
/// with the per-frame layout.
pub(crate) struct WorldBand {
    /// Index into the series' bands / [`SceneParams::band_styles`].
    pub series_index: usize,
    /// Closed outline: ceiling left→right, then floor right→left
    /// (altitudes clamped; UNL ceilings cap at the chart ceiling; both
    /// edges simplified — flat AMSL edges collapse to their endpoints).
    pub polygon: Vec<(f64, f64)>,
    /// The outline's cached fill mesh.
    pub mesh: NormMesh,
    pub label: SharedString,
    /// Block center along-track (the label/badge x anchor).
    pub center_along_m: f64,
    /// Band middle altitude at the center station (label y anchor).
    pub center_alt_m: f64,
    /// Ceiling altitude at the center station (badge y anchor).
    pub center_top_m: f64,
    /// Block extent (first/last station).
    pub left_m: f64,
    pub right_m: f64,
    /// Mean vertical thickness in meters (px gates are per frame).
    pub thickness_m: f64,
    pub conflict: Option<ConflictSeverity>,
}

/// The cached resolution-independent scene. All altitudes are already
/// clamped into `floor_m..ceil_m`, so the px mapping never overdraws.
pub(crate) struct WorldScene {
    params: SceneParams,
    /// X extent (route length, m).
    pub total_m: f64,
    /// Chart altitude range, meters AMSL ([`altitude_range`]).
    pub floor_m: f64,
    pub ceil_m: f64,
    /// Contiguous terrain runs (split where elevation coverage gaps),
    /// each at least two stations, simplified to sub-pixel tolerance.
    pub terrain_runs: Vec<Vec<(f64, f64)>>,
    /// Fill meshes of the terrain runs (closed down to the chart floor),
    /// parallel to `terrain_runs`.
    pub terrain_meshes: Vec<NormMesh>,
    /// Obstacle markers: `(along, top)`.
    pub obstacles: Vec<(f64, f64)>,
    pub bands: Vec<WorldBand>,
    /// Per-leg reference segments: `(start, end, altitude)`. The weather
    /// lines are empty when their drawer toggle is off (the toggle is part
    /// of the cache key).
    pub msa: Vec<(f64, f64, f64)>,
    pub freezing: Vec<(f64, f64, f64)>,
    pub cloud_base: Vec<(f64, f64, f64)>,
    /// Clearance-emphasis fill meshes (terrain↔planned area).
    pub emphasis: Vec<NormMesh>,
    /// The planned-altitude polyline.
    pub planned: Vec<(f64, f64)>,
    /// TOC/TOD diamond centers.
    pub markers: Vec<(f64, f64)>,
    /// Waypoint ticks: `(along, ident)`.
    pub waypoints: Vec<(f64, SharedString)>,
}

impl WorldScene {
    /// Whether this cached scene was built from the same inputs.
    pub fn params_match(&self, params: &SceneParams) -> bool {
        self.params.matches(params)
    }

    pub fn params(&self) -> &SceneParams {
        &self.params
    }
}

/// Builds the world scene for `series` under `params`. Runs only when the
/// compute generation or the style params change — never on resize. The
/// dense series geometry is simplified to sub-pixel tolerance and the fill
/// areas are tessellated here, once — both are resolution-independent, so
/// the per-frame remap only maps vertices.
pub(crate) fn build_world_scene(series: &ProfileSeries, params: SceneParams) -> WorldScene {
    let (floor_m, ceil_m) = altitude_range(series, params.show_freezing);
    let clamp = move |alt: f64| alt.clamp(floor_m, ceil_m);
    let norm = Norm {
        total_m: series.total_m.max(1.0),
        floor_m,
        range_m: (ceil_m - floor_m).max(1e-9),
    };

    // Terrain: contiguous runs with data; sub-2-station runs draw nothing.
    let mut terrain_runs: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut run: Vec<(f64, f64)> = Vec::new();
    for &(along, elevation) in &series.terrain {
        match elevation {
            Some(elevation) => run.push((along, clamp(elevation))),
            None => {
                if run.len() >= 2 {
                    terrain_runs.push(norm.simplify(std::mem::take(&mut run)));
                } else {
                    run.clear();
                }
            }
        }
    }
    if run.len() >= 2 {
        terrain_runs.push(norm.simplify(run));
    }
    // Fill mesh per run: the silhouette closed down to the chart floor.
    let terrain_meshes = terrain_runs
        .iter()
        .map(|run| {
            let mut polygon = run.clone();
            polygon.push((run[run.len() - 1].0, floor_m));
            polygon.push((run[0].0, floor_m));
            norm.tessellate(&polygon)
        })
        .collect();

    let obstacles = series
        .obstacles
        .iter()
        .map(|&(along, top)| (along, clamp(top)))
        .collect();

    let bands = series
        .bands
        .iter()
        .enumerate()
        .filter(|(index, band)| {
            params.band_styles.get(*index).is_some() && band.stations.len() >= 2
        })
        .map(|(index, band)| {
            // Top edge (ceiling; UNL caps at the chart ceiling) left→right,
            // then floor right→left — each edge simplified on its own so
            // the shared entry/exit corners stay exact.
            let mut thickness_sum = 0.0f64;
            let mut ceiling_edge: Vec<(f64, f64)> = Vec::with_capacity(band.stations.len());
            let mut floor_edge: Vec<(f64, f64)> = Vec::with_capacity(band.stations.len());
            for station in &band.stations {
                let top = station.ceiling_m.map(clamp).unwrap_or(ceil_m);
                thickness_sum += (top - clamp(station.floor_m)).max(0.0);
                ceiling_edge.push((station.along_m, top));
                floor_edge.push((station.along_m, clamp(station.floor_m)));
            }
            let mut polygon = norm.simplify(ceiling_edge);
            let mut floor_edge = norm.simplify(floor_edge);
            floor_edge.reverse();
            polygon.extend(floor_edge);
            let (left_m, right_m) = (
                band.stations[0].along_m,
                band.stations[band.stations.len() - 1].along_m,
            );
            let center = &band.stations[band.stations.len() / 2];
            let center_top_m = center.ceiling_m.map(clamp).unwrap_or(ceil_m);
            WorldBand {
                series_index: index,
                mesh: norm.tessellate(&polygon),
                polygon,
                label: SharedString::from(band.label.clone()),
                center_along_m: (left_m + right_m) / 2.0,
                center_alt_m: (center_top_m + clamp(center.floor_m)) / 2.0,
                center_top_m,
                left_m,
                right_m,
                thickness_m: thickness_sum / band.stations.len() as f64,
                conflict: band.conflict,
            }
        })
        .collect();

    let segments = |values: &[Option<f64>]| {
        let mut out: Vec<(f64, f64, f64)> = Vec::new();
        let mut start_m = 0.0f64;
        for (leg, value) in values.iter().enumerate() {
            let end_m = series.leg_ends_m.get(leg).copied().unwrap_or(series.total_m);
            if let Some(value) = value {
                out.push((start_m, end_m, clamp(*value)));
            }
            start_m = end_m;
        }
        out
    };
    let msa = segments(&series.msa_m);
    let freezing = if params.show_freezing {
        segments(&series.freezing_m)
    } else {
        Vec::new()
    };
    let cloud_base = if params.show_cloud_base {
        segments(&series.cloud_base_m)
    } else {
        Vec::new()
    };

    let emphasis = emphasis_polygons(series, clamp)
        .into_iter()
        .map(|polygon| norm.tessellate(&polygon))
        .collect();

    let planned = series
        .planned
        .iter()
        .map(|&(along, alt)| (along, clamp(alt)))
        .collect();
    let markers = [series.toc, series.tod]
        .into_iter()
        .flatten()
        .map(|(along, alt)| (along, clamp(alt)))
        .collect();
    let waypoints = series
        .waypoints
        .iter()
        .map(|(along, ident)| (*along, SharedString::from(ident.clone())))
        .collect();

    WorldScene {
        params,
        total_m: series.total_m,
        floor_m,
        ceil_m,
        terrain_runs,
        terrain_meshes,
        obstacles,
        bands,
        msa,
        freezing,
        cloud_base,
        emphasis,
        planned,
        markers,
        waypoints,
    }
}

/// World → normalized chart space (`0..=NORM_SCALE` on both axes, y up):
/// the resolution-independent frame the cached meshes live in and the
/// simplification measures distances in (the chart is anisotropic —
/// meters along ≠ meters up — so world-space tolerances would be wrong).
#[derive(Debug, Clone, Copy)]
struct Norm {
    total_m: f64,
    floor_m: f64,
    range_m: f64,
}

impl Norm {
    fn point(&self, (along, alt): (f64, f64)) -> (f32, f32) {
        (
            (along / self.total_m * f64::from(NORM_SCALE)) as f32,
            ((alt - self.floor_m) / self.range_m * f64::from(NORM_SCALE)) as f32,
        )
    }

    /// Douglas-Peucker point selection at sub-pixel normalized tolerance.
    /// Returns a subset of the input points (world coordinates untouched).
    fn simplify(&self, points: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
        if points.len() <= 2 {
            return points;
        }
        let scaled: Vec<(f64, f64)> = points
            .iter()
            .map(|&(along, alt)| {
                (
                    along / self.total_m,
                    (alt - self.floor_m) / self.range_m,
                )
            })
            .collect();
        let mut keep = vec![false; points.len()];
        keep[0] = true;
        keep[points.len() - 1] = true;
        // Iterative DP (explicit stack — corridor runs can be long).
        let mut stack = vec![(0usize, points.len() - 1)];
        while let Some((start, end)) = stack.pop() {
            if end <= start + 1 {
                continue;
            }
            let (sx, sy) = scaled[start];
            let (ex, ey) = scaled[end];
            let (dx, dy) = (ex - sx, ey - sy);
            let len = (dx * dx + dy * dy).sqrt();
            let mut worst = (0usize, 0.0f64);
            for (index, &(x, y)) in scaled.iter().enumerate().take(end).skip(start + 1) {
                // Perpendicular distance to the chord (degenerate chord:
                // plain point distance).
                let d = if len <= f64::EPSILON {
                    ((x - sx).powi(2) + (y - sy).powi(2)).sqrt()
                } else {
                    ((x - sx) * dy - (y - sy) * dx).abs() / len
                };
                if d > worst.1 {
                    worst = (index, d);
                }
            }
            if worst.1 > SIMPLIFY_TOLERANCE_NORM {
                keep[worst.0] = true;
                stack.push((start, worst.0));
                stack.push((worst.0, end));
            }
        }
        points
            .into_iter()
            .zip(keep)
            .filter_map(|(p, k)| k.then_some(p))
            .collect()
    }

    /// Tessellates a closed world-space polygon into a normalized fill
    /// mesh (lyon, via gpui's `PathBuilder` — windowless). Empty when the
    /// polygon is degenerate or tessellation fails.
    fn tessellate(&self, polygon: &[(f64, f64)]) -> NormMesh {
        if polygon.len() < 3 {
            return Vec::new();
        }
        let mut builder = PathBuilder::fill();
        let first = self.point(polygon[0]);
        builder.move_to(point(px(first.0), px(first.1)));
        for &p in &polygon[1..] {
            let (x, y) = self.point(p);
            builder.line_to(point(px(x), px(y)));
        }
        builder.close();
        match builder.build() {
            Ok(path) => path
                .vertices
                .iter()
                .map(|v| (f32::from(v.xy_position.x), f32::from(v.xy_position.y)))
                .collect(),
            Err(_) => Vec::new(),
        }
    }
}

/// Altitude range covering every drawn series, padded at the top so the
/// planned line never hugs the frame. The floor sits at sea level unless
/// terrain (or a below-zero freezing estimate when shown) dips lower.
fn altitude_range(series: &ProfileSeries, show_freezing: bool) -> (f64, f64) {
    let mut min = 0.0f64;
    let mut max = f64::NEG_INFINITY;
    let mut take = |value: f64| {
        min = min.min(value);
        max = max.max(value);
    };
    for &(_, t) in &series.terrain {
        if let Some(t) = t {
            take(t);
        }
    }
    for &(_, top) in &series.obstacles {
        take(top);
    }
    for &(_, alt) in &series.planned {
        take(alt);
    }
    for msa in series.msa_m.iter().flatten() {
        take(*msa);
    }
    if show_freezing {
        for level in series.freezing_m.iter().flatten() {
            take(*level);
        }
    }
    if !max.is_finite() {
        max = 1000.0;
    }
    let span = (max - min).max(400.0);
    (min, max + span * 0.14)
}

/// Red clearance emphasis: the closed area between terrain and the planned
/// line over each conflicted interval, in world coordinates.
fn emphasis_polygons(
    series: &ProfileSeries,
    clamp: impl Fn(f64) -> f64,
) -> Vec<Vec<(f64, f64)>> {
    // Half a station spacing, for single-station intervals.
    let half_step = series
        .terrain
        .windows(2)
        .map(|w| w[1].0 - w[0].0)
        .find(|d| *d > 0.0)
        .unwrap_or(250.0)
        / 2.0;
    let mut polygons = Vec::with_capacity(series.emphasis.len());
    for &(start_m, end_m) in &series.emphasis {
        let (start_m, end_m) = if end_m - start_m < 1.0 {
            (start_m - half_step, end_m + half_step)
        } else {
            (start_m, end_m)
        };
        // Sample positions: interval ends + the terrain stations inside.
        let mut xs: Vec<f64> = vec![start_m.max(0.0)];
        xs.extend(
            series
                .terrain
                .iter()
                .map(|&(along, _)| along)
                .filter(|&along| along > start_m && along < end_m),
        );
        xs.push(end_m.min(series.total_m));

        let mut top: Vec<(f64, f64)> = Vec::with_capacity(xs.len());
        let mut bottom: Vec<(f64, f64)> = Vec::with_capacity(xs.len());
        for &along in &xs {
            let Some(planned) = series.planned_at(along) else {
                continue;
            };
            let Some(terrain) = series.terrain_at(along) else {
                continue;
            };
            top.push((along, clamp(planned.max(terrain))));
            bottom.push((along, clamp(planned.min(terrain))));
        }
        bottom.reverse();
        top.extend(bottom);
        if top.len() >= 3 {
            polygons.push(top);
        }
    }
    polygons
}

/// Shared fixtures for this module's and [`super::layout`]'s tests: a
/// representative 6-waypoint flight exercising every chart layer.
#[cfg(test)]
pub(crate) mod fixtures {
    use gpui::hsla;
    use strata_plan::conflict::ConflictSeverity;

    use super::super::series::{BandSeries, BandStation, ProfileSeries};
    use super::{BandStyle, Palette, SceneParams};

    pub(crate) fn test_palette() -> Palette {
        Palette {
            axis_text: hsla(0.0, 0.0, 0.6, 1.0),
            grid: hsla(0.0, 0.0, 0.3, 0.45),
            terrain_fill: hsla(0.3, 0.4, 0.4, 0.2),
            terrain_stroke: hsla(0.3, 0.4, 0.4, 0.55),
            obstacle: hsla(0.1, 0.8, 0.5, 1.0),
            planned: hsla(0.6, 0.8, 0.5, 1.0),
            marker_fill: hsla(0.6, 0.8, 0.7, 1.0),
            msa: hsla(0.12, 0.9, 0.5, 0.9),
            freezing: hsla(0.55, 0.9, 0.6, 1.0),
            cloud_base: hsla(0.0, 0.0, 0.6, 0.9),
            danger: hsla(0.0, 0.9, 0.5, 1.0),
            warning: hsla(0.12, 0.9, 0.5, 1.0),
        }
    }

    fn band_style() -> BandStyle {
        BandStyle {
            fill: hsla(0.7, 0.5, 0.5, 0.25),
            border: hsla(0.7, 0.5, 0.4, 1.0),
            border_width: 1.0,
            dash: Some((4.0, 2.0)),
            label: hsla(0.7, 0.5, 0.3, 1.0),
        }
    }

    pub(crate) fn test_params(generation: u64, band_count: usize) -> SceneParams {
        SceneParams {
            generation,
            palette: test_palette(),
            band_styles: vec![band_style(); band_count],
            show_freezing: true,
            show_cloud_base: false,
        }
    }

    /// A 6-waypoint flight's worth of series data exercising every layer:
    /// terrain with a coverage gap, an obstacle, a sloped band + an UNL
    /// band with a conflict, MSA/freezing per leg, an emphasis interval,
    /// TOC/TOD.
    pub(crate) fn test_series() -> ProfileSeries {
        let terrain = (0..=20)
            .map(|i| {
                let along = i as f64 * 5_000.0;
                // A gap at stations 14–15 (outside elevation coverage).
                let elevation = match i {
                    14 | 15 => None,
                    _ => Some(200.0 + 60.0 * i as f64),
                };
                (along, elevation)
            })
            .collect();
        ProfileSeries {
            total_m: 100_000.0,
            terrain,
            obstacles: vec![(22_000.0, 700.0)],
            planned: vec![
                (0.0, 300.0),
                (12_000.0, 1500.0),
                (88_000.0, 1500.0),
                (100_000.0, 450.0),
            ],
            toc: Some((12_000.0, 1500.0)),
            tod: Some((88_000.0, 1500.0)),
            leg_ends_m: vec![18_000.0, 40_000.0, 62_000.0, 80_000.0, 100_000.0],
            waypoints: vec![
                (0.0, "EDFE".into()),
                (18_000.0, "WP1".into()),
                (40_000.0, "WP2".into()),
                (62_000.0, "WP3".into()),
                (80_000.0, "WP4".into()),
                (100_000.0, "EDQN".into()),
            ],
            msa_m: vec![Some(900.0), Some(1100.0), None, Some(1300.0), Some(800.0)],
            freezing_m: vec![Some(2600.0), Some(2600.0), Some(2500.0), None, Some(2400.0)],
            cloud_base_m: vec![None; 5],
            bands: vec![
                BandSeries {
                    airspace: test_airspace("EDGGN CTR"),
                    style: strata_render::features::AirspaceStyleKey::Ctr,
                    label: "CTR D · GND – 3500 ft".into(),
                    entry_m: 30_000.0,
                    exit_m: 50_000.0,
                    stations: vec![
                        BandStation {
                            along_m: 30_000.0,
                            floor_m: 0.0,
                            ceiling_m: Some(1100.0),
                        },
                        BandStation {
                            along_m: 40_000.0,
                            floor_m: 0.0,
                            ceiling_m: Some(1100.0),
                        },
                        BandStation {
                            along_m: 50_000.0,
                            floor_m: 0.0,
                            ceiling_m: Some(1100.0),
                        },
                    ],
                    conflict: Some(ConflictSeverity::Caution),
                },
                BandSeries {
                    airspace: test_airspace("HIGH UNL"),
                    style: strata_render::features::AirspaceStyleKey::Danger,
                    label: "ED-D 1 · 2000 ft – UNL".into(),
                    entry_m: 60_000.0,
                    exit_m: 90_000.0,
                    stations: vec![
                        BandStation {
                            along_m: 60_000.0,
                            floor_m: 600.0,
                            ceiling_m: None,
                        },
                        BandStation {
                            along_m: 90_000.0,
                            floor_m: 900.0,
                            ceiling_m: None,
                        },
                    ],
                    conflict: None,
                },
                // Degenerate: one station → must be dropped by the world
                // stage.
                BandSeries {
                    airspace: test_airspace("DEGENERATE"),
                    style: strata_render::features::AirspaceStyleKey::Ctr,
                    label: "X".into(),
                    entry_m: 1_000.0,
                    exit_m: 1_100.0,
                    stations: vec![BandStation {
                        along_m: 1_000.0,
                        floor_m: 0.0,
                        ceiling_m: Some(100.0),
                    }],
                    conflict: None,
                },
            ],
            emphasis: vec![(55_000.0, 65_000.0)],
            eta: Vec::new(),
        }
    }

    fn test_airspace(name: &str) -> strata_data::domain::Airspace {
        use strata_data::domain::*;
        Airspace {
            name: name.to_owned(),
            class: AirspaceClass::D,
            kind: AirspaceKind::Ctr,
            lower: VerticalLimit::gnd(),
            upper: VerticalLimit::unl(),
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
}

#[cfg(test)]
mod tests {
    use super::fixtures::{test_params, test_series};
    use super::*;

    #[test]
    fn world_scene_clamps_and_caps_unl_at_the_chart_ceiling() {
        let series = test_series();
        let world = build_world_scene(&series, test_params(1, series.bands.len()));

        assert!(world.ceil_m > world.floor_m);
        // Freezing at 2600 m is shown, so the range must cover it.
        assert!(world.ceil_m > 2600.0);

        // The UNL band's ceiling edge sits exactly on the chart ceiling.
        let unl = world
            .bands
            .iter()
            .find(|b| b.series_index == 1)
            .expect("UNL band kept");
        assert!(
            unl.polygon[..2].iter().all(|&(_, alt)| alt == world.ceil_m),
            "UNL ceiling caps at ceil_m"
        );
        assert_eq!(unl.center_top_m, world.ceil_m);

        // Every stored altitude is inside the chart range.
        let all_alts = world
            .terrain_runs
            .iter()
            .flatten()
            .chain(world.planned.iter())
            .chain(world.markers.iter())
            .chain(world.obstacles.iter())
            .chain(world.bands.iter().flat_map(|b| b.polygon.iter()));
        for &(_, alt) in all_alts {
            assert!(
                (world.floor_m..=world.ceil_m).contains(&alt),
                "altitude {alt} outside {}..{}",
                world.floor_m,
                world.ceil_m
            );
        }

        // Every cached mesh is whole triangles inside the normalized frame.
        let meshes = world
            .terrain_meshes
            .iter()
            .chain(world.bands.iter().map(|b| &b.mesh))
            .chain(world.emphasis.iter());
        for mesh in meshes {
            assert!(!mesh.is_empty(), "fill mesh tessellated");
            assert_eq!(mesh.len() % 3, 0, "whole triangles");
            for &(x, y) in mesh {
                assert!((-0.01..=NORM_SCALE + 0.01).contains(&x), "x {x}");
                assert!((-0.01..=NORM_SCALE + 0.01).contains(&y), "y {y}");
            }
        }
    }

    #[test]
    fn simplification_collapses_flat_edges_and_keeps_peaks() {
        let series = test_series();
        let world = build_world_scene(&series, test_params(1, series.bands.len()));

        // Band 0: flat AMSL ceiling and GND floor across three stations —
        // each edge collapses to its endpoints (4-point polygon).
        let ctr = &world.bands[0];
        assert_eq!(ctr.polygon.len(), 4, "{:?}", ctr.polygon);
        assert_eq!(ctr.polygon[0], (30_000.0, 1100.0));
        assert_eq!(ctr.polygon[1], (50_000.0, 1100.0));

        // The terrain in the fixture climbs linearly — runs collapse to
        // their endpoints, but the original world values survive exactly
        // (simplification selects points, never moves them).
        for run in &world.terrain_runs {
            assert_eq!(run.len(), 2, "linear terrain collapses");
        }
        assert_eq!(world.terrain_runs[0][0], (0.0, 200.0));

        // A peak well above tolerance always survives.
        let norm = Norm {
            total_m: 100_000.0,
            floor_m: 0.0,
            range_m: 3_000.0,
        };
        let jagged = vec![
            (0.0, 100.0),
            (25_000.0, 100.0),
            (50_000.0, 900.0), // the peak
            (75_000.0, 100.0),
            (100_000.0, 100.0),
        ];
        let simplified = norm.simplify(jagged.clone());
        assert!(simplified.contains(&(50_000.0, 900.0)), "{simplified:?}");
        assert_eq!(simplified.first(), jagged.first());
        assert_eq!(simplified.last(), jagged.last());
    }

    #[test]
    fn world_scene_splits_terrain_runs_and_drops_degenerate_bands() {
        let series = test_series();
        let world = build_world_scene(&series, test_params(1, series.bands.len()));

        // The None gap at stations 14–15 splits the silhouette in two.
        assert_eq!(world.terrain_runs.len(), 2);
        assert!(world.terrain_runs.iter().all(|run| run.len() >= 2));

        // The single-station band is gone; the other two survive with
        // their series indices intact (hit-test contract).
        let indices: Vec<usize> = world.bands.iter().map(|b| b.series_index).collect();
        assert_eq!(indices, vec![0, 1]);

        // Per-leg segments skip data-less legs and end at the route end.
        assert_eq!(world.msa.len(), 4, "leg 2 has no MSA");
        assert_eq!(world.freezing.len(), 4, "leg 3 has no freezing level");
        assert_eq!(world.freezing.last().map(|s| s.1), Some(100_000.0));
        assert!(world.cloud_base.is_empty(), "toggle off");
    }

    #[test]
    fn weather_toggles_are_part_of_the_world_cache_key() {
        let series = test_series();
        let on = test_params(1, series.bands.len());
        let world = build_world_scene(&series, test_params(1, series.bands.len()));
        assert!(world.params_match(&on));

        let mut off = test_params(1, series.bands.len());
        off.show_freezing = false;
        assert!(!world.params_match(&off));

        let world_off = build_world_scene(&series, off);
        assert!(world_off.freezing.is_empty());
        // Without the freezing overlay the range shrinks below 2600 m
        // (the highest other input is the freezing level).
        assert!(world_off.ceil_m < world.ceil_m);

        let mut next_gen = test_params(2, series.bands.len());
        next_gen.show_freezing = true;
        assert!(
            !world.params_match(&next_gen),
            "generation bump invalidates"
        );
    }
}
