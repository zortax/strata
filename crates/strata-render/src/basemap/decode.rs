//! Worker-side tile pipeline: raw [`crate::tiles::TileSource`] bytes →
//! (gzip/zlib) decompression → geozero MVT decode → style lookup →
//! lyon tessellation → [`TileData`]. Never runs on the render thread.

use crate::basemap::labels::{self, LabelSpec};
use crate::basemap::style::{self, FeatureProperties, PropertyValue};
use crate::basemap::tess::{MeshBuilder, MeshData};
use crate::map_theme::BasemapTheme;
use crate::tiles::TileId;

use geozero::GeomProcessor;
use geozero::mvt::{Message, Tile, tile};
use glam::DVec2;

use std::borrow::Cow;
use std::io::Read;
use std::panic::{AssertUnwindSafe, catch_unwind};

/// MVT layer carrying place names in the Protomaps schema (v4 and v5).
const PLACES_LAYER: &str = "places";
/// MVT layers whose named lines get waterway labels.
const WATERWAY_LAYERS: &[&str] = &["water", "physical_line"];

/// Errors of the worker-side decode pipeline. Handled internally (log +
/// negative cache), never crosses the layer API.
#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("tile decompression failed: {0}")]
    Decompress(#[from] std::io::Error),
    #[error("MVT protobuf decode failed: {0}")]
    Mvt(String),
}

/// Decoded, tessellated content of one tile.
#[derive(Debug, Clone, Default)]
pub struct TileData {
    pub mesh: MeshData,
    pub labels: Vec<LabelSpec>,
}

/// How a tile job ended.
#[derive(Debug)]
pub enum TileOutcome {
    /// Decoded and tessellated, ready for GPU upload.
    Loaded(TileData),
    /// Authoritative miss (absent at source, or broken beyond decode):
    /// negative-cached so the tile is not re-requested in a hot loop.
    Missing,
    /// The job was superseded before it ran (tile no longer wanted): nothing
    /// is cached, so the tile is re-requested if it becomes wanted again.
    Skipped,
}

/// Worker job result.
#[derive(Debug)]
pub struct DecodedTile {
    pub id: TileId,
    pub outcome: TileOutcome,
}

impl DecodedTile {
    /// A job that bailed out before doing any work (see
    /// [`TileOutcome::Skipped`]).
    pub fn skipped(id: TileId) -> Self {
        Self {
            id,
            outcome: TileOutcome::Skipped,
        }
    }
}

/// Job entry point: turn optional source bytes into a [`DecodedTile`],
/// catching panics from hostile geometry (the worker pool stays healthy and
/// the in-flight bookkeeping always resolves).
pub fn decode_tile(id: TileId, bytes: Option<Vec<u8>>, theme: &BasemapTheme) -> DecodedTile {
    let Some(bytes) = bytes else {
        return DecodedTile {
            id,
            outcome: TileOutcome::Missing,
        };
    };
    let outcome = match catch_unwind(AssertUnwindSafe(|| build_tile(id, &bytes, theme))) {
        Ok(Ok(data)) => TileOutcome::Loaded(data),
        Ok(Err(error)) => {
            tracing::warn!(tile = ?id, %error, "basemap tile decode failed");
            TileOutcome::Missing
        }
        Err(_) => {
            tracing::error!(tile = ?id, "basemap tile decode panicked");
            TileOutcome::Missing
        }
    };
    DecodedTile { id, outcome }
}

/// Decode + tessellate one tile with `theme` colors. Styles are evaluated
/// at the tile's own zoom level (the camera overzooms the mesh; stroke
/// widths stay in logical px).
pub fn build_tile(id: TileId, raw: &[u8], theme: &BasemapTheme) -> Result<TileData, DecodeError> {
    let bytes = decompress(raw)?;
    let mvt = Tile::decode(bytes.as_ref()).map_err(|e| DecodeError::Mvt(e.to_string()))?;
    let zoom = id.z as f64;
    let (tile_origin, _) = id.world_bounds();
    let tile_world_size = id.world_size();
    let to_world =
        |p: [f32; 2]| tile_origin + DVec2::new(p[0] as f64, p[1] as f64) * tile_world_size;

    let mut builder = MeshBuilder::new();
    let mut tile_labels = Vec::new();

    // Deterministic paint order regardless of layer order inside the tile.
    let mut painted: Vec<&tile::Layer> = mvt
        .layers
        .iter()
        .filter(|l| style::layer_rank(&l.name).is_some())
        .collect();
    painted.sort_by_key(|l| style::layer_rank(&l.name));

    for layer in painted {
        let extent = layer.extent.unwrap_or(4096).max(1) as f32;
        let scale = 1.0 / extent;
        for feature in &layer.features {
            let properties = decode_properties(layer, feature);
            let Some(paint) = style::style_for(theme, &layer.name, &properties, zoom) else {
                continue;
            };
            let mut collector = PathCollector::new(scale);
            if let Err(error) = geozero::mvt::process_geom(feature, &mut collector) {
                tracing::warn!(tile = ?id, layer = %layer.name, %error, "bad MVT geometry");
                continue;
            }
            match geom_type(feature) {
                tile::GeomType::Polygon => {
                    if let Some(fill) = paint.fill {
                        builder.add_fill(&collector.paths, fill.color);
                    }
                    if let Some(stroke) = paint.stroke {
                        builder.add_stroke(&collector.paths, &stroke);
                    }
                }
                tile::GeomType::Linestring => {
                    if let Some(stroke) = paint.stroke {
                        builder.add_stroke(&collector.paths, &stroke);
                    }
                    if WATERWAY_LAYERS.contains(&layer.name.as_str())
                        && let Some(anchor) = line_anchor(&collector.paths)
                        && let Some(label) =
                            labels::waterway_label(theme, &properties, to_world(anchor))
                    {
                        tile_labels.push(label);
                    }
                }
                tile::GeomType::Point | tile::GeomType::Unknown => {}
            }
        }
    }

    // Place labels (point features; never painted as geometry).
    for layer in mvt.layers.iter().filter(|l| l.name == PLACES_LAYER) {
        let extent = layer.extent.unwrap_or(4096).max(1) as f32;
        for feature in &layer.features {
            if geom_type(feature) != tile::GeomType::Point {
                continue;
            }
            let mut collector = PathCollector::new(1.0 / extent);
            if geozero::mvt::process_geom(feature, &mut collector).is_err() {
                continue;
            }
            let Some(point) = collector.paths.first().and_then(|p| p.first()).copied() else {
                continue;
            };
            let properties = decode_properties(layer, feature);
            if let Some(label) = labels::place_label(theme, &properties, to_world(point)) {
                tile_labels.push(label);
            }
        }
    }

    Ok(TileData {
        mesh: builder.finish(),
        labels: tile_labels,
    })
}

/// Tile payloads are gzip in PMTiles practice; zlib and raw protobuf are
/// sniffed and handled too.
fn decompress(bytes: &[u8]) -> Result<Cow<'_, [u8]>, DecodeError> {
    if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut out = Vec::new();
        flate2::read::MultiGzDecoder::new(bytes).read_to_end(&mut out)?;
        Ok(Cow::Owned(out))
    } else if matches!(bytes, [0x78, 0x01 | 0x5e | 0x9c | 0xda, ..]) {
        let mut out = Vec::new();
        flate2::read::ZlibDecoder::new(bytes).read_to_end(&mut out)?;
        Ok(Cow::Owned(out))
    } else {
        Ok(Cow::Borrowed(bytes))
    }
}

fn geom_type(feature: &tile::Feature) -> tile::GeomType {
    match feature.r#type {
        Some(t) if t == tile::GeomType::Point as i32 => tile::GeomType::Point,
        Some(t) if t == tile::GeomType::Linestring as i32 => tile::GeomType::Linestring,
        Some(t) if t == tile::GeomType::Polygon as i32 => tile::GeomType::Polygon,
        _ => tile::GeomType::Unknown,
    }
}

/// Resolve a feature's dictionary-encoded tags against the layer tables.
fn decode_properties(layer: &tile::Layer, feature: &tile::Feature) -> FeatureProperties {
    let mut properties = FeatureProperties::default();
    for pair in feature.tags.chunks_exact(2) {
        let (Some(key), Some(value)) = (
            layer.keys.get(pair[0] as usize),
            layer.values.get(pair[1] as usize),
        ) else {
            continue;
        };
        let value = if let Some(s) = &value.string_value {
            PropertyValue::Str(s.clone())
        } else if let Some(v) = value.double_value {
            PropertyValue::F64(v)
        } else if let Some(v) = value.float_value {
            PropertyValue::F64(v as f64)
        } else if let Some(v) = value.int_value {
            PropertyValue::I64(v)
        } else if let Some(v) = value.uint_value {
            PropertyValue::I64(v as i64)
        } else if let Some(v) = value.sint_value {
            PropertyValue::I64(v)
        } else if let Some(v) = value.bool_value {
            PropertyValue::Bool(v)
        } else {
            continue;
        };
        properties.insert(key.clone(), value);
    }
    properties
}

/// Collects MVT geometry into tile-local (0..1) polylines/rings. Each
/// `*_begin` opens a new path; geozero's MVT reader feeds integer tile
/// coordinates through `xy`.
struct PathCollector {
    scale: f32,
    paths: Vec<Vec<[f32; 2]>>,
}

impl PathCollector {
    fn new(scale: f32) -> Self {
        Self {
            scale,
            paths: Vec::new(),
        }
    }
}

impl GeomProcessor for PathCollector {
    fn xy(&mut self, x: f64, y: f64, _idx: usize) -> geozero::error::Result<()> {
        if x.is_finite() && y.is_finite() {
            if self.paths.is_empty() {
                self.paths.push(Vec::new());
            }
            if let Some(path) = self.paths.last_mut() {
                path.push([x as f32 * self.scale, y as f32 * self.scale]);
            }
        }
        Ok(())
    }

    fn point_begin(&mut self, _idx: usize) -> geozero::error::Result<()> {
        self.paths.push(Vec::with_capacity(1));
        Ok(())
    }

    fn multipoint_begin(&mut self, _size: usize, _idx: usize) -> geozero::error::Result<()> {
        self.paths.push(Vec::new());
        Ok(())
    }

    fn linestring_begin(
        &mut self,
        _tagged: bool,
        size: usize,
        _idx: usize,
    ) -> geozero::error::Result<()> {
        self.paths.push(Vec::with_capacity(size));
        Ok(())
    }
}

/// Representative anchor for a line label: the middle vertex of the longest
/// polyline.
fn line_anchor(paths: &[Vec<[f32; 2]>]) -> Option<[f32; 2]> {
    let longest = paths.iter().max_by(|a, b| {
        polyline_length(a)
            .partial_cmp(&polyline_length(b))
            .unwrap_or(std::cmp::Ordering::Equal)
    })?;
    longest.get(longest.len() / 2).copied()
}

fn polyline_length(path: &[[f32; 2]]) -> f32 {
    path.windows(2)
        .map(|w| {
            let (dx, dy) = (w[1][0] - w[0][0], w[1][1] - w[0][1]);
            (dx * dx + dy * dy).sqrt()
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map_theme::MapTheme;

    fn theme() -> BasemapTheme {
        MapTheme::oldworld().basemap
    }

    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::Write;

    /// MVT command integer: `(id & 0x7) | (count << 3)`.
    fn cmd(id: u32, count: u32) -> u32 {
        (id & 0x7) | (count << 3)
    }

    /// MVT parameter integer (zigzag).
    fn zig(v: i32) -> u32 {
        ((v << 1) ^ (v >> 31)) as u32
    }

    fn value_str(s: &str) -> tile::Value {
        tile::Value {
            string_value: Some(s.to_owned()),
            ..Default::default()
        }
    }

    /// A tiny hand-built Protomaps-style tile: a water square, one motorway
    /// across, and a named place point.
    fn tiny_tile() -> Tile {
        let water = tile::Layer {
            version: 2,
            name: "water".to_owned(),
            extent: Some(4096),
            features: vec![tile::Feature {
                id: Some(1),
                tags: vec![],
                r#type: Some(tile::GeomType::Polygon as i32),
                // Square ring: MoveTo(0,0), LineTo(4096,0)(0,4096)(-4096,0), Close.
                geometry: vec![
                    cmd(1, 1),
                    zig(0),
                    zig(0),
                    cmd(2, 3),
                    zig(4096),
                    zig(0),
                    zig(0),
                    zig(4096),
                    zig(-4096),
                    zig(0),
                    cmd(7, 1),
                ],
            }],
            keys: vec![],
            values: vec![],
        };
        let roads = tile::Layer {
            version: 2,
            name: "roads".to_owned(),
            extent: Some(4096),
            features: vec![tile::Feature {
                id: Some(2),
                tags: vec![0, 0],
                r#type: Some(tile::GeomType::Linestring as i32),
                // Horizontal line: MoveTo(0,2048), LineTo(4096,2048).
                geometry: vec![cmd(1, 1), zig(0), zig(2048), cmd(2, 1), zig(4096), zig(0)],
            }],
            keys: vec!["kind".to_owned()],
            values: vec![value_str("highway")],
        };
        let places = tile::Layer {
            version: 2,
            name: "places".to_owned(),
            extent: Some(4096),
            features: vec![tile::Feature {
                id: Some(3),
                tags: vec![0, 0, 1, 1],
                r#type: Some(tile::GeomType::Point as i32),
                geometry: vec![cmd(1, 1), zig(2048), zig(2048)],
            }],
            keys: vec!["name".to_owned(), "kind".to_owned()],
            values: vec![value_str("Berlin"), value_str("locality")],
        };
        Tile {
            layers: vec![water, roads, places],
        }
    }

    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(bytes).expect("gzip write");
        encoder.finish().expect("gzip finish")
    }

    #[test]
    fn gzipped_mvt_round_trips_into_mesh_and_labels() {
        let id = TileId::new(8, 135, 84).expect("valid tile");
        let raw = tiny_tile().encode_to_vec();
        let data = build_tile(id, &gzip(&raw), &theme()).expect("decode gzipped tile");

        assert!(!data.mesh.is_empty(), "mesh must contain triangles");
        // Water fill triangles present (fill vertices: zero width).
        assert!(
            data.mesh
                .vertices
                .iter()
                .any(|v| v.width_px == 0.0 && v.color == theme().water)
        );
        // Motorway stroke present (extruded vertices with width > 0).
        assert!(data.mesh.vertices.iter().any(|v| v.width_px > 0.0));
        // Fill vertices are tile-local 0..1.
        for v in data.mesh.vertices.iter().filter(|v| v.width_px == 0.0) {
            assert!((-0.01..=1.01).contains(&v.pos[0]));
            assert!((-0.01..=1.01).contains(&v.pos[1]));
        }
        // Place label extracted with a world anchor inside the tile bounds.
        let berlin = data
            .labels
            .iter()
            .find(|l| &*l.text == "Berlin")
            .expect("place label");
        let (min, max) = id.world_bounds();
        assert!(berlin.world.x > min.x && berlin.world.x < max.x);
        assert!(berlin.world.y > min.y && berlin.world.y < max.y);
    }

    #[test]
    fn uncompressed_and_zlib_payloads_also_decode() {
        let id = TileId::new(8, 135, 84).expect("valid tile");
        let raw = tiny_tile().encode_to_vec();
        assert!(!build_tile(id, &raw, &theme()).expect("raw").mesh.is_empty());

        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&raw).expect("zlib write");
        let zlibbed = encoder.finish().expect("zlib finish");
        assert!(
            !build_tile(id, &zlibbed, &theme())
                .expect("zlib")
                .mesh
                .is_empty()
        );
    }

    #[test]
    fn garbage_bytes_error_instead_of_panicking() {
        let id = TileId::new(8, 135, 84).expect("valid tile");
        assert!(build_tile(id, &[0xde, 0xad, 0xbe, 0xef, 0x42], &theme()).is_err());
    }

    #[test]
    fn decode_tile_handles_missing_and_broken_input() {
        let id = TileId::new(8, 135, 84).expect("valid tile");
        assert!(matches!(
            decode_tile(id, None, &theme()).outcome,
            TileOutcome::Missing
        ));
        assert!(matches!(
            decode_tile(id, Some(vec![0xff; 16]), &theme()).outcome,
            TileOutcome::Missing
        ));
        let ok = decode_tile(id, Some(tiny_tile().encode_to_vec()), &theme());
        assert!(matches!(ok.outcome, TileOutcome::Loaded(data) if !data.mesh.is_empty()));
    }

    #[test]
    fn worker_round_trip_through_job_queue() {
        use crate::workers::{JobQueue, WorkerPool};
        use std::time::{Duration, Instant};

        let pool = WorkerPool::new(2);
        let mut queue: JobQueue<DecodedTile> = JobQueue::new();
        let id = TileId::new(8, 135, 84).expect("valid tile");
        let bytes = gzip(&tiny_tile().encode_to_vec());
        queue.submit(&pool, move || decode_tile(id, Some(bytes), &theme()));

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut results = Vec::new();
        while results.is_empty() && Instant::now() < deadline {
            results.extend(queue.drain());
            std::thread::sleep(Duration::from_millis(1));
        }
        let decoded = results.pop().expect("worker result within deadline");
        assert_eq!(decoded.id, id);
        assert!(matches!(decoded.outcome, TileOutcome::Loaded(data) if !data.mesh.is_empty()));
    }
}
