// The scene's depth for the splat pass (docs/spec/splats.md 6): splats blend into the resolved,
// single-sample HDR image (2.2x cheaper than blending into four samples), so they test against a
// single-sample copy of the multisampled depth, taken from each pixel's first sample.

@group(0) @binding(0) var depth_msaa: texture_depth_multisampled_2d;

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
    let p = vec2f(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
    return vec4f(p, 0.0, 1.0);
}

@fragment
fn fs_depth(@builtin(position) pos: vec4f) -> @builtin(frag_depth) f32 {
    return textureLoad(depth_msaa, vec2i(pos.xy), 0);
}
