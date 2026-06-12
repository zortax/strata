//! Terrain hillshade layer: raster PNG tiles from a [`TileSource`], decoded
//! to grayscale+alpha on the worker pool, uploaded as `Rg8Unorm` textures
//! (r = shade, g = DEM-coverage alpha) into an LRU cache and drawn as
//! tinted quads under the basemap (`shaders/raster_tile.wgsl`). No-data
//! regions are transparent; fully transparent tiles are cached as ready
//! but never uploaded or drawn.
//!
//! Coverage is computed at the core display level clamped to the terrain
//! pyramid (z5–11 by default); missing tiles fall back to their nearest
//! ready ancestor (UV-windowed) and fresh tiles fade in over ~150 ms, so
//! zooming never shows holes or popping. With no source or an empty store
//! the layer simply draws nothing.

mod cache;
mod decode;
mod selection;
mod style;

pub use decode::{DecodedTile, TerrainDecodeError, decode_terrain_png};
pub use style::{TerrainStyle, tint_from_srgb8};

use self::cache::TileCache;
use self::selection::{MIN_TERRAIN_LEVEL, coverage_at_level, plan_tiles, select_level, uv_window};
use self::style::TerrainStyleUniform;
use crate::error::RenderError;
use crate::gpu::texture_sampler_bind_group_layout;
use crate::layer::{DrawCtx, MapLayer, PrepareCtx};
use crate::tiles::{TileId, TileSource};
use crate::workers::JobQueue;

use bytemuck::{Pod, Zeroable};
use glam::DVec2;

use std::ops::Range;
use std::sync::Arc;

/// GPU texture budget: ~200 R8 256² tiles ≈ 13 MB.
const TILE_CACHE_CAPACITY: usize = 200;
/// Negative-cache budget (tile IDs only — outside the data extent most
/// requests miss, e.g. the whole world minus Germany).
const MISSING_CACHE_CAPACITY: usize = 4096;
/// Negative entries expire after this so tiles ingested while the app runs
/// (`strata-ingest terrain`) appear without a restart. Refetches are indexed
/// SQLite point lookups on worker threads, at most
/// [`MAX_PENDING_FETCHES`] per interval — negligible.
const MISSING_RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);
/// Upper bound on in-flight fetch/decode jobs; the rest are picked up on
/// later frames (the layer keeps requesting redraws while tiles are pending).
const MAX_PENDING_FETCHES: usize = 64;

const VERTICES_PER_QUAD: u32 = 6;

/// Vertex layout of `raster_tile.wgsl` (camera-relative world units + UV +
/// per-tile fade).
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct RasterVertex {
    world_rel: [f32; 2],
    uv: [f32; 2],
    fade: f32,
}

const VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 3] =
    wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32];

/// Worker → render thread decode result. `None` means the source has no
/// such tile (or the bytes failed to decode) — negative-cached either way.
struct TileFetchResult {
    id: TileId,
    decoded: Option<DecodedTile>,
}

/// A resident tile texture (the bind group keeps the texture view alive).
/// `None` marks a fully transparent tile: it is "ready" — so neither
/// refetched nor ancestor-substituted — but nothing is drawn for it.
struct TileTexture {
    bind_group: Option<wgpu::BindGroup>,
}

/// One recorded draw: a vertex range of this frame's quad buffer bound to a
/// cached tile texture.
struct TileDraw {
    texture: TileId,
    vertices: Range<u32>,
}

/// Lazily created GPU state (needs the target format / shader library from
/// the first `prepare`).
struct TerrainGpu {
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    tile_layout: wgpu::BindGroupLayout,
    style_buffer: wgpu::Buffer,
    style_bind_group: wgpu::BindGroup,
    vertex_buffer: Option<wgpu::Buffer>,
    vertex_capacity: u64,
}

/// Raster terrain hillshade layer.
pub struct TerrainLayer {
    source: Option<Arc<dyn TileSource>>,
    max_source_zoom: u8,
    style: TerrainStyle,
    style_dirty: bool,
    jobs: JobQueue<TileFetchResult>,
    cache: TileCache<TileTexture>,
    gpu: Option<TerrainGpu>,
    pipeline_failed: bool,
    draws: Vec<TileDraw>,
    vertex_scratch: Vec<RasterVertex>,
    needs_redraw: bool,
}

impl TerrainLayer {
    pub fn new(source: Option<Arc<dyn TileSource>>, max_source_zoom: u8) -> Self {
        Self {
            source,
            max_source_zoom,
            style: TerrainStyle::default(),
            style_dirty: true,
            jobs: JobQueue::new(),
            cache: TileCache::new(
                TILE_CACHE_CAPACITY,
                MISSING_CACHE_CAPACITY,
                MISSING_RETRY_INTERVAL,
            ),
            gpu: None,
            pipeline_failed: false,
            draws: Vec::new(),
            vertex_scratch: Vec::new(),
            needs_redraw: false,
        }
    }

    pub fn source(&self) -> Option<&Arc<dyn TileSource>> {
        self.source.as_ref()
    }

    pub fn max_source_zoom(&self) -> u8 {
        self.max_source_zoom
    }

    pub fn style(&self) -> TerrainStyle {
        self.style
    }

    /// Replace the tint style; takes effect next frame.
    pub fn set_style(&mut self, style: TerrainStyle) {
        self.style = style;
        self.style_dirty = true;
    }

    /// Create pipeline/sampler/style resources on first use. Returns false
    /// (and permanently disables the layer) if the shader cannot be built.
    fn ensure_gpu(&mut self, ctx: &PrepareCtx<'_>) -> bool {
        if self.pipeline_failed {
            return false;
        }
        if self.gpu.is_some() {
            return true;
        }
        match TerrainGpu::new(ctx) {
            Ok(gpu) => {
                self.gpu = Some(gpu);
                true
            }
            Err(error) => {
                tracing::error!(%error, "terrain pipeline creation failed; layer disabled");
                self.pipeline_failed = true;
                false
            }
        }
    }
}

impl MapLayer for TerrainLayer {
    fn prepare(&mut self, ctx: &mut PrepareCtx<'_>) {
        self.draws.clear();
        self.needs_redraw = false;
        let Some(source) = self.source.clone() else {
            return;
        };
        if !self.ensure_gpu(ctx) {
            return;
        }

        if self.style_dirty
            && let Some(gpu) = &self.gpu
        {
            let uniform = TerrainStyleUniform::from(self.style);
            ctx.queue
                .write_buffer(&gpu.style_buffer, 0, bytemuck::bytes_of(&uniform));
            self.style_dirty = false;
        }

        // Upload tiles decoded since last frame.
        for result in self.jobs.drain() {
            match result.decoded {
                Some(decoded) if decoded.fully_transparent() => {
                    // Nothing to draw anywhere in this tile: ready, no GPU
                    // texture, never substituted by an ancestor.
                    tracing::debug!(tile = ?result.id, "terrain tile fully transparent");
                    self.cache
                        .insert_ready(result.id, TileTexture { bind_group: None });
                }
                Some(decoded) => {
                    if let Some(gpu) = &self.gpu {
                        tracing::debug!(tile = ?result.id, "terrain tile uploaded");
                        let texture = upload_tile(
                            ctx.device,
                            ctx.queue,
                            &gpu.tile_layout,
                            &gpu.sampler,
                            &decoded,
                        );
                        self.cache.insert_ready(result.id, texture);
                    }
                }
                None => self.cache.insert_missing(result.id),
            }
        }
        let fading = self.cache.advance_fades(ctx.frame.dt);

        // Select coverage and plan draws/fetches against the cache.
        let min_level = MIN_TERRAIN_LEVEL.min(self.max_source_zoom);
        let level = select_level(ctx.camera.zoom(), min_level, self.max_source_zoom);
        let (world_min, world_max) = ctx.camera.visible_world_bounds();
        let needed = coverage_at_level(level, world_min, world_max);
        let plan = plan_tiles(&needed, min_level, |id| self.cache.status(id));

        for id in plan.fetch {
            if self.cache.pending_count() >= MAX_PENDING_FETCHES {
                break;
            }
            if self.cache.begin_fetch(id) {
                let source = source.clone();
                self.jobs.submit(ctx.workers, move || {
                    let decoded = source.tile(id).and_then(|bytes| {
                        match decode_terrain_png(&bytes) {
                            Ok(tile) => Some(tile),
                            Err(error) => {
                                tracing::warn!(tile = ?id, %error, "terrain tile decode failed");
                                None
                            }
                        }
                    });
                    TileFetchResult { id, decoded }
                });
            }
        }

        // Build this frame's quads (camera-relative f32 positions).
        self.vertex_scratch.clear();
        let center = ctx.camera.center();
        for draw in &plan.draws {
            // Fully transparent tiles stay resident but emit no quad.
            if self
                .cache
                .resource(draw.texture)
                .is_none_or(|texture| texture.bind_group.is_none())
            {
                self.cache.promote(draw.texture);
                continue;
            }
            let Some((uv_min, uv_max)) = uv_window(draw.texture, draw.target) else {
                continue;
            };
            let (bounds_min, bounds_max) = draw.target.world_bounds();
            let start = self.vertex_scratch.len() as u32;
            push_quad(
                &mut self.vertex_scratch,
                bounds_min - center,
                bounds_max - center,
                uv_min,
                uv_max,
                draw.fade,
            );
            self.draws.push(TileDraw {
                texture: draw.texture,
                vertices: start..start + VERTICES_PER_QUAD,
            });
            self.cache.promote(draw.texture);
        }

        if !self.vertex_scratch.is_empty()
            && let Some(gpu) = &mut self.gpu
        {
            let bytes: &[u8] = bytemuck::cast_slice(&self.vertex_scratch);
            gpu.ensure_vertex_capacity(ctx.device, bytes.len() as u64);
            if let Some(buffer) = &gpu.vertex_buffer {
                ctx.queue.write_buffer(buffer, 0, bytes);
            }
        }

        self.needs_redraw = fading || self.cache.pending_count() > 0;
    }

    fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, _ctx: &DrawCtx<'_>) {
        let Some(gpu) = &self.gpu else {
            return;
        };
        let Some(vertex_buffer) = &gpu.vertex_buffer else {
            return;
        };
        if self.draws.is_empty() {
            return;
        }
        pass.set_pipeline(&gpu.pipeline);
        pass.set_bind_group(1, &gpu.style_bind_group, &[]);
        pass.set_vertex_buffer(0, vertex_buffer.slice(..));
        for draw in &self.draws {
            // Evicted-while-planned cannot happen (no insertions after
            // planning), but stay defensive: skip rather than panic.
            let Some(bind_group) = self
                .cache
                .resource(draw.texture)
                .and_then(|texture| texture.bind_group.as_ref())
            else {
                continue;
            };
            pass.set_bind_group(2, bind_group, &[]);
            pass.draw(draw.vertices.clone(), 0..1);
        }
    }

    fn wants_redraw(&self) -> bool {
        self.needs_redraw
    }
}

impl TerrainGpu {
    fn new(ctx: &PrepareCtx<'_>) -> Result<Self, RenderError> {
        let device = ctx.device;
        let module = ctx.shaders.create_module(device, "raster_tile.wgsl")?;

        let tile_layout = texture_sampler_bind_group_layout(device, "strata terrain tile layout");
        let style_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("strata terrain style layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(
                        std::mem::size_of::<TerrainStyleUniform>() as u64,
                    ),
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("strata terrain pipeline layout"),
            bind_group_layouts: &[
                Some(ctx.globals_layout),
                Some(&style_layout),
                Some(&tile_layout),
            ],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("strata terrain raster pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<RasterVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &VERTEX_ATTRIBUTES,
                }],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: ctx.target_format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("strata terrain sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..wgpu::SamplerDescriptor::default()
        });

        let style_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("strata terrain style"),
            size: std::mem::size_of::<TerrainStyleUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let style_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("strata terrain style"),
            layout: &style_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: style_buffer.as_entire_binding(),
            }],
        });

        Ok(Self {
            pipeline,
            sampler,
            tile_layout,
            style_buffer,
            style_bind_group,
            vertex_buffer: None,
            vertex_capacity: 0,
        })
    }

    /// Grow the per-frame quad buffer when needed (never shrinks).
    fn ensure_vertex_capacity(&mut self, device: &wgpu::Device, bytes: u64) {
        if self.vertex_buffer.is_some() && self.vertex_capacity >= bytes {
            return;
        }
        let capacity = bytes.next_power_of_two();
        self.vertex_buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("strata terrain quads"),
            size: capacity,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
        self.vertex_capacity = capacity;
    }
}

/// Upload a decoded grayscale+alpha tile as an `Rg8Unorm` texture
/// (r = shade, g = coverage alpha) and build its bind group.
/// (`Queue::write_texture` has no 256-byte row alignment requirement, so
/// rows are tightly packed.)
fn upload_tile(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    tile: &DecodedTile,
) -> TileTexture {
    let size = wgpu::Extent3d {
        width: tile.width,
        height: tile.height,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("strata terrain tile"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rg8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        texture.as_image_copy(),
        &tile.pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(tile.width * 2),
            rows_per_image: None,
        },
        size,
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("strata terrain tile"),
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
    });
    TileTexture {
        bind_group: Some(bind_group),
    }
}

/// Two CCW triangles covering `[min, max]` with the given UV window. The
/// world→camera-relative subtraction stays f64 until here, so f32 only holds
/// small quantities.
fn push_quad(
    vertices: &mut Vec<RasterVertex>,
    min: DVec2,
    max: DVec2,
    uv_min: DVec2,
    uv_max: DVec2,
    fade: f32,
) {
    let v = |world: DVec2, uv: DVec2| RasterVertex {
        world_rel: [world.x as f32, world.y as f32],
        uv: [uv.x as f32, uv.y as f32],
        fade,
    };
    let top_left = v(min, uv_min);
    let top_right = v(DVec2::new(max.x, min.y), DVec2::new(uv_max.x, uv_min.y));
    let bottom_left = v(DVec2::new(min.x, max.y), DVec2::new(uv_min.x, uv_max.y));
    let bottom_right = v(max, uv_max);
    vertices.extend_from_slice(&[
        top_left,
        top_right,
        bottom_left,
        bottom_left,
        top_right,
        bottom_right,
    ]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raster_vertex_matches_shader_stride() {
        assert_eq!(std::mem::size_of::<RasterVertex>(), 20);
    }

    #[test]
    fn push_quad_emits_two_triangles_with_uv_window() {
        let mut vertices = Vec::new();
        push_quad(
            &mut vertices,
            DVec2::new(-1.0, -2.0),
            DVec2::new(3.0, 4.0),
            DVec2::new(0.25, 0.5),
            DVec2::new(0.5, 0.75),
            0.7,
        );
        assert_eq!(vertices.len(), VERTICES_PER_QUAD as usize);
        assert!(vertices.iter().all(|v| v.fade == 0.7));
        // Corners: first vertex is the top-left, last the bottom-right.
        assert_eq!(vertices[0].world_rel, [-1.0, -2.0]);
        assert_eq!(vertices[0].uv, [0.25, 0.5]);
        assert_eq!(vertices[5].world_rel, [3.0, 4.0]);
        assert_eq!(vertices[5].uv, [0.5, 0.75]);
        // Both triangles wind the same way.
        let area = |a: [f32; 2], b: [f32; 2], c: [f32; 2]| {
            (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
        };
        let first = area(
            vertices[0].world_rel,
            vertices[1].world_rel,
            vertices[2].world_rel,
        );
        let second = area(
            vertices[3].world_rel,
            vertices[4].world_rel,
            vertices[5].world_rel,
        );
        assert!(first * second > 0.0);
    }

    #[test]
    fn layer_without_source_stays_inert() {
        let layer = TerrainLayer::new(None, 11);
        assert!(layer.source().is_none());
        assert_eq!(layer.max_source_zoom(), 11);
        assert!(!layer.wants_redraw());
    }

    #[test]
    fn style_roundtrips_and_marks_dirty() {
        let mut layer = TerrainLayer::new(None, 11);
        let style = TerrainStyle {
            opacity: 0.8,
            ..TerrainStyle::default()
        };
        layer.set_style(style);
        assert_eq!(layer.style(), style);
        assert!(layer.style_dirty);
    }

    /// Fully transparent tiles (all-no-data, e.g. open sea) become ready
    /// without a texture and produce no draws — no flat tinted block, no
    /// refetch loop, no ancestor fallback. Skipped without an adapter.
    #[test]
    fn fully_transparent_tiles_are_ready_but_never_drawn() {
        use crate::camera::{Camera, Viewport};
        use crate::gpu::Globals;
        use crate::gpu::shader::ShaderLibrary;
        use crate::layer::{FrameInfo, LayerToggles};
        use crate::workers::WorkerPool;
        use glam::UVec2;
        use std::io::Cursor;
        use std::time::{Duration, Instant};

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let Ok(adapter) =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
        else {
            eprintln!("skipping GPU test: no wgpu adapter");
            return;
        };
        let Ok((device, queue)) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
        else {
            eprintln!("skipping GPU test: no device");
            return;
        };

        struct TransparentPngSource;
        impl TileSource for TransparentPngSource {
            fn tile(&self, _id: TileId) -> Option<Vec<u8>> {
                let image = image::GrayAlphaImage::from_pixel(64, 64, image::LumaA([180, 0]));
                let mut bytes = Vec::new();
                image::DynamicImage::ImageLumaA8(image)
                    .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
                    .expect("encode PNG");
                Some(bytes)
            }
        }

        let mut layer = TerrainLayer::new(Some(Arc::new(TransparentPngSource)), 11);
        let workers = WorkerPool::new(2);
        let camera = Camera::new(Viewport::new(UVec2::new(512, 512), 1.0));
        let toggles = LayerToggles::all_enabled();
        let globals = Globals::new(&device);
        let shaders = ShaderLibrary::embedded();
        let mut frame_index = 0u64;
        let mut prepare = |layer: &mut TerrainLayer| {
            let mut ctx = PrepareCtx {
                device: &device,
                queue: &queue,
                camera: &camera,
                workers: &workers,
                layers: &toggles,
                frame: FrameInfo {
                    dt: Duration::from_millis(16),
                    frame_index,
                },
                target_format: wgpu::TextureFormat::Rgba8UnormSrgb,
                globals_layout: &globals.layout,
                shaders: &shaders,
            };
            layer.prepare(&mut ctx);
            frame_index += 1;
        };

        let deadline = Instant::now() + Duration::from_secs(10);
        prepare(&mut layer);
        while layer.wants_redraw() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
            prepare(&mut layer);
        }
        assert!(!layer.wants_redraw(), "layer failed to settle");
        assert_eq!(layer.cache.pending_count(), 0);
        assert!(
            layer.cache.ready_count() > 0,
            "tiles must be cached as ready"
        );
        assert!(
            layer.draws.is_empty(),
            "fully transparent tiles must not record draws"
        );
    }

    /// End-to-end on a real device (skipped without an adapter): tiles flow
    /// source → worker decode → texture upload → recorded draws, fades
    /// settle, and the draw pass validates.
    #[test]
    fn layer_uploads_and_draws_tiles_on_gpu() {
        use crate::camera::{Camera, Viewport};
        use crate::gpu::shader::ShaderLibrary;
        use crate::gpu::{GLOBALS_BIND_GROUP_INDEX, Globals};
        use crate::layer::{FrameInfo, LayerToggles};
        use crate::workers::WorkerPool;
        use glam::UVec2;
        use std::io::Cursor;
        use std::time::{Duration, Instant};

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let Ok(adapter) =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
        else {
            eprintln!("skipping GPU test: no wgpu adapter");
            return;
        };
        let Ok((device, queue)) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
        else {
            eprintln!("skipping GPU test: no device");
            return;
        };

        struct GradientPngSource;
        impl TileSource for GradientPngSource {
            fn tile(&self, _id: TileId) -> Option<Vec<u8>> {
                let image =
                    image::GrayImage::from_fn(64, 64, |x, y| image::Luma([(x * 2 + y) as u8]));
                let mut bytes = Vec::new();
                image::DynamicImage::ImageLuma8(image)
                    .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
                    .expect("encode PNG");
                Some(bytes)
            }
        }

        let mut layer = TerrainLayer::new(Some(Arc::new(GradientPngSource)), 11);
        let workers = WorkerPool::new(2);
        let camera = Camera::new(Viewport::new(UVec2::new(512, 512), 1.0));
        let toggles = LayerToggles::all_enabled();
        let globals = Globals::new(&device);
        let shaders = ShaderLibrary::embedded();
        let mut frame_index = 0u64;
        let mut prepare = |layer: &mut TerrainLayer| {
            let mut ctx = PrepareCtx {
                device: &device,
                queue: &queue,
                camera: &camera,
                workers: &workers,
                layers: &toggles,
                frame: FrameInfo {
                    dt: Duration::from_millis(16),
                    frame_index,
                },
                target_format: wgpu::TextureFormat::Rgba8UnormSrgb,
                globals_layout: &globals.layout,
                shaders: &shaders,
            };
            layer.prepare(&mut ctx);
            frame_index += 1;
        };

        // Tiles arrive asynchronously from the workers.
        let deadline = Instant::now() + Duration::from_secs(10);
        prepare(&mut layer);
        assert!(layer.wants_redraw(), "fetches must be in flight");
        while layer.draws.is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
            prepare(&mut layer);
        }
        assert!(!layer.draws.is_empty(), "no tiles arrived within deadline");

        // Once everything is resident, fades settle and the layer goes idle.
        for _ in 0..200 {
            if !layer.wants_redraw() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
            prepare(&mut layer);
        }
        assert!(!layer.wants_redraw(), "layer failed to settle");
        assert!(layer.cache.pending_count() == 0);

        // Record a real pass; the error scope catches validation failures.
        globals.update(&queue, &camera);
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("terrain test target"),
            size: wgpu::Extent3d {
                width: 512,
                height: 512,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("terrain test pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(GLOBALS_BIND_GROUP_INDEX, &globals.bind_group, &[]);
            let ctx = DrawCtx {
                camera: &camera,
                layers: &toggles,
                globals: &globals.bind_group,
            };
            layer.draw(&mut pass, &ctx);
        }
        queue.submit([encoder.finish()]);
        if let Some(error) = pollster::block_on(scope.pop()) {
            panic!("terrain draw failed validation: {error}");
        }
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("device poll");
    }
}
