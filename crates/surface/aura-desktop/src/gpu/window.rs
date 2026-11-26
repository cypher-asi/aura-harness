//! The GPU drawing into a window.

use super::setup::device;
use super::Renderer;
use crate::scene::Scene;
use crate::text::AtlasRows;
use anyhow::{anyhow, Context};

/// The GPU drawing into a window.
pub struct Gpu {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    /// The format frames are drawn in: the surface's, without sRGB
    /// encoding
    view: wgpu::TextureFormat,
    renderer: Renderer,
    software: bool,
    name: String,
}

impl Gpu {
    /// Set up drawing into `window`, which is `width` × `height` pixels.
    ///
    /// # Errors
    ///
    /// Fails when there is no adapter that can draw to the window, or
    /// the window has no 8-bit format.
    pub fn new(
        window: impl Into<wgpu::SurfaceTarget<'static>>,
        width: u32,
        height: u32,
    ) -> anyhow::Result<Self> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
        let surface = instance
            .create_surface(window)
            .context("creating the window's surface")?;
        let (adapter, device, queue) = device(&instance, Some(&surface))?;
        let info = adapter.get_info();

        let capabilities = surface.get_capabilities(&adapter);
        let format = surface_format(&capabilities)?;
        let view = format.remove_srgb_suffix();
        let limit = device.limits().max_texture_dimension_2d;
        let size = (width.clamp(1, limit), height.clamp(1, limit));
        let config = surface_config(&capabilities, format, view, size);
        surface.configure(&device, &config);
        Ok(Self {
            surface,
            config,
            view,
            renderer: Renderer::new(device, queue, view),
            software: info.device_type == wgpu::DeviceType::Cpu,
            name: format!("{} ({:?})", info.name, info.backend),
        })
    }

    /// The adapter and its backend, for the log.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether the "GPU" is the CPU running a software rasterizer, which
    /// [`crate::raster`] does with less work.
    pub const fn is_software(&self) -> bool {
        self.software
    }

    /// The tallest the atlas may grow ([`crate::text::Atlas::set_limit`]).
    pub fn atlas_limit(&self) -> u32 {
        self.renderer.atlas_limit()
    }

    /// The size frames are drawn at, in pixels.
    pub const fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    /// The window is `width` × `height` pixels now.
    pub fn resize(&mut self, width: u32, height: u32) {
        let limit = self.renderer.atlas_limit();
        let size = (width.clamp(1, limit), height.clamp(1, limit));
        if size != self.size() {
            (self.config.width, self.config.height) = size;
            self.configure();
        }
    }

    fn configure(&self) {
        self.surface.configure(&self.renderer.device, &self.config);
    }

    /// Draw `scene` at its size from the atlas brought up to date with
    /// `atlas`, and show it, calling `before_showing` once it is drawn.
    /// `false` when the window gave no frame to draw into this time; the
    /// next one will.
    ///
    /// # Errors
    ///
    /// Fails when the surface is gone for good (out of memory, or the
    /// device was lost).
    pub fn render(
        &mut self,
        scene: &Scene,
        atlas: &AtlasRows,
        before_showing: impl FnOnce(),
    ) -> anyhow::Result<bool> {
        // Before the window is asked for a frame, which it may not give:
        // these rows are handed over only once.
        let uploaded = self.renderer.upload(atlas);
        self.resize(scene.width, scene.height);
        let frame = match self.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.configure();
                match self.surface.get_current_texture() {
                    Ok(frame) => frame,
                    Err(wgpu::SurfaceError::OutOfMemory) => {
                        return Err(anyhow!("out of graphics memory"));
                    }
                    Err(_) => return Ok(false),
                }
            }
            Err(wgpu::SurfaceError::Timeout) => return Ok(false),
            Err(error) => return Err(error).context("taking the window's next frame"),
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(self.view),
            ..wgpu::TextureViewDescriptor::default()
        });
        if uploaded {
            self.renderer.draw(scene, &view, self.size());
        }
        let stale = frame.suboptimal;
        before_showing();
        frame.present();
        if stale {
            self.configure();
        }
        Ok(true)
    }
}

/// The window's 8-bit format: one that is not sRGB when there is one,
/// else an sRGB one seen through a view that is not.
fn surface_format(capabilities: &wgpu::SurfaceCapabilities) -> anyhow::Result<wgpu::TextureFormat> {
    let plain = |format: &wgpu::TextureFormat| {
        matches!(
            format.remove_srgb_suffix(),
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm
        )
    };
    capabilities
        .formats
        .iter()
        .copied()
        .filter(plain)
        .min_by_key(wgpu::TextureFormat::is_srgb)
        .ok_or_else(|| anyhow!("the window has no 8-bit format"))
}

/// How the surface is set up: frames of `format`, drawn through `view`,
/// `size` pixels, opaque when the window can be.
fn surface_config(
    capabilities: &wgpu::SurfaceCapabilities,
    format: wgpu::TextureFormat,
    view: wgpu::TextureFormat,
    (width, height): (u32, u32),
) -> wgpu::SurfaceConfiguration {
    let alpha_mode = if capabilities
        .alpha_modes
        .contains(&wgpu::CompositeAlphaMode::Opaque)
    {
        wgpu::CompositeAlphaMode::Opaque
    } else {
        wgpu::CompositeAlphaMode::Auto
    };
    wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format,
        width,
        height,
        present_mode: wgpu::PresentMode::AutoVsync,
        // A frame is shown as soon as it is drawn, not queued behind
        // another.
        desired_maximum_frame_latency: 1,
        alpha_mode,
        view_formats: if view == format {
            Vec::new()
        } else {
            vec![view]
        },
    }
}
