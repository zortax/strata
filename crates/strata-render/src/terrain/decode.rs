//! Worker-side PNG → grayscale+alpha decoding for hillshade tiles.

/// Terrain tile decode failure (logged on the worker, the tile is then
/// negative-cached as missing).
#[derive(Debug, thiserror::Error)]
pub enum TerrainDecodeError {
    #[error("terrain tile PNG decode failed: {0}")]
    Png(#[from] image::ImageError),
    #[error("terrain tile decoded to zero size")]
    Empty,
}

/// A decoded hillshade tile: 8-bit grayscale+alpha, row-major, two bytes
/// per pixel `[luma, alpha]` (uploaded as `Rg8Unorm`). Alpha is DEM
/// coverage: 0 where the source had no data, so the shader draws nothing
/// there instead of a flat tinted block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedTile {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl DecodedTile {
    /// True when every pixel is fully transparent — the layer then marks
    /// the tile ready without uploading a texture or drawing anything.
    pub fn fully_transparent(&self) -> bool {
        self.pixels.chunks_exact(2).all(|px| px[1] == 0)
    }
}

/// Decode PNG bytes to 8-bit grayscale+alpha. Other PNG color types are
/// converted (luma + alpha); plain grayscale tiles from older ingests come
/// out fully opaque.
pub fn decode_terrain_png(bytes: &[u8]) -> Result<DecodedTile, TerrainDecodeError> {
    let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)?;
    let luma_alpha = image.into_luma_alpha8();
    let (width, height) = luma_alpha.dimensions();
    if width == 0 || height == 0 {
        return Err(TerrainDecodeError::Empty);
    }
    Ok(DecodedTile {
        width,
        height,
        pixels: luma_alpha.into_raw(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use image::{DynamicImage, ImageFormat};
    use std::io::Cursor;

    fn encode_png(image: DynamicImage) -> Vec<u8> {
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .expect("encode PNG");
        bytes
    }

    #[test]
    fn decodes_gray_alpha_png_to_luma_alpha_pairs() {
        let source = image::GrayAlphaImage::from_fn(8, 4, |x, y| {
            image::LumaA([(x * 30 + y * 2) as u8, if x < 4 { 255 } else { 0 }])
        });
        let bytes = encode_png(DynamicImage::ImageLumaA8(source.clone()));

        let tile = decode_terrain_png(&bytes).expect("decode");
        assert_eq!(tile.width, 8);
        assert_eq!(tile.height, 4);
        assert_eq!(tile.pixels, source.into_raw());
        assert!(!tile.fully_transparent());
    }

    #[test]
    fn plain_grayscale_png_decodes_fully_opaque() {
        // Tiles from older ingests have no alpha channel: full coverage.
        let source = image::GrayImage::from_fn(8, 4, |x, y| image::Luma([(x * 30 + y * 2) as u8]));
        let bytes = encode_png(DynamicImage::ImageLuma8(source.clone()));

        let tile = decode_terrain_png(&bytes).expect("decode");
        assert_eq!((tile.width, tile.height), (8, 4));
        let lumas: Vec<u8> = tile.pixels.chunks_exact(2).map(|px| px[0]).collect();
        assert_eq!(lumas, source.into_raw());
        assert!(tile.pixels.chunks_exact(2).all(|px| px[1] == 255));
        assert!(!tile.fully_transparent());
    }

    #[test]
    fn converts_rgb_png_to_luma() {
        // A pure-gray RGB image must survive the luma conversion exactly.
        let source = image::RgbImage::from_fn(4, 4, |_, _| image::Rgb([100, 100, 100]));
        let bytes = encode_png(DynamicImage::ImageRgb8(source));

        let tile = decode_terrain_png(&bytes).expect("decode");
        assert_eq!((tile.width, tile.height), (4, 4));
        assert!(tile.pixels.chunks_exact(2).all(|px| px == [100, 255]));
    }

    #[test]
    fn all_transparent_tile_is_detected() {
        let source = image::GrayAlphaImage::from_fn(4, 4, |_, _| image::LumaA([180, 0]));
        let bytes = encode_png(DynamicImage::ImageLumaA8(source));
        let tile = decode_terrain_png(&bytes).expect("decode");
        assert!(tile.fully_transparent());
    }

    #[test]
    fn rejects_garbage_bytes() {
        assert!(matches!(
            decode_terrain_png(b"definitely not a png"),
            Err(TerrainDecodeError::Png(_))
        ));
    }
}
