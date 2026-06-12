//! Shared GPU plumbing: the per-frame globals uniform and bind group layout
//! helpers.
//!
//! ## Coordinate convention for shaders
//!
//! World coordinates do not fit in `f32` at deep zoom, so vertex positions
//! handed to shaders are **world units relative to the camera center**, with
//! the f64 subtraction done on the CPU (per tile origin / per upload).
//! `Globals.camera_to_clip` then maps those camera-relative units to clip
//! space in pure f32. See `shaders/common.wgsl`.

pub mod shader;

use crate::camera::Camera;

use bytemuck::{Pod, Zeroable};

/// Bind group index of [`Globals`] in every pipeline.
pub const GLOBALS_BIND_GROUP_INDEX: u32 = 0;

/// CPU mirror of the `Globals` uniform struct in `shaders/common.wgsl`.
/// Layout (std140-compatible, 32 bytes):
///
/// | offset | field            | type        |
/// |--------|------------------|-------------|
/// | 0      | camera_to_clip   | `vec2<f32>` |
/// | 8      | viewport_size_px | `vec2<f32>` |
/// | 16     | zoom             | `f32`       |
/// | 20     | scale_factor     | `f32`       |
/// | 24     | pad              | `vec2<f32>` |
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct GlobalsUniform {
    /// Camera-relative world units → clip space (y is negated: world y grows
    /// south/down, clip y grows up).
    pub camera_to_clip: [f32; 2],
    /// Render target size in physical pixels.
    pub viewport_size_px: [f32; 2],
    /// Continuous camera zoom.
    pub zoom: f32,
    /// Device scale factor (physical px per logical px).
    pub scale_factor: f32,
    pub pad: [f32; 2],
}

impl GlobalsUniform {
    pub fn from_camera(camera: &Camera) -> Self {
        let viewport = camera.viewport();
        let size = viewport.size_px();
        // Physical px per world unit; f64 until the final relative quantity.
        let scale_phys = camera.world_scale() * viewport.scale_factor() as f64;
        Self {
            camera_to_clip: [
                (2.0 * scale_phys / size.x as f64) as f32,
                (-2.0 * scale_phys / size.y as f64) as f32,
            ],
            viewport_size_px: [size.x as f32, size.y as f32],
            zoom: camera.zoom() as f32,
            scale_factor: viewport.scale_factor(),
            pad: [0.0; 2],
        }
    }
}

/// The shared globals uniform buffer + bind group (group 0 everywhere).
pub struct Globals {
    pub layout: wgpu::BindGroupLayout,
    pub bind_group: wgpu::BindGroup,
    buffer: wgpu::Buffer,
}

impl Globals {
    pub fn new(device: &wgpu::Device) -> Self {
        let layout = uniform_bind_group_layout(device, "strata globals layout");
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("strata globals"),
            size: std::mem::size_of::<GlobalsUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("strata globals"),
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

    /// Write the current camera state into the uniform buffer.
    pub fn update(&self, queue: &wgpu::Queue, camera: &Camera) {
        let uniform = GlobalsUniform::from_camera(camera);
        queue.write_buffer(&self.buffer, 0, bytemuck::bytes_of(&uniform));
    }
}

/// A single uniform buffer visible to vertex + fragment stages at binding 0.
pub fn uniform_bind_group_layout(device: &wgpu::Device, label: &str) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(
                    std::mem::size_of::<GlobalsUniform>() as u64
                ),
            },
            count: None,
        }],
    })
}

/// A filterable 2D texture (binding 0) + filtering sampler (binding 1) for
/// the fragment stage — raster tiles, symbol atlas, glyph atlas.
pub fn texture_sampler_bind_group_layout(
    device: &wgpu::Device,
    label: &str,
) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    })
}
