//! GLO-30 GeoTIFF (COG) decoding with the pure-Rust `tiff` crate.
//!
//! Format facts, verified 2026-06-10 against the live object
//! `Copernicus_DSM_COG_10_N50_00_E010_00_DEM.tif` from
//! `https://copernicus-dem-30m.s3.amazonaws.com` (decoded with tiff 0.11.3):
//!
//! - IFD 0 is the full-resolution raster (overview IFDs 2×/4×/8× follow);
//!   `tiff::decoder::Decoder::new` already positions there.
//! - Gray 32-bit float samples, *tiled* layout, DEFLATE (compression 8)
//!   with the floating-point predictor (predictor 3) — all handled by
//!   `Decoder::read_image`.
//! - Raster sizes vary by latitude band (constant ~30 m ground spacing):
//!   3600×3600 below 50°N, 2400×3600 for 50–60°N.
//! - Georeferencing: `GTRasterTypeGeoKey = 2` (PixelIsPoint) and
//!   `ModelTiepoint` maps raster (0,0) exactly onto the tile's north-west
//!   integer corner. Sample `(col, row)` therefore sits at
//!   `lon = lon_sw + col/width`, `lat = (lat_sw + 1) − row/height`.

use std::io::{Read, Seek};

use tiff::decoder::{Decoder, DecodingResult, Limits};
use tiff::tags::Tag;

use crate::providers::{DemTile, DemTileId};

use super::CopernicusError;

/// Values at or below this are treated as void/no-data and mapped to NaN
/// (the [`DemTile`] no-data marker, rendered transparent by the hillshade
/// tiler). Real elevations bottom out around −430 m.
const VOID_THRESHOLD_M: f32 = -1_000.0;

/// Decodes the full-resolution IFD of a GLO-30 COG into a [`DemTile`].
pub(crate) fn decode_dem<R: Read + Seek>(
    id: DemTileId,
    reader: R,
) -> Result<DemTile, CopernicusError> {
    let tiff = |source| CopernicusError::Tiff { tile: id, source };

    // Default limits allow a 256 MiB decode buffer; the largest GLO-30
    // raster (3600×3600 f32) needs ~52 MiB.
    let mut decoder = Decoder::new(reader).map_err(tiff)?.with_limits(Limits::default());
    let (width, height) = decoder.dimensions().map_err(tiff)?;
    check_georeferencing(&mut decoder, id);

    let mut elevations_m: Vec<f32> = match decoder.read_image().map_err(tiff)? {
        DecodingResult::F32(v) => v,
        DecodingResult::F64(v) => v.into_iter().map(|s| s as f32).collect(),
        DecodingResult::I16(v) => v.into_iter().map(f32::from).collect(),
        DecodingResult::U16(v) => v.into_iter().map(f32::from).collect(),
        DecodingResult::I32(v) => v.into_iter().map(|s| s as f32).collect(),
        other => {
            return Err(CopernicusError::UnsupportedFormat {
                tile: id,
                found: decoding_variant_name(&other),
            });
        }
    };

    let expected = width as usize * height as usize;
    if elevations_m.len() != expected {
        return Err(CopernicusError::SampleCountMismatch {
            tile: id,
            got: elevations_m.len(),
            width,
            height,
        });
    }

    for v in &mut elevations_m {
        if !v.is_finite() || *v <= VOID_THRESHOLD_M {
            *v = f32::NAN;
        }
    }

    Ok(DemTile { id, width, height, elevations_m })
}

/// Warn-only sanity check that the tiepoint matches the point-registered
/// NW-corner grid this module assumes (see module docs). A mismatch would
/// shift sampling by up to half a pixel — visible only as a tiny offset,
/// so it is not worth failing the ingest over.
fn check_georeferencing<R: Read + Seek>(decoder: &mut Decoder<R>, id: DemTileId) {
    let Ok(Some(value)) = decoder.find_tag(Tag::ModelTiepointTag) else {
        return;
    };
    let Ok(tiepoint) = value.into_f64_vec() else {
        return;
    };
    if tiepoint.len() < 6 {
        return;
    }
    let (raster_x, raster_y, geo_lon, geo_lat) =
        (tiepoint[0], tiepoint[1], tiepoint[3], tiepoint[4]);
    let expected_lon = id.lon_sw as f64;
    let expected_lat = id.lat_sw as f64 + 1.0;
    if raster_x.abs() > 1e-9
        || raster_y.abs() > 1e-9
        || (geo_lon - expected_lon).abs() > 1e-6
        || (geo_lat - expected_lat).abs() > 1e-6
    {
        tracing::warn!(
            tile = %id,
            ?tiepoint,
            expected_lon,
            expected_lat,
            "GLO-30 tiepoint deviates from the assumed NW-corner point grid"
        );
    }
}

fn decoding_variant_name(result: &DecodingResult) -> &'static str {
    match result {
        DecodingResult::U8(_) => "u8",
        DecodingResult::U16(_) => "u16",
        DecodingResult::U32(_) => "u32",
        DecodingResult::U64(_) => "u64",
        DecodingResult::F16(_) => "f16",
        DecodingResult::F32(_) => "f32",
        DecodingResult::F64(_) => "f64",
        DecodingResult::I8(_) => "i8",
        DecodingResult::I16(_) => "i16",
        DecodingResult::I32(_) => "i32",
        DecodingResult::I64(_) => "i64",
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use tiff::encoder::TiffEncoder;
    use tiff::encoder::colortype::{Gray32Float, RGB8};

    use super::*;

    fn tile_id() -> DemTileId {
        DemTileId { lat_sw: 50, lon_sw: 10 }
    }

    fn encode_gray_f32(width: u32, height: u32, data: &[f32]) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        let mut encoder = TiffEncoder::new(&mut buf).expect("tiff encoder");
        encoder
            .write_image::<Gray32Float>(width, height, data)
            .expect("write f32 image");
        buf.into_inner()
    }

    #[test]
    fn round_trips_a_float_ramp() {
        let (w, h) = (60u32, 40u32);
        let data: Vec<f32> = (0..w * h).map(|i| i as f32 * 0.25).collect();
        let bytes = encode_gray_f32(w, h, &data);

        let dem = decode_dem(tile_id(), Cursor::new(bytes)).expect("decode");
        assert_eq!(dem.id, tile_id());
        assert_eq!((dem.width, dem.height), (w, h));
        assert_eq!(dem.elevations_m, data);
    }

    #[test]
    fn normalizes_voids_to_no_data() {
        let data = vec![100.0_f32, f32::INFINITY, -32767.0, 8.5];
        let bytes = encode_gray_f32(2, 2, &data);
        let dem = decode_dem(tile_id(), Cursor::new(bytes)).expect("decode");
        assert_eq!(dem.elevations_m[0], 100.0);
        assert!(dem.elevations_m[1].is_nan(), "infinity becomes no-data");
        assert!(dem.elevations_m[2].is_nan(), "void value becomes no-data");
        assert_eq!(dem.elevations_m[3], 8.5);
    }

    #[test]
    fn rejects_unsupported_color_types() {
        let mut buf = Cursor::new(Vec::new());
        let mut encoder = TiffEncoder::new(&mut buf).expect("tiff encoder");
        encoder
            .write_image::<RGB8>(2, 2, &[0u8; 12])
            .expect("write rgb image");
        let err = decode_dem(tile_id(), Cursor::new(buf.into_inner()));
        assert!(
            matches!(err, Err(CopernicusError::UnsupportedFormat { .. })),
            "got {err:?}"
        );
    }
}
