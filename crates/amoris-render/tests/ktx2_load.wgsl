// The KTX2 test's reader (tests/ktx2.rs): every texel of one mip of a 2D texture, as the GPU
// decodes it, written unfiltered to an RGBA8 target of that mip's size.

@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var<uniform> level: vec4u;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
    let uv = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4f(uv * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) p: vec4f) -> @location(0) vec4f {
    return textureLoad(tex, vec2i(p.xy), i32(level.x));
}
