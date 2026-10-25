// Caustics under water (water_caustics.rs): the sun reaching a point under a water region's
// surface is focused by the waves above it (the map's gathered light, its contrast growing with
// the point's depth up to the bed's) and absorbed along its refracted path down.
struct CausticsMap { origin:vec2f, texel:f32, on:f32, size:vec2u, unused:vec2u, sigma:vec4f }
@group(1) @binding(15) var caustics_map: texture_2d<f32>;
@group(1) @binding(16) var<uniform> caustics: CausticsMap;
fn caustic_texel(q: vec2i) -> vec4f {
    return textureLoad(caustics_map, clamp(q, vec2i(0), vec2i(caustics.size) - 1), 0);
}
fn caustic_light(world: vec3f) -> vec3f {
    if (caustics.on == 0.0) { return vec3f(1.0); }
    let q = (world.xz - caustics.origin) / caustics.texel - 0.5;
    if (any(q < vec2f(-0.5)) || any(q > vec2f(caustics.size) - 0.5)) { return vec3f(1.0); }
    let surface = caustic_texel(vec2i(round(q))).w;
    let depth = surface - world.y;
    if (surface < -1e3 || depth <= 0.0) { return vec3f(1.0); }
    let i0 = vec2i(floor(q));
    let t = q - floor(q);
    let m = mix(mix(caustic_texel(i0).xy, caustic_texel(i0 + vec2i(1, 0)).xy, t.x),
        mix(caustic_texel(i0 + vec2i(0, 1)).xy, caustic_texel(i0 + vec2i(1, 1)).xy, t.x), t.y);
    let focus = max(1.0 + (m.x - 1.0) * clamp(depth / max(m.y, 0.05), 0.0, 1.0), 0.0);
    // The refracted sun's path down to the point: depth over the cosine of its angle below.
    let s = max(view.sun_dir.y, 0.0);
    let down = sqrt(max(1.0 - 0.5625 * (1.0 - s * s), 0.01));
    return focus * exp(-caustics.sigma.rgb * (depth / down));
}
