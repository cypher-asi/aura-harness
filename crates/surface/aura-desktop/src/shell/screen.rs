//! What the window's frames are shown on: the GPU, or the window's own
//! pixels filled in on the CPU where there is no GPU worth the name.
//!
//! `AURA_DESKTOP_RENDERER=cpu` draws on the CPU whatever the machine
//! has, and `=gpu` on whatever adapter there is, a software one too.

use super::present::Presenter;
use super::window::Wake;
use crate::gpu::Gpu;
use crate::raster;
use crate::scene::Scene;
use crate::text::{Atlas, TextSystem};
use anyhow::{anyhow, Context as _};
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::window::Window;

pub enum Screen {
    /// The GPU, showing frames from a thread of its own
    Gpu(Presenter),
    Soft(Soft),
}

/// How showing a frame went.
#[derive(Debug)]
pub enum Showing {
    /// Shown, after this long drawing and showing it
    Done(Duration),
    /// The window gave nothing to draw into this time. The next thing
    /// that changes how it looks tries again.
    Missed,
    /// Handed to the thread that shows frames, which says how it went
    /// with [`Wake::Presented`]
    Handed,
}

impl Screen {
    /// The best there is to draw `window` on. A GPU tells the loop with
    /// `wake` how showing each frame went.
    ///
    /// # Errors
    ///
    /// Fails when the window can be drawn on neither way.
    pub fn new(
        window: &Arc<Window>,
        wake: impl Fn(Wake) -> bool + Send + 'static,
    ) -> anyhow::Result<Self> {
        let asked = std::env::var("AURA_DESKTOP_RENDERER").ok();
        let asked = asked.as_deref().map(str::trim);
        if asked != Some("cpu") {
            let size = window.inner_size();
            match Gpu::new(Arc::clone(window), size.width, size.height) {
                Ok(gpu) if asked == Some("gpu") || !gpu.is_software() => {
                    tracing::info!(adapter = gpu.name(), "drawing on the GPU");
                    return Presenter::start(gpu, Arc::clone(window), wake).map(Self::Gpu);
                }
                // A software adapter rasterizes on the CPU as well, with
                // more to do for the same pixels.
                Ok(gpu) => tracing::info!(adapter = gpu.name(), "no GPU: drawing on the CPU"),
                Err(error) => {
                    tracing::warn!(error = format!("{error:#}"), "no GPU: drawing on the CPU");
                }
            }
        }
        Soft::new(Arc::clone(window)).map(Self::Soft)
    }

    /// The tallest the glyph atlas may grow here, if it matters.
    pub const fn atlas_limit(&self) -> Option<u32> {
        match self {
            Self::Gpu(presenter) => Some(presenter.atlas_limit()),
            Self::Soft(_) => None,
        }
    }

    /// Whether a frame can be drawn now without waiting behind the last.
    pub const fn ready(&self) -> bool {
        match self {
            Self::Gpu(presenter) => presenter.ready(),
            Self::Soft(_) => true,
        }
    }

    /// Show `scene`, at its size, calling `before_showing` once it is
    /// drawn on this thread.
    ///
    /// # Errors
    ///
    /// Fails when the window can't be drawn on any more.
    pub fn render(
        &mut self,
        scene: &Scene,
        text: &mut TextSystem,
        before_showing: impl FnOnce(),
    ) -> anyhow::Result<Showing> {
        match self {
            Self::Gpu(presenter) => {
                presenter.hand_over(scene, text.atlas_mut());
                Ok(Showing::Handed)
            }
            Self::Soft(soft) => {
                let started = Instant::now();
                let shown = soft.render(scene, text.atlas(), before_showing)?;
                Ok(if shown {
                    Showing::Done(started.elapsed())
                } else {
                    Showing::Missed
                })
            }
        }
    }

    /// The thread that shows frames said how one went.
    pub fn presented(&mut self) {
        if let Self::Gpu(presenter) = self {
            presenter.presented();
        }
    }
}

/// The window's pixels, filled in by [`crate::raster`].
pub struct Soft {
    surface: softbuffer::Surface<Arc<Window>, Arc<Window>>,
    /// Kept for as long as the surface made from it
    _context: softbuffer::Context<Arc<Window>>,
    size: (u32, u32),
    pixels: Vec<u8>,
}

impl Soft {
    fn new(window: Arc<Window>) -> anyhow::Result<Self> {
        // Its errors hold what can't cross threads; their text can.
        let failed = |error: softbuffer::SoftBufferError| anyhow!("{error}");
        let context = softbuffer::Context::new(Arc::clone(&window))
            .map_err(failed)
            .context("reaching the display")?;
        let surface = softbuffer::Surface::new(&context, window)
            .map_err(failed)
            .context("reaching the window's pixels")?;
        Ok(Self {
            surface,
            _context: context,
            size: (0, 0),
            pixels: Vec::new(),
        })
    }

    fn render(
        &mut self,
        scene: &Scene,
        atlas: &Atlas,
        before_showing: impl FnOnce(),
    ) -> anyhow::Result<bool> {
        let failed = |error: softbuffer::SoftBufferError| anyhow!("{error}");
        let size = (scene.width, scene.height);
        let (Some(width), Some(height)) = (NonZeroU32::new(size.0), NonZeroU32::new(size.1)) else {
            return Ok(false);
        };
        if self.size != size {
            self.surface
                .resize(width, height)
                .map_err(failed)
                .context("sizing the window's pixels")?;
            self.size = size;
        }
        raster::paint(scene, atlas, &mut self.pixels);
        let mut buffer = self
            .surface
            .buffer_mut()
            .map_err(failed)
            .context("taking the window's pixels")?;
        if buffer.len() * 4 != self.pixels.len() {
            return Ok(false);
        }
        pack(&self.pixels, &mut buffer);
        before_showing();
        buffer
            .present()
            .map_err(failed)
            .context("showing the window's pixels")?;
        Ok(true)
    }
}

/// RGBA bytes as the words a window's pixels are: red, green and blue
/// from the high end down, under a byte that is left empty.
fn pack(rgba: &[u8], out: &mut [u32]) {
    for (out, pixel) in out.iter_mut().zip(rgba.chunks_exact(4)) {
        *out = u32::from(pixel[0]) << 16 | u32::from(pixel[1]) << 8 | u32::from(pixel[2]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixels_are_packed_red_first_without_alpha() {
        let mut out = [0; 2];
        pack(&[0x12, 0x34, 0x56, 0xff, 0xff, 0x00, 0x01, 0x80], &mut out);
        assert_eq!(out, [0x0012_3456, 0x00ff_0001]);
    }
}
