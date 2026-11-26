//! The GPU drawing into a texture, to read the pixels back.

use super::setup::device;
use super::Renderer;
use crate::scene::Scene;
use crate::text::Atlas;

/// The GPU drawing into a texture, to read the pixels back.
pub struct Offscreen {
    renderer: Renderer,
    name: String,
    software: bool,
}

impl Offscreen {
    const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

    /// # Errors
    ///
    /// Fails when there is no adapter.
    pub fn new() -> anyhow::Result<Self> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
        let (adapter, device, queue) = device(&instance, None)?;
        let info = adapter.get_info();
        Ok(Self {
            renderer: Renderer::new(device, queue, Self::FORMAT),
            name: format!("{} ({:?})", info.name, info.backend),
            software: info.device_type == wgpu::DeviceType::Cpu,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether the adapter is the CPU running a software rasterizer.
    pub const fn is_software(&self) -> bool {
        self.software
    }

    pub fn atlas_limit(&self) -> u32 {
        self.renderer.atlas_limit()
    }

    /// `scene` as RGBA rows, top to bottom: what [`crate::raster::paint`]
    /// gives.
    pub fn read(&mut self, scene: &Scene, atlas: &mut Atlas) -> Vec<u8> {
        let (width, height) = (scene.width.max(1), scene.height.max(1));
        let texture = self.target(width, height);
        // Rows of a copy start on multiples of 256 bytes.
        let stride = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = self.readback(stride, height);
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let rows = atlas.take_rows(self.renderer.atlas_size());
        if self.renderer.upload(&rows) {
            self.renderer.draw(scene, &view, (width, height));
        }
        self.copy(&texture, &buffer, stride, (width, height));
        self.mapped(&buffer, stride, (width, height))
    }

    /// The texture frames are drawn into and copied out of.
    fn target(&self, width: u32, height: u32) -> wgpu::Texture {
        self.renderer
            .device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("aura offscreen"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: Self::FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
    }

    fn readback(&self, stride: u32, height: u32) -> wgpu::Buffer {
        self.renderer.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("aura readback"),
            size: u64::from(stride) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        })
    }

    /// Copy `texture` into `buffer`, `stride` bytes a row.
    fn copy(
        &self,
        texture: &wgpu::Texture,
        buffer: &wgpu::Buffer,
        stride: u32,
        (width, height): (u32, u32),
    ) {
        let device = &self.renderer.device;
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.renderer.queue.submit([encoder.finish()]);
    }

    /// Wait for `buffer` and take its rows without their padding.
    fn mapped(&self, buffer: &wgpu::Buffer, stride: u32, (width, height): (u32, u32)) -> Vec<u8> {
        buffer.slice(..).map_async(wgpu::MapMode::Read, |result| {
            result.expect("mapping the readback buffer");
        });
        self.renderer
            .device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("waiting for the GPU");
        let mapped = buffer.slice(..).get_mapped_range();
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for row in mapped.chunks_exact(stride as usize) {
            pixels.extend_from_slice(&row[..(width * 4) as usize]);
        }
        pixels
    }
}
