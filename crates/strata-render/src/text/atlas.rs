//! Glyph atlas: an etagere shelf allocator over an R8Unorm coverage texture.
//!
//! Starts at 1024² and grows once to 2048². Entries are never evicted within
//! a run; growing resets the whole index (callers re-insert what the current
//! frame needs — glyph variety on a map is small, so this happens at most
//! once or twice per session).

use super::shape::RasterGlyph;
use crate::gpu;

use cosmic_text::CacheKey;
use rustc_hash::FxHashMap;

use std::hash::Hash;

pub(crate) const ATLAS_INITIAL_SIZE: u32 = 1024;
pub(crate) const ATLAS_MAX_SIZE: u32 = 2048;

/// Transparent border kept around each glyph so linear sampling never bleeds
/// into a neighbor. The texture is zero-initialized and borders are never
/// written, so they stay transparent.
const PADDING_PX: u32 = 1;

/// Where a glyph lives in the atlas (texel rect, padding excluded) plus its
/// swash placement relative to the pen position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AtlasSlot {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    /// Bitmap offset from the pen, physical px (`top` is *above* the pen).
    pub left: i32,
    pub top: i32,
}

/// The atlas has no room left at its current size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AtlasFull;

/// CPU half of the atlas — allocator plus key→slot map — kept separate from
/// the GPU texture so allocation behavior is unit-testable.
pub(crate) struct AtlasIndex<K> {
    allocator: etagere::AtlasAllocator,
    /// `Some(slot)` = resident; `None` = known to rasterize to nothing.
    slots: FxHashMap<K, Option<AtlasSlot>>,
    size: u32,
    allocation_count: usize,
}

impl<K: Eq + Hash> AtlasIndex<K> {
    pub fn new(size: u32) -> Self {
        Self {
            allocator: etagere::AtlasAllocator::new(etagere::size2(size as i32, size as i32)),
            slots: FxHashMap::default(),
            size,
            allocation_count: 0,
        }
    }

    pub fn size(&self) -> u32 {
        self.size
    }

    /// Outer `None` = key unknown; `Some(None)` = known-empty glyph.
    pub fn lookup(&self, key: &K) -> Option<Option<AtlasSlot>> {
        self.slots.get(key).copied()
    }

    /// Record that `key` rasterizes to no pixels (space, missing glyph).
    pub fn mark_empty(&mut self, key: K) {
        self.slots.insert(key, None);
    }

    /// Slot for `key`, allocating only when the key is new. Returns the slot
    /// and whether it was freshly allocated (fresh slots need an upload).
    pub fn ensure(
        &mut self,
        key: K,
        width: u32,
        height: u32,
        left: i32,
        top: i32,
    ) -> Result<(AtlasSlot, bool), AtlasFull> {
        if let Some(Some(slot)) = self.slots.get(&key) {
            return Ok((*slot, false));
        }
        let padded = etagere::size2(
            (width + 2 * PADDING_PX) as i32,
            (height + 2 * PADDING_PX) as i32,
        );
        let alloc = self.allocator.allocate(padded).ok_or(AtlasFull)?;
        let slot = AtlasSlot {
            x: alloc.rectangle.min.x as u32 + PADDING_PX,
            y: alloc.rectangle.min.y as u32 + PADDING_PX,
            width,
            height,
            left,
            top,
        };
        self.allocation_count += 1;
        self.slots.insert(key, Some(slot));
        Ok((slot, true))
    }

    pub fn allocation_count(&self) -> usize {
        self.allocation_count
    }
}

/// GPU half: the R8Unorm texture, sampler and bind group over [`AtlasIndex`].
pub(crate) struct GlyphAtlas {
    index: AtlasIndex<CacheKey>,
    texture: wgpu::Texture,
    sampler: wgpu::Sampler,
    layout: wgpu::BindGroupLayout,
    bind_group: wgpu::BindGroup,
}

impl GlyphAtlas {
    pub fn new(device: &wgpu::Device) -> Self {
        let layout = gpu::texture_sampler_bind_group_layout(device, "strata glyph atlas layout");
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("strata glyph atlas sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let index = AtlasIndex::new(ATLAS_INITIAL_SIZE);
        let texture = create_texture(device, ATLAS_INITIAL_SIZE);
        let bind_group = create_bind_group(device, &layout, &texture, &sampler);
        Self {
            index,
            texture,
            sampler,
            layout,
            bind_group,
        }
    }

    pub fn layout(&self) -> &wgpu::BindGroupLayout {
        &self.layout
    }

    pub fn bind_group(&self) -> &wgpu::BindGroup {
        &self.bind_group
    }

    pub fn size(&self) -> u32 {
        self.index.size()
    }

    pub fn lookup(&self, key: &CacheKey) -> Option<Option<AtlasSlot>> {
        self.index.lookup(key)
    }

    pub fn mark_empty(&mut self, key: CacheKey) {
        self.index.mark_empty(key);
    }

    /// Allocate (if new) and upload a rasterized glyph.
    pub fn ensure(
        &mut self,
        queue: &wgpu::Queue,
        key: CacheKey,
        raster: &RasterGlyph,
    ) -> Result<AtlasSlot, AtlasFull> {
        let (slot, fresh) =
            self.index
                .ensure(key, raster.width, raster.height, raster.left, raster.top)?;
        if fresh {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: slot.x,
                        y: slot.y,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &raster.coverage,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(raster.width),
                    rows_per_image: Some(raster.height),
                },
                wgpu::Extent3d {
                    width: raster.width,
                    height: raster.height,
                    depth_or_array_layers: 1,
                },
            );
        }
        Ok(slot)
    }

    /// Double the atlas, dropping all entries (the texture starts zeroed and
    /// glyphs are re-uploaded by the caller). False once at [`ATLAS_MAX_SIZE`].
    pub fn grow(&mut self, device: &wgpu::Device) -> bool {
        let size = self.index.size();
        if size >= ATLAS_MAX_SIZE {
            return false;
        }
        let size = (size * 2).min(ATLAS_MAX_SIZE);
        tracing::debug!(
            dropped_glyphs = self.index.allocation_count(),
            new_size = size,
            "growing glyph atlas"
        );
        self.index = AtlasIndex::new(size);
        self.texture = create_texture(device, size);
        self.bind_group = create_bind_group(device, &self.layout, &self.texture, &self.sampler);
        true
    }
}

fn create_texture(device: &wgpu::Device, size: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("strata glyph atlas"),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

fn create_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    texture: &wgpu::Texture,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("strata glyph atlas"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_key_allocates_once() {
        let mut index: AtlasIndex<u32> = AtlasIndex::new(256);
        let (a, fresh_a) = index.ensure(7, 10, 12, 1, 9).expect("fits");
        assert!(fresh_a);
        let (b, fresh_b) = index.ensure(7, 10, 12, 1, 9).expect("cached");
        assert!(!fresh_b);
        assert_eq!(a, b);
        assert_eq!(index.allocation_count(), 1);
    }

    #[test]
    fn different_keys_get_disjoint_slots() {
        let mut index: AtlasIndex<u32> = AtlasIndex::new(256);
        let (a, _) = index.ensure(1, 16, 16, 0, 0).expect("fits");
        let (b, _) = index.ensure(2, 16, 16, 0, 0).expect("fits");
        assert_eq!(index.allocation_count(), 2);
        // Padded rects must not overlap.
        let overlap_x = a.x < b.x + b.width && b.x < a.x + a.width;
        let overlap_y = a.y < b.y + b.height && b.y < a.y + a.height;
        assert!(!(overlap_x && overlap_y), "slots overlap: {a:?} vs {b:?}");
    }

    #[test]
    fn slots_keep_a_transparent_border_and_stay_in_bounds() {
        let mut index: AtlasIndex<u32> = AtlasIndex::new(64);
        let (slot, _) = index.ensure(1, 30, 20, 0, 0).expect("fits");
        assert!(slot.x >= PADDING_PX && slot.y >= PADDING_PX);
        assert!(slot.x + slot.width + PADDING_PX <= 64);
        assert!(slot.y + slot.height + PADDING_PX <= 64);
    }

    #[test]
    fn oversized_request_reports_full() {
        let mut index: AtlasIndex<u32> = AtlasIndex::new(64);
        assert_eq!(index.ensure(1, 200, 200, 0, 0), Err(AtlasFull));
        assert_eq!(index.allocation_count(), 0);
    }

    #[test]
    fn empty_marks_are_remembered() {
        let mut index: AtlasIndex<u32> = AtlasIndex::new(64);
        assert_eq!(index.lookup(&5), None);
        index.mark_empty(5);
        assert_eq!(index.lookup(&5), Some(None));
        assert_eq!(index.allocation_count(), 0);
    }
}
