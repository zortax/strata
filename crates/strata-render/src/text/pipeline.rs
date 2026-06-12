//! The glyph render pipeline: one instanced triangle-strip quad per glyph,
//! premultiplied-alpha blending over the map (`shaders/text.wgsl`).

use crate::error::RenderError;
use crate::gpu::shader::ShaderLibrary;

use bytemuck::{Pod, Zeroable};

const SHADER_NAME: &str = "text.wgsl";
const INITIAL_INSTANCE_CAPACITY: usize = 256;

/// CPU mirror of `GlyphInstance` in `text.wgsl` (vertex buffer slot 1).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub(crate) struct GlyphInstance {
    /// Quad top-left, logical px (screen space, y-down).
    pub pos_px: [f32; 2],
    /// Quad size, logical px.
    pub size_px: [f32; 2],
    pub uv_min: [f32; 2],
    pub uv_max: [f32; 2],
    /// Premultiplied linear RGBA.
    pub color: [f32; 4],
}

const QUAD_CORNERS: [[f32; 2]; 4] = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]];

const QUAD_ATTRIBUTES: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![0 => Float32x2];
const INSTANCE_ATTRIBUTES: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
    1 => Float32x2,
    2 => Float32x2,
    3 => Float32x2,
    4 => Float32x2,
    5 => Float32x4,
];

pub(crate) struct TextPipeline {
    pipeline: wgpu::RenderPipeline,
    quad: wgpu::Buffer,
    instances: wgpu::Buffer,
    capacity: usize,
    count: u32,
}

impl TextPipeline {
    pub fn new(
        device: &wgpu::Device,
        target_format: wgpu::TextureFormat,
        globals_layout: &wgpu::BindGroupLayout,
        atlas_layout: &wgpu::BindGroupLayout,
        shaders: &ShaderLibrary,
    ) -> Result<Self, RenderError> {
        let module = shaders.create_module(device, SHADER_NAME)?;
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("strata text pipeline layout"),
            bind_group_layouts: &[Some(globals_layout), Some(atlas_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("strata text pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<[f32; 2]>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &QUAD_ATTRIBUTES,
                    },
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<GlyphInstance>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &INSTANCE_ATTRIBUTES,
                    },
                ],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let quad = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("strata text quad"),
            size: std::mem::size_of_val(&QUAD_CORNERS) as u64,
            usage: wgpu::BufferUsages::VERTEX,
            mapped_at_creation: true,
        });
        quad.slice(..)
            .get_mapped_range_mut()
            .copy_from_slice(bytemuck::cast_slice(&QUAD_CORNERS));
        quad.unmap();

        Ok(Self {
            pipeline,
            quad,
            instances: create_instance_buffer(device, INITIAL_INSTANCE_CAPACITY),
            capacity: INITIAL_INSTANCE_CAPACITY,
            count: 0,
        })
    }

    /// Replace the instance buffer contents (grows the buffer as needed).
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        instances: &[GlyphInstance],
    ) {
        if instances.len() > self.capacity {
            self.capacity = instances.len().next_power_of_two();
            self.instances = create_instance_buffer(device, self.capacity);
        }
        if !instances.is_empty() {
            queue.write_buffer(&self.instances, 0, bytemuck::cast_slice(instances));
        }
        self.count = instances.len() as u32;
    }

    pub fn draw<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        atlas_bind_group: &'a wgpu::BindGroup,
    ) {
        if self.count == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(1, atlas_bind_group, &[]);
        pass.set_vertex_buffer(0, self.quad.slice(..));
        pass.set_vertex_buffer(1, self.instances.slice(..));
        pass.draw(0..QUAD_CORNERS.len() as u32, 0..self.count);
    }
}

fn create_instance_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("strata text instances"),
        size: (capacity * std::mem::size_of::<GlyphInstance>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}
