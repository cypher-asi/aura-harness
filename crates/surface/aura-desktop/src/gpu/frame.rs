//! What happens every frame: the atlas's new rows and the quads go up,
//! and one pass draws them.

use super::setup::instance_buffer;
use super::{Renderer, Texture, INSTANCE};
use crate::scene::{Fill, Quad, Scene};
use crate::text::AtlasRows;

impl Renderer {
    /// Draw `scene` over all of `target`, which is `size` pixels, from
    /// the atlas as uploaded.
    pub(super) fn draw(&mut self, scene: &Scene, target: &wgpu::TextureView, size: (u32, u32)) {
        // As many quads as an instance count can say; a frame has
        // thousands.
        let count = u32::try_from(scene.quads.len()).unwrap_or(u32::MAX);
        let used = self.upload_instances(scene, count);
        self.upload_frame_size(size);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        self.pass(&mut encoder, scene, target, count, used);
        self.queue.submit([encoder.finish()]);
    }

    /// Encode the first `count` quads of `scene` and write them to the
    /// instance buffer, grown when they don't fit; the bytes they take.
    fn upload_instances(&mut self, scene: &Scene, count: u32) -> u64 {
        self.bytes.clear();
        self.bytes.reserve(scene.quads.len() * INSTANCE);
        for quad in scene.quads.iter().take(count as usize) {
            encode(&mut self.bytes, quad);
        }
        let used = self.bytes.len() as u64;
        if used > self.room {
            self.room = used.next_power_of_two();
            self.instances = instance_buffer(&self.device, self.room);
        }
        self.queue.write_buffer(&self.instances, 0, &self.bytes);
        used
    }

    #[allow(clippy::cast_precision_loss)] // window sizes
    fn upload_frame_size(&self, size: (u32, u32)) {
        let mut frame = [0; 16];
        frame[..4].copy_from_slice(&(size.0 as f32).to_ne_bytes());
        frame[4..8].copy_from_slice(&(size.1 as f32).to_ne_bytes());
        self.queue.write_buffer(&self.frame, 0, &frame);
    }

    /// Clear `target` to the background and draw the `count` instances.
    fn pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        scene: &Scene,
        target: &wgpu::TextureView,
        count: u32,
        used: u64,
    ) {
        let [red, green, blue, _] = scene.background.map(|level| f64::from(level) / 255.0);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("aura frame"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: red,
                        g: green,
                        b: blue,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        if let Some(kept) = self.atlas.as_ref().filter(|_| count > 0) {
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &kept.bind, &[]);
            pass.set_vertex_buffer(0, self.instances.slice(..used));
            pass.draw(0..4, 0..count);
        }
    }

    /// Bring the texture up to date with the atlas: a new texture when it
    /// changed size, which comes with every row, else the rows written
    /// since the last frame. `false` when the atlas is larger than a
    /// texture can be.
    pub(super) fn upload(&mut self, atlas: &AtlasRows) -> bool {
        let (width, height) = atlas.size;
        let limit = self.atlas_limit();
        if width > limit || height > limit {
            tracing::error!(width, height, limit, "the atlas is larger than a texture");
            return false;
        }
        let fresh = self
            .atlas
            .as_ref()
            .is_none_or(|kept| kept.size != (width, height));
        if fresh {
            self.atlas = Some(self.atlas_texture(width, height));
        }
        let Some(kept) = &self.atlas else {
            return false;
        };
        write_rows(&self.queue, kept, atlas);
        true
    }

    /// A texture for an atlas of `width` × `height`, bound for the shader.
    fn atlas_texture(&self, width: u32, height: u32) -> Texture {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("aura atlas"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("aura quads"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.frame.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
            ],
        });
        Texture {
            texture,
            size: (width, height),
            bind,
        }
    }
}

/// Write the atlas's new rows into `kept`, when there are any that fit.
fn write_rows(queue: &wgpu::Queue, kept: &Texture, atlas: &AtlasRows) {
    let (width, height) = atlas.size;
    let rows = atlas.rows();
    if rows.start < rows.end && rows.end <= height {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &kept.texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: 0,
                    y: rows.start,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            &atlas.pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width,
                height: rows.end - rows.start,
                depth_or_array_layers: 1,
            },
        );
    }
}

/// `quad` as the shader's `Quad`: its corner, its size, its corner in
/// the atlas, its color and how it is filled.
pub(super) fn encode(out: &mut Vec<u8>, quad: &Quad) {
    out.extend_from_slice(&quad.x.to_ne_bytes());
    out.extend_from_slice(&quad.y.to_ne_bytes());
    out.extend_from_slice(&quad.w.to_ne_bytes());
    out.extend_from_slice(&quad.h.to_ne_bytes());
    out.extend_from_slice(&quad.u.to_ne_bytes());
    out.extend_from_slice(&quad.v.to_ne_bytes());
    out.extend_from_slice(&quad.color);
    let fill: u32 = match quad.fill {
        Fill::Solid => 0,
        Fill::Mask => 1,
        Fill::Image => 2,
    };
    out.extend_from_slice(&fill.to_ne_bytes());
}
