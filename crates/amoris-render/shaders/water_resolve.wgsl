// Nearest opaque depth for screen-space water rays. Depth is reversed-Z.
@group(0) @binding(0) var depth: texture_depth_multisampled_2d;
@group(0) @binding(1) var resolved: texture_storage_2d<r32float, write>;
@compute @workgroup_size(8,8)
fn main(@builtin(global_invocation_id) i: vec3u) {
    let size = textureDimensions(resolved);
    if any(i.xy >= size) { return; }
    var z = 0.0;
    for (var s = 0u; s < textureNumSamples(depth); s++) {
        z = max(z, textureLoad(depth, vec2i(i.xy), i32(s)));
    }
    textureStore(resolved, vec2i(i.xy), vec4f(z,0.0,0.0,0.0));
}
