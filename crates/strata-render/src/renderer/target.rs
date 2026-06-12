//! Double-buffered offscreen color target.
//!
//! Ping-pong: the renderer draws into the *back* texture while the app's UI
//! may still be sampling the *front* one from the previous frame. After
//! submission the buffers swap and the just-completed texture becomes front.

use glam::UVec2;

pub(crate) struct RenderTargets {
    format: wgpu::TextureFormat,
    size: UVec2,
    textures: [wgpu::Texture; 2],
    views: [wgpu::TextureView; 2],
    /// Index rendered into next; `1 - back` is what the app samples.
    back: usize,
}

impl RenderTargets {
    pub(crate) fn new(device: &wgpu::Device, format: wgpu::TextureFormat, size: UVec2) -> Self {
        let size = size.max(UVec2::ONE);
        let textures = [
            create_target(device, format, size, "strata color target A"),
            create_target(device, format, size, "strata color target B"),
        ];
        let views = [
            textures[0].create_view(&wgpu::TextureViewDescriptor::default()),
            textures[1].create_view(&wgpu::TextureViewDescriptor::default()),
        ];
        Self {
            format,
            size,
            textures,
            views,
            back: 0,
        }
    }

    /// Recreate both buffers at `size` if it changed.
    pub(crate) fn resize(&mut self, device: &wgpu::Device, size: UVec2) {
        let size = size.max(UVec2::ONE);
        if size != self.size {
            *self = Self::new(device, self.format, size);
        }
    }

    /// The view to render the coming frame into.
    pub(crate) fn back_view(&self) -> &wgpu::TextureView {
        &self.views[self.back]
    }

    /// Flip after submission; returns the just-completed front texture.
    pub(crate) fn swap(&mut self) -> &wgpu::Texture {
        self.back = 1 - self.back;
        &self.textures[1 - self.back]
    }

    /// The most recently completed texture (what the app samples).
    pub(crate) fn front_texture(&self) -> &wgpu::Texture {
        &self.textures[1 - self.back]
    }
}

fn create_target(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    size: UVec2,
    label: &str,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size.x,
            height: size.y,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}
