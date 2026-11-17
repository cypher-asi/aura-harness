//! A frame's quads drawn by the GPU: the whole display list is one
//! buffer of instances and one draw call, over the atlas as one texture.
//!
//! The picture is the one [`crate::raster`] paints. Quads are whole
//! pixels and the atlas is read texel for pixel, so nothing is filtered,
//! and the target is not an sRGB one: colors are blended as the bytes
//! they are, which is how the canvas renderer blended them.

use crate::scene::{Fill, Quad, Scene};
use crate::text::Atlas;
use anyhow::{anyhow, Context};

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

/// Draws [`Scene`]s into textures of one format.
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
    fn new(device: wgpu::Device, queue: wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("aura quads"),
            source: wgpu::ShaderSource::Wgsl(include_str!("quad.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("aura quads"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("aura quads"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("aura quads"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: INSTANCE as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    // The fields of `encode`, in its order.
                    attributes: &wgpu::vertex_attr_array![
                        0 => Sint32x2,
                        1 => Uint32x2,
                        2 => Uint32x2,
                        3 => Unorm8x4,
                        4 => Uint32,
                    ],
                }],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..wgpu::PrimitiveState::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fragment"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });
        let frame = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("aura frame"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let room = ROOM * INSTANCE as u64;
        let instances = instance_buffer(&device, room);
        Self {
            device,
            queue,
            pipeline,
            layout,
            frame,
            instances,
            room,
            bytes: Vec::new(),
            atlas: None,
        }
    }

    /// The tallest atlas this device takes as a texture.
    fn atlas_limit(&self) -> u32 {
        self.device.limits().max_texture_dimension_2d
    }

    /// Draw `scene` over all of `target`, which is `size` pixels, from
    /// `atlas`.
    #[allow(clippy::cast_precision_loss)] // window sizes
    fn draw(
        &mut self,
        scene: &Scene,
        atlas: &mut Atlas,
        target: &wgpu::TextureView,
        size: (u32, u32),
    ) {
        if !self.upload(atlas) {
            return;
        }

        // As many quads as an instance count can say; a frame has
        // thousands.
        let count = u32::try_from(scene.quads.len()).unwrap_or(u32::MAX);
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

        let mut frame = [0; 16];
        frame[..4].copy_from_slice(&(size.0 as f32).to_ne_bytes());
        frame[4..8].copy_from_slice(&(size.1 as f32).to_ne_bytes());
        self.queue.write_buffer(&self.frame, 0, &frame);

        let [red, green, blue, _] = scene.background.map(|level| f64::from(level) / 255.0);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
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
        self.queue.submit([encoder.finish()]);
    }

    /// Bring the texture up to date with `atlas`: all of it when the
    /// atlas grew, else the rows written since the last frame. `false`
    /// when the atlas is larger than a texture can be.
    fn upload(&mut self, atlas: &mut Atlas) -> bool {
        let (width, height) = atlas.size();
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
            self.atlas = Some(Texture {
                texture,
                size: (width, height),
                bind,
            });
        }
        let Some(kept) = &self.atlas else {
            return false;
        };
        let dirty = atlas.take_dirty();
        let rows = if fresh { Some(0..height) } else { dirty };
        if let Some(rows) = rows.filter(|rows| rows.start < rows.end && rows.end <= height) {
            let stride = width as usize * 4;
            let bytes = &atlas.pixels()[rows.start as usize * stride..rows.end as usize * stride];
            self.queue.write_texture(
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
                bytes,
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
        true
    }
}

fn instance_buffer(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("aura quads"),
        size,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// `quad` as the shader's `Quad`: its corner, its size, its corner in
/// the atlas, its color and how it is filled.
fn encode(out: &mut Vec<u8>, quad: &Quad) {
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

/// A device on the adapter the environment asks for (`WGPU_BACKEND`,
/// `WGPU_POWER_PREF`), else the one that draws least power: a grid of
/// text is no work for any of them.
fn device(
    instance: &wgpu::Instance,
    surface: Option<&wgpu::Surface<'_>>,
) -> anyhow::Result<(wgpu::Adapter, wgpu::Device, wgpu::Queue)> {
    let power = wgpu::PowerPreference::from_env().unwrap_or(wgpu::PowerPreference::LowPower);
    let adapter =
        futures::executor::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: power,
            force_fallback_adapter: false,
            compatible_surface: surface,
        }))
        .context("no graphics adapter")?;
    // What every backend can do, with textures as large as this adapter
    // takes: the window and the atlas are each one.
    let limits = wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits());
    let (device, queue) =
        futures::executor::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("aura"),
            required_limits: limits,
            memory_hints: wgpu::MemoryHints::MemoryUsage,
            ..wgpu::DeviceDescriptor::default()
        }))
        .context("opening the graphics device")?;
    Ok((adapter, device, queue))
}

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
        let plain = |format: &wgpu::TextureFormat| {
            matches!(
                format.remove_srgb_suffix(),
                wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm
            )
        };
        // One that is not sRGB when there is one, else an sRGB one seen
        // through a view that is not.
        let format = capabilities
            .formats
            .iter()
            .copied()
            .filter(plain)
            .min_by_key(wgpu::TextureFormat::is_srgb)
            .ok_or_else(|| anyhow!("the window has no 8-bit format"))?;
        let view = format.remove_srgb_suffix();
        let alpha_mode = if capabilities
            .alpha_modes
            .contains(&wgpu::CompositeAlphaMode::Opaque)
        {
            wgpu::CompositeAlphaMode::Opaque
        } else {
            wgpu::CompositeAlphaMode::Auto
        };
        let limit = device.limits().max_texture_dimension_2d;
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: width.clamp(1, limit),
            height: height.clamp(1, limit),
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
        };
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

    /// The tallest the atlas may grow ([`Atlas::set_limit`]).
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

    /// Draw `scene` and show it. `false` when the window gave no frame
    /// to draw into this time; the next one will.
    ///
    /// # Errors
    ///
    /// Fails when the surface is gone for good (out of memory, or the
    /// device was lost).
    pub fn render(&mut self, scene: &Scene, atlas: &mut Atlas) -> anyhow::Result<bool> {
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
        self.renderer.draw(scene, atlas, &view, self.size());
        let stale = frame.suboptimal;
        frame.present();
        if stale {
            self.configure();
        }
        Ok(true)
    }
}

/// The GPU drawing into a texture, to read the pixels back.
#[cfg(test)]
pub struct Offscreen {
    renderer: Renderer,
    name: String,
}

#[cfg(test)]
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
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn atlas_limit(&self) -> u32 {
        self.renderer.atlas_limit()
    }

    /// `scene` as RGBA rows, top to bottom: what [`crate::raster::paint`]
    /// gives.
    pub fn read(&mut self, scene: &Scene, atlas: &mut Atlas) -> Vec<u8> {
        let (width, height) = (scene.width.max(1), scene.height.max(1));
        let device = &self.renderer.device;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
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
        });
        // Rows of a copy start on multiples of 256 bytes.
        let stride = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("aura readback"),
            size: u64::from(stride) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.renderer.draw(scene, atlas, &view, (width, height));

        let device = &self.renderer.device;
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
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
        buffer.slice(..).map_async(wgpu::MapMode::Read, |result| {
            result.expect("mapping the readback buffer");
        });
        device
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quad_is_encoded_as_the_shader_reads_it() {
        let quad = Quad {
            x: -3,
            y: 7,
            w: 20,
            h: 18,
            color: [1, 2, 3, 4],
            fill: Fill::Mask,
            u: 512,
            v: 96,
        };
        let mut bytes = Vec::new();
        encode(&mut bytes, &quad);
        encode(&mut bytes, &quad.lowered(1));
        assert_eq!(bytes.len(), 2 * INSTANCE);
        let word = |at: usize| {
            u32::from_ne_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
        };
        assert_eq!(word(0), (-3i32).cast_unsigned());
        assert_eq!([word(4), word(8), word(12)], [7, 20, 18]);
        assert_eq!([word(16), word(20)], [512, 96]);
        assert_eq!(&bytes[24..28], &[1, 2, 3, 4]);
        assert_eq!(word(28), 1);
        assert_eq!(word(INSTANCE + 4), 8, "the next quad, one row lower");
    }
}
