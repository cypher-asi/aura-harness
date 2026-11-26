//! A frame's quads drawn by the GPU: the whole display list is one
//! buffer of instances and one draw call, over the atlas as one texture.
//!
//! The picture is the one [`crate::raster`] paints. Quads are whole
//! pixels and the atlas is read texel for pixel, so nothing is filtered,
//! and the target is not an sRGB one: colors are blended as the bytes
//! they are, as the CPU rasterizer blends them.

mod frame;
#[cfg(test)]
mod offscreen;
mod setup;
#[cfg(test)]
mod tests;
mod window;

#[cfg(test)]
pub use offscreen::Offscreen;
pub use window::Gpu;

/// Bytes of one quad in the instance buffer.
const INSTANCE: usize = 32;

/// Quads the instance buffer starts with room for.
const ROOM: u64 = 4096;

/// The atlas as a texture, and what the shader is bound to with it.
struct Texture {
    texture: wgpu::Texture,
    size: (u32, u32),
    bind: wgpu::BindGroup,
}

/// Draws [`crate::scene::Scene`]s into textures of one format.
struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    /// The target's size, for the shader
    frame: wgpu::Buffer,
    instances: wgpu::Buffer,
    /// Bytes `instances` holds
    room: u64,
    /// The frame's instances, before they are uploaded
    bytes: Vec<u8>,
    atlas: Option<Texture>,
}

impl Renderer {
    /// The tallest atlas this device takes as a texture.
    fn atlas_limit(&self) -> u32 {
        self.device.limits().max_texture_dimension_2d
    }

    /// The size of the atlas the texture holds, if there is one.
    #[cfg(test)]
    fn atlas_size(&self) -> Option<(u32, u32)> {
        self.atlas.as_ref().map(|kept| kept.size)
    }
}
