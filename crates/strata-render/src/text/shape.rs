//! cosmic-text shaping and swash rasterization, with an LRU shape cache.
//!
//! Shaping runs in logical px; rasterization runs at physical px
//! (`logical × scale_factor`). Glyph pen positions are rounded to integer
//! physical px so every glyph rasterizes at subpixel bin zero — one atlas
//! entry per `(font, glyph, quantized physical size)`.

use cosmic_text::fontdb;
use cosmic_text::{
    Attrs, Buffer, CacheKey, Family, FontSystem, Metrics, Shaping, SwashCache, SwashContent, Wrap,
};
use glam::{IVec2, Vec2};
use lru::LruCache;

use std::num::NonZeroUsize;
use std::sync::Arc;

/// Preferred label fonts; the first installed family wins. Falls back to the
/// system generic sans-serif when neither is available.
const PREFERRED_FAMILIES: [&str; 2] = ["DejaVu Sans", "Noto Sans"];

/// Shaped labels kept hot; evicted labels are simply re-shaped on demand.
const SHAPE_CACHE_CAPACITY: NonZeroUsize = match NonZeroUsize::new(4096) {
    Some(n) => n,
    None => unreachable!(),
};

const LINE_HEIGHT_FACTOR: f32 = 1.2;

/// Font sizes are quantized to this step (logical for the shape cache,
/// physical for the atlas key) so float jitter cannot multiply cache entries.
const SIZE_QUANTUM: f32 = 0.25;

/// One glyph of a shaped label.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ShapedGlyph {
    /// Rasterization + atlas key (subpixel bins are always zero).
    pub key: CacheKey,
    /// Integer pen position in physical px relative to the label's top-left.
    pub pen_px: IVec2,
}

/// A shaped label: glyphs plus the laid-out box size in logical px.
#[derive(Debug, Clone)]
pub(crate) struct ShapedLabel {
    pub glyphs: Vec<ShapedGlyph>,
    pub size: Vec2,
}

/// The rasterized coverage bitmap of one glyph.
#[derive(Debug, Clone)]
pub(crate) struct RasterGlyph {
    /// Bitmap offset from the pen position, physical px (swash placement;
    /// `top` is distance *above* the pen, i.e. subtract from y).
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    /// One coverage byte per pixel, row-major.
    pub coverage: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ShapeKey {
    /// Shared with the [`crate::text::LabelRequest`] so cache hits are
    /// allocation-free (a refcount bump instead of a `String` copy).
    text: Arc<str>,
    /// Logical font size in [`SIZE_QUANTUM`] steps.
    size_q: u32,
    /// Rasterization scale factor bits (pen rounding depends on it).
    scale_bits: u32,
}

/// Owns the font database, the swash scaler and the shape cache.
pub(crate) struct Shaper {
    fonts: FontSystem,
    swash: SwashCache,
    scratch: Buffer,
    /// Resolved preferred family, if installed.
    family: Option<String>,
    cache: LruCache<ShapeKey, Arc<ShapedLabel>>,
}

impl Shaper {
    /// Loads system fonts (fontdb defaults). One-time cost at renderer init.
    pub fn new() -> Self {
        let mut fonts = FontSystem::new();
        let family = PREFERRED_FAMILIES
            .iter()
            .find(|name| {
                fonts
                    .db()
                    .query(&fontdb::Query {
                        families: &[fontdb::Family::Name(name)],
                        ..fontdb::Query::default()
                    })
                    .is_some()
            })
            .map(|name| (*name).to_owned());
        tracing::debug!(
            family = family.as_deref().unwrap_or("<generic sans-serif>"),
            faces = fonts.db().len(),
            "text shaper ready"
        );
        let scratch = Buffer::new(&mut fonts, Metrics::new(14.0, 14.0 * LINE_HEIGHT_FACTOR));
        Self {
            fonts,
            swash: SwashCache::new(),
            scratch,
            family,
            cache: LruCache::new(SHAPE_CACHE_CAPACITY),
        }
    }

    /// False in environments with no system fonts at all (shaping then
    /// produces zero glyphs for any text). Tests skip themselves on this.
    #[cfg(test)]
    pub fn has_fonts(&self) -> bool {
        !self.fonts.db().is_empty()
    }

    /// Shape `text` at `size_px` logical px for rasterization at
    /// `scale_factor` physical px per logical px. Cached; a hit costs no
    /// allocation (the key shares the caller's `Arc<str>`).
    pub fn shape(&mut self, text: &Arc<str>, size_px: f32, scale_factor: f32) -> Arc<ShapedLabel> {
        let size_q = (size_px / SIZE_QUANTUM).round().max(1.0) as u32;
        let key = ShapeKey {
            text: Arc::clone(text),
            size_q,
            scale_bits: scale_factor.to_bits(),
        };
        if let Some(hit) = self.cache.get(&key) {
            return Arc::clone(hit);
        }
        let shaped = Arc::new(self.shape_uncached(
            text,
            size_q as f32 * SIZE_QUANTUM,
            scale_factor,
        ));
        self.cache.put(key, Arc::clone(&shaped));
        shaped
    }

    fn shape_uncached(&mut self, text: &str, font_size: f32, scale: f32) -> ShapedLabel {
        let metrics = Metrics::new(font_size, (font_size * LINE_HEIGHT_FACTOR).max(1.0));
        self.scratch
            .set_metrics_and_size(&mut self.fonts, metrics, None, None);
        self.scratch.set_wrap(&mut self.fonts, Wrap::None);
        let attrs = match &self.family {
            Some(name) => Attrs::new().family(Family::Name(name.as_str())),
            None => Attrs::new().family(Family::SansSerif),
        };
        self.scratch
            .set_text(&mut self.fonts, text, &attrs, Shaping::Advanced);

        let mut glyphs = Vec::new();
        let mut width = 0.0f32;
        let mut height = 0.0f32;
        for run in self.scratch.layout_runs() {
            width = width.max(run.line_w);
            height = height.max(run.line_top + run.line_height);
            for glyph in run.glyphs {
                let x = (glyph.x + glyph.font_size * glyph.x_offset) * scale;
                let y = (run.line_y + glyph.y - glyph.font_size * glyph.y_offset) * scale;
                // Rounding the pen forces the cache key's subpixel bins to
                // zero, collapsing the atlas key to (font, glyph, size).
                let (key, pen_x, pen_y) = CacheKey::new(
                    glyph.font_id,
                    glyph.glyph_id,
                    quantize_size(glyph.font_size * scale),
                    (x.round(), y.round()),
                    glyph.cache_key_flags,
                );
                glyphs.push(ShapedGlyph {
                    key,
                    pen_px: IVec2::new(pen_x, pen_y),
                });
            }
        }
        ShapedLabel {
            glyphs,
            size: Vec2::new(width, height),
        }
    }

    /// Rasterize a glyph to an 8-bit coverage bitmap. `None` for glyphs with
    /// no pixels (spaces) or fonts swash cannot scale.
    pub fn rasterize(&mut self, key: CacheKey) -> Option<RasterGlyph> {
        let image = self.swash.get_image_uncached(&mut self.fonts, key)?;
        let width = image.placement.width;
        let height = image.placement.height;
        let pixels = width as usize * height as usize;
        if pixels == 0 {
            return None;
        }
        let coverage = match image.content {
            SwashContent::Mask => {
                if image.data.len() < pixels {
                    tracing::warn!(?key, "swash mask shorter than placement, skipping glyph");
                    return None;
                }
                let mut data = image.data;
                data.truncate(pixels);
                data
            }
            // Color emoji / subpixel masks are out of scope for map labels:
            // collapse to the alpha channel.
            SwashContent::Color | SwashContent::SubpixelMask => {
                if image.data.len() < pixels * 4 {
                    tracing::warn!(?key, "swash rgba shorter than placement, skipping glyph");
                    return None;
                }
                image.data.chunks_exact(4).take(pixels).map(|px| px[3]).collect()
            }
        };
        Some(RasterGlyph {
            left: image.placement.left,
            top: image.placement.top,
            width,
            height,
            coverage,
        })
    }
}

fn quantize_size(size: f32) -> f32 {
    (size / SIZE_QUANTUM).round().max(1.0) * SIZE_QUANTUM
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns `None` (and logs) when the environment has no fonts, so CI
    /// containers without fontconfig don't fail.
    fn shaper_or_skip() -> Option<Shaper> {
        let shaper = Shaper::new();
        if shaper.has_fonts() {
            Some(shaper)
        } else {
            eprintln!("skipping: no system fonts available in this environment");
            None
        }
    }

    fn text(s: &str) -> Arc<str> {
        Arc::from(s)
    }

    #[test]
    fn shaping_a_realistic_label_yields_glyphs() {
        let Some(mut shaper) = shaper_or_skip() else {
            return;
        };
        let shaped = shaper.shape(&text("EDDF 2500 ft MSL"), 14.0, 1.0);
        assert!(!shaped.glyphs.is_empty(), "expected at least one glyph");
        assert!(shaped.size.x > 0.0 && shaped.size.y > 0.0);
        // Subpixel bins must be zero so the atlas key is position-free.
        for glyph in &shaped.glyphs {
            assert_eq!(glyph.key.x_bin, cosmic_text::SubpixelBin::Zero);
            assert_eq!(glyph.key.y_bin, cosmic_text::SubpixelBin::Zero);
        }
    }

    #[test]
    fn shape_cache_returns_the_same_allocation() {
        let Some(mut shaper) = shaper_or_skip() else {
            return;
        };
        let a = shaper.shape(&text("EDDF"), 14.0, 1.0);
        let b = shaper.shape(&text("EDDF"), 14.0, 1.0);
        assert!(Arc::ptr_eq(&a, &b), "expected a shape-cache hit");
        let c = shaper.shape(&text("EDDF"), 16.0, 1.0);
        assert!(!Arc::ptr_eq(&a, &c), "different size must shape anew");
    }

    #[test]
    fn rasterize_produces_coverage_for_a_visible_glyph() {
        let Some(mut shaper) = shaper_or_skip() else {
            return;
        };
        let shaped = shaper.shape(&text("E"), 24.0, 1.0);
        let Some(glyph) = shaped.glyphs.first().copied() else {
            eprintln!("skipping: no glyph shaped");
            return;
        };
        let raster = shaper.rasterize(glyph.key).expect("glyph must rasterize");
        assert!(raster.width > 0 && raster.height > 0);
        assert_eq!(
            raster.coverage.len(),
            raster.width as usize * raster.height as usize
        );
        assert!(raster.coverage.iter().any(|&c| c > 0), "all-zero coverage");
    }
}
