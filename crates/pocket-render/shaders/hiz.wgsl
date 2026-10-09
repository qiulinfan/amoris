// The depth pyramid of two-phase occlusion culling (docs/spec/occlusion.md): level 0 from the
// multisampled depth target, each further level from the one below, every texel the minimum
// (reversed-Z: the farthest) depth of what it covers. Levels halve with the texture's own mip
// sizes (rounding down); where a source dimension is odd, the last texel of the next level also
// takes the source's last row or column, so texel `t` of level `k` covers pixels
// `[t << (k + 1), (t + 1) << (k + 1))` and the last texel of a level everything to the edge.
// WebGPU core only: textureLoad of the depth target, r32float storage textures written, one
// dispatch per level.

@group(0) @binding(0) var depth: texture_depth_multisampled_2d;
@group(0) @binding(1) var dst: texture_storage_2d<r32float, write>;
@group(0) @binding(2) var src: texture_2d<f32>;

// The source texels destination texel `t` reduces along one axis: `2t` and `2t + 1`, and for the
// last destination texel everything up to the source's edge.
fn span(t: u32, dst_size: u32, src_size: u32) -> vec2u {
    let a = min(2u * t, src_size - 1u);
    let b = select(min(2u * t + 1u, src_size - 1u), src_size - 1u, t == dst_size - 1u);
    return vec2u(a, b);
}

@compute @workgroup_size(8, 8)
fn from_depth(@builtin(global_invocation_id) id: vec3u) {
    let size = textureDimensions(dst);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }
    let src_size = textureDimensions(depth);
    let samples = textureNumSamples(depth);
    let sx = span(id.x, size.x, src_size.x);
    let sy = span(id.y, size.y, src_size.y);
    var d = 1.0;
    for (var y = sy.x; y <= sy.y; y++) {
        for (var x = sx.x; x <= sx.y; x++) {
            for (var s = 0u; s < samples; s++) {
                d = min(d, textureLoad(depth, vec2u(x, y), s));
            }
        }
    }
    textureStore(dst, id.xy, vec4f(d, 0.0, 0.0, 0.0));
}

@compute @workgroup_size(8, 8)
fn reduce(@builtin(global_invocation_id) id: vec3u) {
    let size = textureDimensions(dst);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }
    let src_size = textureDimensions(src);
    let sx = span(id.x, size.x, src_size.x);
    let sy = span(id.y, size.y, src_size.y);
    var d = 1.0;
    for (var y = sy.x; y <= sy.y; y++) {
        for (var x = sx.x; x <= sx.y; x++) {
            d = min(d, textureLoad(src, vec2u(x, y), 0).x);
        }
    }
    textureStore(dst, id.xy, vec4f(d, 0.0, 0.0, 0.0));
}
