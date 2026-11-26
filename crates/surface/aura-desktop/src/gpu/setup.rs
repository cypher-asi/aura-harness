//! Setting up the device and the quad pipeline, once.

use super::{Renderer, INSTANCE, ROOM};
use anyhow::Context;

impl Renderer {
    pub(super) fn new(
        device: wgpu::Device,
        queue: wgpu::Queue,
        format: wgpu::TextureFormat,
    ) -> Self {
        let shader = shader(&device);
        let layout = bind_group_layout(&device);
        let pipeline_layout = pipeline_layout(&device, &layout);
        let pipeline = pipeline(&device, &pipeline_layout, &shader, format);
        let frame = frame_buffer(&device);
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
}

fn shader(device: &wgpu::Device) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("aura quads"),
        source: wgpu::ShaderSource::Wgsl(include_str!("../quad.wgsl").into()),
    })
}

/// The frame's size for the vertex shader, the atlas for the fragment
/// shader.
fn bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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
    })
}

fn pipeline_layout(device: &wgpu::Device, layout: &wgpu::BindGroupLayout) -> wgpu::PipelineLayout {
    device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("aura quads"),
        bind_group_layouts: &[layout],
        push_constant_ranges: &[],
    })
}

fn pipeline(
    device: &wgpu::Device,
    pipeline_layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("aura quads"),
        layout: Some(pipeline_layout),
        vertex: wgpu::VertexState {
            module: shader,
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
            module: shader,
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
    })
}

/// The target's size, for the shader.
fn frame_buffer(device: &wgpu::Device) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("aura frame"),
        size: 16,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

pub(super) fn instance_buffer(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("aura quads"),
        size,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// A device on the adapter the environment asks for (`WGPU_BACKEND`,
/// `WGPU_POWER_PREF`), else the one that draws least power: a grid of
/// text is no work for any of them.
pub(super) fn device(
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
