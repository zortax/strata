//! Shared GPU plumbing for the aero layers: the per-dataset local-origin
//! uniform (group 1 in `fill_airspace` / `line_dash` / `symbol` / `weather`
//! shaders), pipeline construction and mesh upload helpers.

use crate::error::RenderError;
use crate::gpu::shader::ShaderLibrary;
use crate::layer::PrepareCtx;

use bytemuck::{Pod, Zeroable};
use glam::DVec2;
use wgpu::util::DeviceExt as _;

/// Aero-layer shader sources not registered in the embedded library
/// ([`crate::gpu::shader::ShaderLibrary::embedded`] is owned elsewhere);
/// resolved on demand against the frame's library via [`create_layer_module`].
pub const FILL_AIRSPACE_SHADER: (&str, &str) = (
    "fill_airspace.wgsl",
    include_str!("../shaders/fill_airspace.wgsl"),
);
pub const LINE_DASH_SHADER: (&str, &str) =
    ("line_dash.wgsl", include_str!("../shaders/line_dash.wgsl"));
pub const ROUTE_LINE_SHADER: (&str, &str) = (
    "route_line.wgsl",
    include_str!("../shaders/route_line.wgsl"),
);
pub const ROUTE_RING_SHADER: (&str, &str) = (
    "route_ring.wgsl",
    include_str!("../shaders/route_ring.wgsl"),
);
pub const WEATHER_SHADER: (&str, &str) = ("weather.wgsl", include_str!("../shaders/weather.wgsl"));

/// CPU mirror of the `*Locals` uniform at group 1, binding 0: the dataset
/// origin in camera-relative world units, recomputed in f64 every frame.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct OriginUniform {
    pub origin_rel: [f32; 2],
    pub pad: [f32; 2],
}

/// The local-origin uniform buffer + bind group (group 1).
pub struct OriginBinding {
    pub layout: wgpu::BindGroupLayout,
    pub bind_group: wgpu::BindGroup,
    buffer: wgpu::Buffer,
}

impl OriginBinding {
    pub fn new(device: &wgpu::Device, label: &str) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(label),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(
                        std::mem::size_of::<OriginUniform>() as u64
                    ),
                },
                count: None,
            }],
        });
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: std::mem::size_of::<OriginUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        Self {
            layout,
            bind_group,
            buffer,
        }
    }

    /// Write `origin_world − camera_center` (f64 subtraction done by the
    /// caller) for this frame.
    pub fn update(&self, queue: &wgpu::Queue, origin_rel: DVec2) {
        let uniform = OriginUniform {
            origin_rel: [origin_rel.x as f32, origin_rel.y as f32],
            pad: [0.0; 2],
        };
        queue.write_buffer(&self.buffer, 0, bytemuck::bytes_of(&uniform));
    }
}

/// The frame's shader library extended with one extra (unregistered) source.
pub fn library_with(base: &ShaderLibrary, extra: (&'static str, &'static str)) -> ShaderLibrary {
    let mut sources: Vec<(&'static str, &'static str)> = base
        .names()
        .filter_map(|name| base.raw_source(name).map(|src| (name, src)))
        .collect();
    sources.push(extra);
    ShaderLibrary::from_sources(sources)
}

/// Create a shader module for an aero-layer source, resolving `//#include`
/// against the frame's library plus the given source.
pub fn create_layer_module(
    ctx: &PrepareCtx<'_>,
    shader: (&'static str, &'static str),
) -> Result<wgpu::ShaderModule, RenderError> {
    library_with(ctx.shaders, shader).create_module(ctx.device, shader.0)
}

/// One-stop render-pipeline builder for the aero layers: globals at group 0,
/// local origin at group 1, premultiplied alpha, triangle list, no culling.
pub fn create_layer_pipeline(
    ctx: &PrepareCtx<'_>,
    label: &str,
    module: &wgpu::ShaderModule,
    origin_layout: &wgpu::BindGroupLayout,
    buffers: &[wgpu::VertexBufferLayout<'_>],
) -> wgpu::RenderPipeline {
    let layout = ctx
        .device
        .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(label),
            bind_group_layouts: &[Some(ctx.globals_layout), Some(origin_layout)],
            immediate_size: 0,
        });
    ctx.device
        .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers,
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module,
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
        })
}

/// An uploaded vertex+index buffer pair.
pub struct GpuMesh {
    pub vertices: wgpu::Buffer,
    pub indices: wgpu::Buffer,
    pub index_count: u32,
}

/// Upload a tessellated mesh; `None` when there is nothing to draw.
pub fn upload_mesh<V: Pod>(
    device: &wgpu::Device,
    label: &str,
    vertices: &[V],
    indices: &[u32],
) -> Option<GpuMesh> {
    if vertices.is_empty() || indices.is_empty() {
        return None;
    }
    let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytemuck::cast_slice(vertices),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytemuck::cast_slice(indices),
        usage: wgpu::BufferUsages::INDEX,
    });
    Some(GpuMesh {
        vertices: vertex_buffer,
        indices: index_buffer,
        index_count: indices.len() as u32,
    })
}
