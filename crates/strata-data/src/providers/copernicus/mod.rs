//! Copernicus GLO-30 DEM from AWS Open Data (no key required) plus the
//! hillshade tile pipeline (Horn algorithm, slope-darkened grayscale PNGs).
//!
//! Object layout, verified live on 2026-06-10 (HTTP 200, `image/tiff`,
//! ~30 MB):
//!
//! ```text
//! https://copernicus-dem-30m.s3.amazonaws.com/
//!   Copernicus_DSM_COG_10_N50_00_E010_00_DEM/
//!     Copernicus_DSM_COG_10_N50_00_E010_00_DEM.tif
//! ```
//!
//! Names use the integer-degree south-west corner, `N/S` latitude
//! zero-padded to two digits and `E/W` longitude to three (Germany needs
//! N47–N55, E005–E015). Tiles that are entirely sea are **not published**
//! (verified: `N55_00_E006_00` → 404, coastal `N54_00_E008_00` → 200);
//! [`CopernicusDem`] substitutes an all-NaN no-data tile for those, which
//! the hillshade tiler renders fully transparent (sea *within* published
//! coastal tiles is real 0 m data and shades normally).
//!
//! GeoTIFF specifics live in [`geotiff`], slippy-tile math in
//! [`tile_math`], the shading math in [`hillshade`].

mod dem_cache;
mod geotiff;
mod hillshade;
#[cfg(test)]
mod testutil;
mod tile_math;
mod tiler;

pub use dem_cache::{DemCache, ElevationSampler};
pub use tiler::{HillshadeTiler, TerrainTile};

use std::io::Cursor;
use std::path::PathBuf;

use async_trait::async_trait;

use crate::Error;
use crate::domain::BoundingBox;
use crate::providers::{DemTile, DemTileId, TerrainProvider};

/// Errors internal to this provider; converted to [`Error::Provider`]
/// (provider name `"copernicus"`) at the crate boundary.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CopernicusError {
    #[error("decoding DEM tile {tile}: {source}")]
    Tiff {
        tile: DemTileId,
        #[source]
        source: tiff::TiffError,
    },
    #[error("DEM tile {tile}: unsupported sample format {found}")]
    UnsupportedFormat {
        tile: DemTileId,
        found: &'static str,
    },
    #[error("DEM tile {tile}: {got} samples do not match {width}x{height}")]
    SampleCountMismatch {
        tile: DemTileId,
        got: usize,
        width: u32,
        height: u32,
    },
    #[error("encoding hillshade tile {z}/{x}/{y}: {source}")]
    PngEncode {
        z: u8,
        x: u32,
        y: u32,
        #[source]
        source: image::ImageError,
    },
    #[error("hillshade render thread panicked")]
    RenderPanicked,
    #[error("internal: {0}")]
    Internal(&'static str),
}

impl From<CopernicusError> for Error {
    fn from(err: CopernicusError) -> Self {
        Error::provider("copernicus", err)
    }
}

/// The 1°×1° DEM tiles whose extent intersects `bbox` (pure; usable
/// without a client, e.g. for download planning).
pub fn dem_tiles_covering(bbox: BoundingBox) -> Vec<DemTileId> {
    dem_tiles_in(bbox.west(), bbox.south(), bbox.east(), bbox.north())
}

/// Like [`dem_tiles_covering`] for raw degree edges, clamping to valid
/// tile corners (latitudes −90..=89, longitudes −180..=179).
pub(crate) fn dem_tiles_in(west: f64, south: f64, east: f64, north: f64) -> Vec<DemTileId> {
    let corner = |v: f64, lo: i32, hi: i32| (v.floor() as i32).clamp(lo, hi) as i16;
    let lat0 = corner(south, -90, 89);
    let lat1 = corner(north, -90, 89);
    let lon0 = corner(west, -180, 179);
    let lon1 = corner(east, -180, 179);
    (lat0..=lat1)
        .flat_map(|lat_sw| (lon0..=lon1).map(move |lon_sw| DemTileId { lat_sw, lon_sw }))
        .collect()
}

/// AWS Open Data dataset name for a tile, e.g.
/// `Copernicus_DSM_COG_10_N50_00_E010_00_DEM`.
fn dataset_name(tile: DemTileId) -> String {
    let (ns, lat) = if tile.lat_sw >= 0 {
        ('N', tile.lat_sw)
    } else {
        ('S', -tile.lat_sw)
    };
    let (ew, lon) = if tile.lon_sw >= 0 {
        ('E', tile.lon_sw)
    } else {
        ('W', -tile.lon_sw)
    };
    format!("Copernicus_DSM_COG_10_{ns}{lat:02}_00_{ew}{lon:03}_00_DEM")
}

/// Substitute for tiles the bucket does not publish (entirely sea): a small
/// uniform no-data raster (NaN, see the `DemTile` contract). The hillshade
/// tiler renders no-data as transparent pixels instead of inventing a flat
/// sea-level shade. The grid size is irrelevant — sampling a constant grid
/// is constant.
fn no_data_tile(id: DemTileId) -> DemTile {
    const SIZE: u32 = 16;
    DemTile {
        id,
        width: SIZE,
        height: SIZE,
        elevations_m: vec![f32::NAN; (SIZE * SIZE) as usize],
    }
}

pub struct CopernicusDem {
    http: reqwest::Client,
    base_url: String,
    /// Downloaded `.tif` objects are kept here and decoded from disk on
    /// re-fetch; `None` disables the disk cache.
    cache_dir: Option<PathBuf>,
}

impl CopernicusDem {
    pub const DEFAULT_BASE_URL: &'static str = "https://copernicus-dem-30m.s3.amazonaws.com";

    pub fn new() -> Self {
        Self::with_base_url(Self::DEFAULT_BASE_URL)
    }

    /// Override the bucket root (fixture/local-server tests).
    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        // No *total* timeout — tiles are ~30 MB and slow links are
        // legitimate. Connect and read-inactivity timeouts only kill stalled
        // connections, which would otherwise hang the terrain ingest forever
        // (reqwest defaults to no timeout at all). Falling back to the
        // default client only happens when TLS init fails, where
        // `Client::new()` would panic anyway.
        let http = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(15))
            .read_timeout(std::time::Duration::from_secs(60))
            .build()
            .unwrap_or_default();
        Self {
            http,
            base_url,
            // Tiles are immutable ~30 MB objects fetched repeatedly by the
            // tiler; default to a persistent disk cache.
            cache_dir: dirs::cache_dir().map(|d| d.join("strata").join("copernicus-dem")),
        }
    }

    /// Cache downloaded GeoTIFFs in `dir` instead of the default
    /// `~/.cache/strata/copernicus-dem`.
    pub fn with_cache_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cache_dir = Some(dir.into());
        self
    }

    /// Disable the on-disk GeoTIFF cache (every fetch re-downloads).
    pub fn without_disk_cache(mut self) -> Self {
        self.cache_dir = None;
        self
    }

    fn tile_url(&self, tile: DemTileId) -> String {
        let name = dataset_name(tile);
        format!("{}/{name}/{name}.tif", self.base_url)
    }
}

impl Default for CopernicusDem {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl TerrainProvider for CopernicusDem {
    fn tiles_for(&self, bbox: BoundingBox) -> Vec<DemTileId> {
        dem_tiles_covering(bbox)
    }

    async fn fetch_tile(&self, tile: DemTileId) -> Result<DemTile, Error> {
        let cache_path = self
            .cache_dir
            .as_ref()
            .map(|dir| dir.join(format!("{}.tif", dataset_name(tile))));

        if let Some(path) = &cache_path
            && tokio::fs::try_exists(path).await.unwrap_or(false)
        {
            tracing::debug!(%tile, path = %path.display(), "decoding DEM tile from disk cache");
            // Decoding takes ~150 ms; acceptable inline in the ingest context.
            let file = std::fs::File::open(path)?;
            return Ok(decode_dem_logged(tile, std::io::BufReader::new(file))?);
        }

        let url = self.tile_url(tile);
        tracing::info!(%tile, url, "downloading GLO-30 DEM tile");
        let response = self.http.get(&url).send().await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            tracing::warn!(
                %tile,
                url,
                "GLO-30 tile not published (all-sea tiles are omitted); substituting no-data"
            );
            return Ok(no_data_tile(tile));
        }
        let bytes = response.error_for_status()?.bytes().await?;

        if let Some(path) = &cache_path {
            // Best effort — a failed cache write must not fail the fetch.
            let write = async {
                if let Some(parent) = path.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                tokio::fs::write(path, &bytes).await
            };
            if let Err(error) = write.await {
                tracing::warn!(%tile, path = %path.display(), %error, "failed to cache DEM tile");
            }
        }

        Ok(decode_dem_logged(tile, Cursor::new(bytes))?)
    }
}

fn decode_dem_logged<R: std::io::Read + std::io::Seek>(
    tile: DemTileId,
    reader: R,
) -> Result<DemTile, CopernicusError> {
    let decoded = geotiff::decode_dem(tile, reader)?;
    tracing::debug!(%tile, width = decoded.width, height = decoded.height, "decoded DEM tile");
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bbox(w: f64, s: f64, e: f64, n: f64) -> BoundingBox {
        BoundingBox::new(w, s, e, n).expect("valid test bbox")
    }

    #[test]
    fn germany_needs_99_dem_tiles() {
        // Spec region: lat 47..55.2 -> N47..N55 (9), lon 5.5..15.5 ->
        // E005..E015 (11).
        let tiles = dem_tiles_covering(crate::domain::Country::DE.bounding_box());
        assert_eq!(tiles.len(), 99);
        assert!(tiles.contains(&DemTileId {
            lat_sw: 47,
            lon_sw: 5
        }));
        assert!(tiles.contains(&DemTileId {
            lat_sw: 55,
            lon_sw: 15
        }));
    }

    #[test]
    fn bbox_within_one_tile() {
        assert_eq!(
            dem_tiles_covering(bbox(10.2, 50.2, 10.8, 50.8)),
            vec![DemTileId {
                lat_sw: 50,
                lon_sw: 10
            }]
        );
    }

    #[test]
    fn bbox_straddling_a_degree_line_needs_both_tiles() {
        let tiles = dem_tiles_covering(bbox(9.9, 50.5, 10.1, 50.6));
        assert_eq!(
            tiles,
            vec![
                DemTileId {
                    lat_sw: 50,
                    lon_sw: 9
                },
                DemTileId {
                    lat_sw: 50,
                    lon_sw: 10
                },
            ]
        );
    }

    #[test]
    fn dataset_names_match_the_bucket_layout() {
        // Pattern verified against the live bucket (see module docs).
        assert_eq!(
            dataset_name(DemTileId {
                lat_sw: 50,
                lon_sw: 10
            }),
            "Copernicus_DSM_COG_10_N50_00_E010_00_DEM"
        );
        assert_eq!(
            dataset_name(DemTileId {
                lat_sw: 47,
                lon_sw: 5
            }),
            "Copernicus_DSM_COG_10_N47_00_E005_00_DEM"
        );
        assert_eq!(
            dataset_name(DemTileId {
                lat_sw: -3,
                lon_sw: -72
            }),
            "Copernicus_DSM_COG_10_S03_00_W072_00_DEM"
        );
    }

    #[test]
    fn tile_url_pattern() {
        let dem = CopernicusDem::with_base_url("https://example.test/bucket/");
        assert_eq!(
            dem.tile_url(DemTileId {
                lat_sw: 50,
                lon_sw: 10
            }),
            "https://example.test/bucket/Copernicus_DSM_COG_10_N50_00_E010_00_DEM/Copernicus_DSM_COG_10_N50_00_E010_00_DEM.tif"
        );
    }

    #[test]
    fn no_data_tile_is_uniform_nan() {
        let tile = no_data_tile(DemTileId {
            lat_sw: 55,
            lon_sw: 6,
        });
        assert_eq!(tile.elevations_m.len(), (tile.width * tile.height) as usize);
        assert!(tile.elevations_m.iter().all(|v| v.is_nan()));
    }
}
