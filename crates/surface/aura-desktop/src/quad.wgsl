// One quad of the display list per instance, as a strip of four corners.
// Positions and atlas coordinates are whole pixels, and the atlas is read
// texel for pixel, never filtered.

struct Frame {
    // The target, in pixels
    size: vec2<f32>,
    unused: vec2<f32>,
}

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var atlas: texture_2d<f32>;

struct Quad {
    @location(0) at: vec2<i32>,
    @location(1) size: vec2<u32>,
    @location(2) texel: vec2<u32>,
    @location(3) color: vec4<f32>,
    // 0: the color; 1: the color through the atlas's coverage; 2: the
    // atlas's colors
    @location(4) fill: u32,
}

struct Corner {
    @builtin(position) position: vec4<f32>,
    @location(0) texel: vec2<f32>,
    @location(1) @interpolate(flat) color: vec4<f32>,
    @location(2) @interpolate(flat) fill: u32,
}

@vertex
fn vertex(@builtin(vertex_index) index: u32, quad: Quad) -> Corner {
    let across = vec2<f32>(f32(index & 1u), f32(index >> 1u)) * vec2<f32>(quad.size);
    let pixel = vec2<f32>(quad.at) + across;
    var corner: Corner;
    corner.position = vec4<f32>(
        pixel.x / frame.size.x * 2.0 - 1.0,
        1.0 - pixel.y / frame.size.y * 2.0,
        0.0,
        1.0,
    );
    corner.texel = vec2<f32>(quad.texel) + across;
    corner.color = quad.color;
    corner.fill = quad.fill;
    return corner;
}

// Colors are not premultiplied; the target blends them by their alpha.
@fragment
fn fragment(corner: Corner) -> @location(0) vec4<f32> {
    if corner.fill == 0u {
        return corner.color;
    }
    // A pixel's center is half a texel in, so this rounds down to it.
    let texel = textureLoad(atlas, vec2<i32>(corner.texel), 0);
    let alpha = corner.color.a * texel.a;
    if corner.fill == 1u {
        return vec4<f32>(corner.color.rgb, alpha);
    }
    return vec4<f32>(texel.rgb, alpha);
}
