//! Hillshade tile rendering: DEM → Horn hillshade → 256×256
//! grayscale+alpha PNG slippy tiles, streamed to a sink.
//!
//! Per output pixel the DEM is sampled bilinearly at the pixel's geographic
//! position; the Horn gradient uses neighboring pixel positions, so the
//! sample spacing automatically tracks meters-per-pixel at the tile's
//! latitude. For coarse zoom levels the DEM is pre-downsampled (power-of-two
//! box filter) so the whole region fits in a bounded cache.
//!
//! Alpha encodes DEM coverage: pixels whose position has no DEM data —
//! outside the rendered region, on an unpublished all-sea square, or in a
//! void — are fully transparent (alpha 0) so the renderer never paints a
//! flat fake tone there; pixels with data are fully opaque (alpha 255).

use std::collections::BTreeSet;
use std::io::Cursor;

use image::{GrayAlphaImage, ImageFormat};

use crate::Error;
use crate::domain::BoundingBox;
use crate::providers::{DemTileId, TerrainProvider};

use super::dem_cache::{DemCache, ElevationSampler};
use super::hillshade::shade_pixel;
use super::tile_math::{
    TILE_SIZE, TileRange, ground_resolution_m_per_px, tile_extent_deg, x_norm_to_lon,
    y_norm_to_lat,
};
use super::{CopernicusError, dem_tiles_in};

/// One rendered hillshade raster tile in slippy-map (XYZ, web-mercator)
/// addressing: `y` grows southward.
#[derive(Debug, Clone, PartialEq)]
pub struct TerrainTile {
    pub z: u8,
    pub x: u32,
    pub y: u32,
    /// Encoded grayscale+alpha PNG bytes; alpha is 0 wherever the DEM has
    /// no data (transparent), 255 where it does.
    pub png: Vec<u8>,
}

/// Soft cap for decoded DEM data held in memory while rendering.
const CACHE_BUDGET_BYTES: usize = 512 * 1024 * 1024;

/// GLO-30 ground sample distance (approximately constant with latitude
/// because the product widens its longitude spacing in bands).
const DEM_GROUND_SPACING_M: f64 = 30.0;

const MAX_DOWNSAMPLE_FACTOR: u32 = 64;

/// Renders hillshade PNG tiles for a zoom range from DEM data and hands
/// each finished tile to a sink (the ingest CLI writes them to the store).
pub struct HillshadeTiler {
    min_zoom: u8,
    max_zoom: u8,
}

impl HillshadeTiler {
    /// Spec default range is z5..=z11. Arguments are reordered if reversed.
    pub fn new(min_zoom: u8, max_zoom: u8) -> Self {
        Self { min_zoom: min_zoom.min(max_zoom), max_zoom: min_zoom.max(max_zoom) }
    }

    /// Number of tiles [`Self::render_tiles`] will produce for `bbox` —
    /// lets callers size progress bars before rendering.
    pub fn count_tiles(&self, bbox: BoundingBox) -> usize {
        (self.min_zoom..=self.max_zoom)
            .map(|z| TileRange::covering(z, &bbox).count())
            .sum()
    }

    /// Renders every tile of `bbox` across the configured zoom range,
    /// fetching DEM data from `provider` as needed. Returns the number of
    /// tiles produced. The sink is called once per finished tile; a sink
    /// error aborts the run.
    pub async fn render_tiles<F>(
        &self,
        provider: &dyn TerrainProvider,
        bbox: BoundingBox,
        sink: F,
    ) -> Result<usize, Error>
    where
        F: FnMut(TerrainTile) -> Result<(), Error> + Send,
    {
        self.render_tiles_with_progress(provider, bbox, sink, |_, _| {}).await
    }

    /// Like [`Self::render_tiles`] but reports `(done, total)` after every
    /// finished tile.
    pub async fn render_tiles_with_progress<F, P>(
        &self,
        provider: &dyn TerrainProvider,
        bbox: BoundingBox,
        mut sink: F,
        mut progress: P,
    ) -> Result<usize, Error>
    where
        F: FnMut(TerrainTile) -> Result<(), Error> + Send,
        P: FnMut(usize, usize) + Send,
    {
        let total = self.count_tiles(bbox);
        let coverage: BTreeSet<DemTileId> = provider.tiles_for(bbox).into_iter().collect();
        let cache = DemCache::new(CACHE_BUDGET_BYTES);
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);

        let mut done = 0usize;
        for z in self.min_zoom..=self.max_zoom {
            let range = TileRange::covering(z, &bbox);
            let factor = downsample_factor(z, bbox.center().lat());
            let coords: Vec<(u32, u32)> = range.iter().collect();
            tracing::debug!(
                zoom = z,
                tiles = coords.len(),
                dem_downsample = factor,
                "rendering hillshade zoom level"
            );

            // Batches sized to the thread pool keep the DEM working set
            // small at high zoom (row-major order gives tile locality).
            for batch in coords.chunks(threads.max(1)) {
                let needed: Vec<DemTileId> = batch
                    .iter()
                    .flat_map(|&(x, y)| dem_ids_for_tile(z, x, y))
                    .filter(|id| coverage.contains(id))
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect();
                cache.ensure_loaded(provider, &needed, factor).await?;
                let sampler = cache.sampler(&needed, factor);

                for tile in render_batch(z, batch, &sampler, threads)? {
                    sink(tile)?;
                    done += 1;
                    progress(done, total);
                }
            }
        }
        Ok(done)
    }
}

/// Power-of-two DEM downsampling for a zoom level: coarsest factor that
/// still leaves at least two DEM samples per output pixel.
fn downsample_factor(zoom: u8, center_lat: f64) -> u32 {
    let mpp = ground_resolution_m_per_px(zoom, center_lat);
    let mut factor = 1u32;
    while factor < MAX_DOWNSAMPLE_FACTOR && DEM_GROUND_SPACING_M * (factor as f64) * 2.0 <= mpp {
        factor *= 2;
    }
    factor
}

/// DEM tiles needed to render slippy tile `(z, x, y)`, including a margin
/// of two output pixels for the Horn kernel at the tile edges.
fn dem_ids_for_tile(z: u8, x: u32, y: u32) -> Vec<DemTileId> {
    let (west, south, east, north) = tile_extent_deg(z, x, y);
    let margin_lat = (north - south) / TILE_SIZE as f64 * 2.0;
    let margin_lon = (east - west) / TILE_SIZE as f64 * 2.0;
    dem_tiles_in(
        west - margin_lon,
        south - margin_lat,
        east + margin_lon,
        north + margin_lat,
    )
}

/// Renders a batch of tiles on scoped threads, preserving input order.
fn render_batch(
    z: u8,
    coords: &[(u32, u32)],
    sampler: &ElevationSampler,
    threads: usize,
) -> Result<Vec<TerrainTile>, Error> {
    if coords.is_empty() {
        return Ok(Vec::new());
    }
    let chunk = coords.len().div_ceil(threads.max(1)).max(1);

    let mut rendered = Vec::with_capacity(coords.len());
    std::thread::scope(|scope| -> Result<(), CopernicusError> {
        let handles: Vec<_> = coords
            .chunks(chunk)
            .map(|part| {
                scope.spawn(move || {
                    part.iter()
                        .map(|&(x, y)| render_tile(z, x, y, sampler))
                        .collect::<Result<Vec<_>, _>>()
                })
            })
            .collect();
        for handle in handles {
            let part = handle
                .join()
                .unwrap_or(Err(CopernicusError::RenderPanicked))?;
            rendered.extend(part);
        }
        Ok(())
    })?;
    Ok(rendered)
}

/// Luma stored under fully transparent (no-data) pixels: the flat-terrain
/// shade (`sin 45° · 255 ≈ 180`), so GPU bilinear filtering across the
/// data/no-data edge blends toward "flat", not toward black.
const NO_DATA_LUMA: u8 = 180;

/// Renders one 256×256 grayscale+alpha hillshade tile from the sampler.
/// Pixels without DEM data at their center are fully transparent.
fn render_tile(
    z: u8,
    x: u32,
    y: u32,
    sampler: &ElevationSampler,
) -> Result<TerrainTile, CopernicusError> {
    const T: usize = TILE_SIZE as usize;
    const PATCH: usize = T + 2;
    let n = (1u64 << z) as f64;

    // Pixel-center geographic coordinates with a one-pixel margin on every
    // side (patch index i corresponds to pixel i-1). Margins are absolute
    // geo positions, so adjacent tiles shade seamlessly.
    let lons: Vec<f64> = (0..PATCH)
        .map(|i| x_norm_to_lon((x as f64 + (i as f64 - 0.5) / T as f64) / n))
        .collect();
    let lats: Vec<f64> = (0..PATCH)
        .map(|j| y_norm_to_lat(((y as f64 + (j as f64 - 0.5) / T as f64) / n).clamp(0.0, 1.0)))
        .collect();

    // NaN = no DEM data at that position (see `ElevationSampler`).
    let mut patch = vec![f64::NAN; PATCH * PATCH];
    for (j, &lat) in lats.iter().enumerate() {
        let row = &mut patch[j * PATCH..(j + 1) * PATCH];
        for (i, &lon) in lons.iter().enumerate() {
            row[i] = sampler.elevation_or_nan_deg(lat, lon);
        }
    }

    // Interleaved luma+alpha, 2 bytes per pixel.
    let mut pixels = vec![0u8; T * T * 2];
    for r in 0..T {
        // Web mercator is conformal: one ground resolution for both axes.
        let res_m = ground_resolution_m_per_px(z, lats[r + 1]);
        for c in 0..T {
            let w = |dj: usize, di: usize| patch[(r + dj) * PATCH + (c + di)];
            let center = w(1, 1);
            let out = &mut pixels[(r * T + c) * 2..(r * T + c) * 2 + 2];
            if center.is_nan() {
                out[0] = NO_DATA_LUMA;
                out[1] = 0;
                continue;
            }
            // No-data neighbours take the center elevation: the gradient
            // flattens toward data edges instead of inventing cliffs.
            let s = |dj: usize, di: usize| {
                let v = w(dj, di);
                if v.is_nan() { center } else { v }
            };
            let window = [
                [s(0, 0), s(0, 1), s(0, 2)],
                [s(1, 0), s(1, 1), s(1, 2)],
                [s(2, 0), s(2, 1), s(2, 2)],
            ];
            out[0] = shade_pixel(&window, res_m, res_m);
            out[1] = u8::MAX;
        }
    }

    let image = GrayAlphaImage::from_raw(TILE_SIZE, TILE_SIZE, pixels)
        .ok_or(CopernicusError::Internal("hillshade pixel buffer size mismatch"))?;
    let mut png = Cursor::new(Vec::new());
    image
        .write_to(&mut png, ImageFormat::Png)
        .map_err(|source| CopernicusError::PngEncode { z, x, y, source })?;
    Ok(TerrainTile { z, x, y, png: png.into_inner() })
}

#[cfg(test)]
mod tests {
    use super::super::testutil::SyntheticDem;
    use super::*;

    fn bbox(w: f64, s: f64, e: f64, n: f64) -> BoundingBox {
        BoundingBox::new(w, s, e, n).expect("valid test bbox")
    }

    /// Plane rising to the south-east: its normal faces north-west,
    /// straight at the sun.
    fn nw_facing_plane(lat: f64, lon: f64) -> f64 {
        20_000.0 * ((lon - 10.0) + (51.0 - lat))
    }

    fn sea(_lat: f64, _lon: f64) -> f64 {
        0.0
    }

    async fn render_all(
        tiler: &HillshadeTiler,
        provider: &SyntheticDem,
        bbox: BoundingBox,
    ) -> Vec<TerrainTile> {
        let mut tiles = Vec::new();
        let count = tiler
            .render_tiles(provider, bbox, |t| {
                tiles.push(t);
                Ok(())
            })
            .await
            .expect("render");
        assert_eq!(count, tiles.len());
        tiles
    }

    fn mean_luma(png: &[u8]) -> f64 {
        let img = image::load_from_memory(png).expect("png decodes").into_luma8();
        assert_eq!(img.dimensions(), (TILE_SIZE, TILE_SIZE));
        let sum: u64 = img.pixels().map(|p| p.0[0] as u64).sum();
        sum as f64 / (TILE_SIZE as u64 * TILE_SIZE as u64) as f64
    }

    fn luma_alpha(png: &[u8]) -> image::GrayAlphaImage {
        let img = image::load_from_memory(png).expect("png decodes").into_luma_alpha8();
        assert_eq!(img.dimensions(), (TILE_SIZE, TILE_SIZE));
        img
    }

    /// Pixel coordinates of `(lat, lon)` within slippy tile `(z, x, y)`.
    fn tile_pixel(z: u8, x: u32, y: u32, lat: f64, lon: f64) -> (u32, u32) {
        use super::super::tile_math::{lat_to_y_norm, lon_to_x_norm};
        let n = (1u64 << z) as f64;
        let px = ((lon_to_x_norm(lon) * n - x as f64) * TILE_SIZE as f64).floor();
        let py = ((lat_to_y_norm(lat) * n - y as f64) * TILE_SIZE as f64).floor();
        assert!(
            (0.0..TILE_SIZE as f64).contains(&px) && (0.0..TILE_SIZE as f64).contains(&py),
            "({lat}, {lon}) not inside tile {z}/{x}/{y}"
        );
        (px as u32, py as u32)
    }

    /// The rendered tile that contains `(lat, lon)`.
    fn tile_containing(tiles: &[TerrainTile], z: u8, lat: f64, lon: f64) -> &TerrainTile {
        use super::super::tile_math::{lat_to_y_norm, lon_to_x_norm};
        let n = (1u64 << z) as f64;
        let x = (lon_to_x_norm(lon) * n).floor() as u32;
        let y = (lat_to_y_norm(lat) * n).floor() as u32;
        tiles
            .iter()
            .find(|t| t.z == z && t.x == x && t.y == y)
            .unwrap_or_else(|| panic!("no rendered tile {z}/{x}/{y}"))
    }

    #[test]
    fn new_normalizes_reversed_range() {
        let t = HillshadeTiler::new(11, 5);
        assert_eq!((t.min_zoom, t.max_zoom), (5, 11));
    }

    #[test]
    fn count_tiles_sums_zoom_levels() {
        // From tile_math tests: this bbox is 2 tiles at z8.
        let b = bbox(10.0, 50.0, 10.9, 50.9);
        assert_eq!(HillshadeTiler::new(8, 8).count_tiles(b), 2);
        let z9 = TileRange::covering(9, &b).count();
        assert_eq!(HillshadeTiler::new(8, 9).count_tiles(b), 2 + z9);
    }

    #[test]
    fn downsample_factor_ladder() {
        // ~49 m/px at z11/51°N -> full resolution; each zoom-out doubles.
        assert_eq!(downsample_factor(11, 51.0), 1);
        assert_eq!(downsample_factor(10, 51.0), 2);
        assert_eq!(downsample_factor(8, 51.0), 8);
        assert_eq!(downsample_factor(5, 51.0), 64);
        assert_eq!(downsample_factor(0, 51.0), 64); // clamped
    }

    #[test]
    fn dem_ids_include_neighbours_at_tile_edges() {
        // A z9 tile straddling the 10°E meridian needs both DEM columns.
        let r = TileRange::covering(9, &bbox(9.99, 50.5, 10.01, 50.5));
        let ids = dem_ids_for_tile(9, r.x_min, r.y_min);
        assert!(ids.len() >= 2, "expected neighbour DEM tiles, got {ids:?}");
    }

    #[tokio::test]
    async fn renders_expected_tile_count_with_valid_pngs() {
        let provider = SyntheticDem::new(nw_facing_plane, 120, 120);
        let tiler = HillshadeTiler::new(9, 9);
        let b = bbox(10.2, 50.2, 10.8, 50.8);

        let tiles = render_all(&tiler, &provider, b).await;
        assert_eq!(tiles.len(), tiler.count_tiles(b));
        for tile in &tiles {
            assert_eq!(tile.z, 9);
            // PNG round-trip: decodes back to a 256x256 grayscale image.
            mean_luma(&tile.png);
        }
    }

    #[tokio::test]
    async fn sun_facing_terrain_renders_brighter_than_sea() {
        // Wide enough that the first z9 tile is fully DEM-covered (no
        // sea-level falloff outside the bbox skewing its mean).
        let b = bbox(9.5, 49.5, 11.5, 51.5);
        let tiler = HillshadeTiler::new(9, 9);

        let plane_tiles =
            render_all(&tiler, &SyntheticDem::new(nw_facing_plane, 120, 120), b).await;
        let sea_tiles = render_all(&tiler, &SyntheticDem::new(sea, 120, 120), b).await;

        let plane_mean = mean_luma(&plane_tiles[0].png);
        let sea_mean = mean_luma(&sea_tiles[0].png);
        assert!((sea_mean - 180.0).abs() < 2.0, "flat sea should be ~180, got {sea_mean}");
        assert!(
            plane_mean > sea_mean + 15.0,
            "sun-facing plane {plane_mean} should out-shine sea {sea_mean}"
        );
    }

    #[tokio::test]
    async fn progress_reports_monotonically_to_total() {
        let provider = SyntheticDem::new(sea, 60, 60);
        let tiler = HillshadeTiler::new(8, 9);
        let b = bbox(10.0, 50.0, 10.9, 50.9);

        let mut reports = Vec::new();
        let rendered = tiler
            .render_tiles_with_progress(
                &provider,
                b,
                |_| Ok(()),
                |done, total| reports.push((done, total)),
            )
            .await
            .expect("render");

        let total = tiler.count_tiles(b);
        assert_eq!(rendered, total);
        assert_eq!(reports.len(), total);
        assert_eq!(reports.last(), Some(&(total, total)));
        assert!(reports.windows(2).all(|w| w[0].0 + 1 == w[1].0));
    }

    /// The smoke-ingest bug scenario: a small bbox at a coarse zoom. The
    /// z5 tile covers a huge area but only the bbox's DEM square has data —
    /// everything else must be transparent, not a flat fake tone.
    #[tokio::test]
    async fn no_data_beyond_coverage_renders_transparent() {
        let b = bbox(10.2, 50.2, 10.8, 50.8);
        let tiler = HillshadeTiler::new(5, 5);
        let tiles = render_all(&tiler, &SyntheticDem::new(nw_facing_plane, 120, 120), b).await;

        let tile = tile_containing(&tiles, 5, 50.5, 10.5);
        let img = luma_alpha(&tile.png);

        // Inside the covered DEM square (50..51°N, 10..11°E): opaque data.
        let (px, py) = tile_pixel(5, tile.x, tile.y, 50.5, 10.5);
        assert_eq!(img.get_pixel(px, py).0[1], 255, "covered pixel must be opaque");

        // The tile corners are hundreds of km outside the coverage.
        for (cx, cy) in [(0, 0), (255, 0), (0, 255), (255, 255)] {
            let [luma, alpha] = img.get_pixel(cx, cy).0;
            assert_eq!(alpha, 0, "out-of-coverage pixel ({cx},{cy}) must be transparent");
            assert_eq!(luma, NO_DATA_LUMA, "transparent pixels carry the flat shade");
        }

        // Sanity: the covered square is a small part of an 11.25° z5 tile.
        let opaque = img.pixels().filter(|p| p.0[1] == 255).count();
        let total = (TILE_SIZE * TILE_SIZE) as usize;
        assert!(opaque > 0 && opaque < total / 4, "opaque {opaque} of {total}");
    }

    /// Plateau at 800 m with a no-data hole (e.g. a DEM void): the hole
    /// must come out transparent, the surrounding data opaque.
    fn holed_plateau(lat: f64, lon: f64) -> f64 {
        if (10.4..10.6).contains(&lon) && (50.4..50.6).contains(&lat) {
            f64::NAN
        } else {
            800.0
        }
    }

    #[tokio::test]
    async fn dem_hole_renders_as_transparent_hole() {
        let b = bbox(10.35, 50.35, 10.65, 50.65);
        let tiler = HillshadeTiler::new(9, 9);
        let tiles = render_all(&tiler, &SyntheticDem::new(holed_plateau, 120, 120), b).await;

        let tile = tile_containing(&tiles, 9, 50.5, 10.5);
        let img = luma_alpha(&tile.png);

        let (hx, hy) = tile_pixel(9, tile.x, tile.y, 50.5, 10.5);
        assert_eq!(img.get_pixel(hx, hy).0[1], 0, "hole center must be transparent");

        // Same tile, west of the hole, still inside the covered DEM square.
        let (dx, dy) = tile_pixel(9, tile.x, tile.y, 50.5, 10.2);
        let [luma, alpha] = img.get_pixel(dx, dy).0;
        assert_eq!(alpha, 255, "plateau pixel must be opaque");
        // Flat plateau shades at the flat-terrain value.
        assert!((luma as i32 - 180).abs() <= 1, "plateau luma {luma}");
    }

    /// Tiles fully inside DEM coverage stay fully opaque (the pre-fix
    /// behavior for real data must not change).
    #[tokio::test]
    async fn tiles_inside_coverage_are_fully_opaque() {
        let b = bbox(9.5, 49.5, 11.5, 51.5);
        let tiler = HillshadeTiler::new(9, 9);
        let tiles = render_all(&tiler, &SyntheticDem::new(nw_facing_plane, 120, 120), b).await;

        let tile = tile_containing(&tiles, 9, 50.5, 10.5);
        let img = luma_alpha(&tile.png);
        assert!(img.pixels().all(|p| p.0[1] == 255), "interior tile must be fully opaque");
    }

    #[tokio::test]
    async fn sink_error_aborts_the_run() {
        let provider = SyntheticDem::new(sea, 60, 60);
        let tiler = HillshadeTiler::new(8, 8);
        let b = bbox(10.0, 50.0, 10.9, 50.9);

        let result = tiler
            .render_tiles(&provider, b, |_| {
                Err(Error::provider("test", "sink rejected the tile"))
            })
            .await;
        assert!(result.is_err());
    }
}
