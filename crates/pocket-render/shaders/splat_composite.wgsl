// Composites the tile rasterizer's image (splat_tile.wgsl: premultiplied color, transmittance) over
// the resolved HDR image: blending One, SrcAlpha gives splats + transmittance x scene.

@group(0) @binding(0) var splat_image: texture_2d<f32>;

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
    let p = vec2f(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
    return vec4f(p, 0.0, 1.0);
}

@fragment
fn fs_composite(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    return textureLoad(splat_image, vec2i(pos.xy), 0);
}
