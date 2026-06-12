//! GPU side of the basemap layer: the fill+stroke render pipeline
//! (`shaders/basemap_tile.wgsl`), per-tile mesh buffers and a dynamic-offset
//! uniform arena for the per-draw tile transform + fade alpha.

use crate::basemap::tess::{BasemapVertex, MeshData};
use crate::error::RenderError;
use crate::gpu::shader::ShaderLibrary;
use crate::layer::PrepareCtx;
use crate::tiles::TileId;

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

/// Shader file name; the source is embedded here so the layer works whether
/// or not the shared library lists it.
pub const SHADER_NAME: &str = "basemap_tile.wgsl";
const SHADER_SOURCE: &str = include_str!("../shaders/basemap_tile.wgsl");

const INITIAL_UNIFORM_SLOTS: u32 = 256;

/// CPU mirror of `BasemapTile` in `basemap_tile.wgsl` (16 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct TileUniform {
    /// Tile origin in world units relative to the camera center.
    pub origin_rel: [f32; 2],
    /// Tile side length in world units.
    pub scale: f32,
    /// Fade-in multiplier.
    pub alpha: f32,
}

/// Uploaded tile mesh.
pub struct GpuMesh {
    pub vertices: wgpu::Buffer,
    pub indices: wgpu::Buffer,
    pub index_count: u32,
}

impl GpuMesh {
    pub fn upload(device: &wgpu::Device, mesh: &MeshData, id: TileId) -> Self {
        let label = format!("basemap tile {}/{}/{}", id.z, id.x, id.y);
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&label),
            contents: bytemuck::cast_slice(&mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&label),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        Self {
            vertices,
            indices,
            index_count: mesh.indices.len() as u32,
        }
    }
}

/// Pipeline + per-draw uniform arena. Created lazily on the first `prepare`
/// (needs the target format and shared layouts from the context).
pub struct BasemapGpu {
    pipeline: wgpu::RenderPipeline,
    tile_layout: wgpu::BindGroupLayout,
    uniform_stride: u32,
    uniform_capacity: u32,
    uniform_buffer: wgpu::Buffer,
    tile_bind_group: wgpu::BindGroup,
}

impl BasemapGpu {
    pub fn new(ctx: &PrepareCtx<'_>) -> Result<Self, RenderError> {
        let device = ctx.device;
        // Prefer the shared library (lets a future embedding/hot-reload win);
        // fall back to a local library over the same `common.wgsl`.
        let module = if ctx.shaders.raw_source(SHADER_NAME).is_some() {
            ctx.shaders.create_module(device, SHADER_NAME)?
        } else {
            let common = ctx.shaders.raw_source("common.wgsl").ok_or_else(|| {
                RenderError::ShaderNotFound {
                    name: "common.wgsl".to_owned(),
                    referenced_from: SHADER_NAME.to_owned(),
                }
            })?;
            ShaderLibrary::from_sources([("common.wgsl", common), (SHADER_NAME, SHADER_SOURCE)])
                .create_module(device, SHADER_NAME)?
        };

        let tile_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("basemap tile layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(
                        std::mem::size_of::<TileUniform>() as u64
                    ),
                },
                count: None,
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("basemap pipeline layout"),
            bind_group_layouts: &[Some(ctx.globals_layout), Some(&tile_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("basemap tiles"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: BasemapVertex::STRIDE,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &BasemapVertex::ATTRIBUTES,
                }],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: ctx.target_format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let uniform_stride = uniform_stride(device);
        let (uniform_buffer, tile_bind_group) =
            create_uniform_arena(device, &tile_layout, uniform_stride, INITIAL_UNIFORM_SLOTS);

        Ok(Self {
            pipeline,
            tile_layout,
            uniform_stride,
            uniform_capacity: INITIAL_UNIFORM_SLOTS,
            uniform_buffer,
            tile_bind_group,
        })
    }

    pub fn pipeline(&self) -> &wgpu::RenderPipeline {
        &self.pipeline
    }

    pub fn tile_bind_group(&self) -> &wgpu::BindGroup {
        &self.tile_bind_group
    }

    /// Byte offset of uniform slot `index` (for `set_bind_group` dynamic
    /// offsets).
    pub fn uniform_offset(&self, index: u32) -> u32 {
        index * self.uniform_stride
    }

    /// Write this frame's per-draw uniforms into slots `0..uniforms.len()`,
    /// growing the arena if needed.
    pub fn upload_tile_uniforms(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        uniforms: &[TileUniform],
    ) {
        let needed = uniforms.len() as u32;
        if needed == 0 {
            return;
        }
        if needed > self.uniform_capacity {
            let capacity = needed.next_power_of_two();
            let (buffer, bind_group) =
                create_uniform_arena(device, &self.tile_layout, self.uniform_stride, capacity);
            self.uniform_buffer = buffer;
            self.tile_bind_group = bind_group;
            self.uniform_capacity = capacity;
        }
        let stride = self.uniform_stride as usize;
        let mut staging = vec![0u8; stride * uniforms.len()];
        for (i, uniform) in uniforms.iter().enumerate() {
            let bytes = bytemuck::bytes_of(uniform);
            staging[i * stride..i * stride + bytes.len()].copy_from_slice(bytes);
        }
        queue.write_buffer(&self.uniform_buffer, 0, &staging);
    }
}

fn uniform_stride(device: &wgpu::Device) -> u32 {
    let align = device.limits().min_uniform_buffer_offset_alignment;
    (std::mem::size_of::<TileUniform>() as u32).max(align)
}

fn create_uniform_arena(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    stride: u32,
    capacity: u32,
) -> (wgpu::Buffer, wgpu::BindGroup) {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("basemap tile uniforms"),
        size: stride as u64 * capacity as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("basemap tile uniforms"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: &buffer,
                offset: 0,
                size: wgpu::BufferSize::new(std::mem::size_of::<TileUniform>() as u64),
            }),
        }],
    });
    (buffer, bind_group)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The naga validation gate required for every shader (the shared
    /// `gpu::shader` test only covers the embedded list).
    #[test]
    fn basemap_shader_resolves_and_validates_with_naga() {
        let embedded = ShaderLibrary::embedded();
        let common = embedded
            .raw_source("common.wgsl")
            .expect("common.wgsl embedded");
        let lib =
            ShaderLibrary::from_sources([("common.wgsl", common), (SHADER_NAME, SHADER_SOURCE)]);
        let resolved = lib.resolve(SHADER_NAME).expect("resolve");
        let module = naga::front::wgsl::parse_str(&resolved)
            .unwrap_or_else(|e| panic!("{SHADER_NAME} failed to parse: {e}"));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("{SHADER_NAME} failed validation: {e:?}"));
    }

    #[test]
    fn tile_uniform_matches_wgsl_layout() {
        assert_eq!(std::mem::size_of::<TileUniform>(), 16);
        assert_eq!(std::mem::offset_of!(TileUniform, origin_rel), 0);
        assert_eq!(std::mem::offset_of!(TileUniform, scale), 8);
        assert_eq!(std::mem::offset_of!(TileUniform, alpha), 12);
    }

    #[test]
    fn vertex_attributes_cover_the_stride() {
        assert_eq!(BasemapVertex::STRIDE, 36);
        let last = BasemapVertex::ATTRIBUTES
            .last()
            .expect("attributes non-empty");
        assert_eq!(last.offset + 16, BasemapVertex::STRIDE); // vec4<f32> color
    }
}
