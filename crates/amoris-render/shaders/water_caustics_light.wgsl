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
// The surface over a map point, bilinear; a dry texel takes the nearest one's, and a point whose
// nearest texel is dry has none (far below anything).
fn caustic_surface(q: vec2f) -> f32 {
    let home = caustic_texel(vec2i(round(q))).w;
    if (home < -1e3) { return home; }
    let i0 = vec2i(floor(q));
    let t = q - floor(q);
    var s = vec4f(caustic_texel(i0).w, caustic_texel(i0 + vec2i(1, 0)).w,
        caustic_texel(i0 + vec2i(0, 1)).w, caustic_texel(i0 + vec2i(1, 1)).w);
    s = select(s, vec4f(home), s < vec4f(-1e3));
    return mix(mix(s.x, s.y, t.x), mix(s.z, s.w, t.x), t.y);
}
// How far up the refracted sun a point under water asks the cascades for shadow, metres of height
// (shadows.rs `UNDERWATER_REACH_M`): each cascade's box and caster volume hold its slice swept that
// far up the path.
const SUN_REACH_M: f32 = 4.0;
// The sun as a point under the surface meets it: refracted by flat water (Snell's law, air over
// water 1/1.333), so steeper than in air, along a path that leaves the water above the point.
// `dir` points to the sun, and `exit`, where the path meets the surface (or `SUN_REACH_M` up it,
// for a deeper point), is where the cascades are asked: what stands over the water shadows the
// point along the light's real path, and what lies wholly under it shadows nothing. In air the low
// sun's shadows of the bed's own slopes are far longer than the refracted light's, and hard.
// `beam` is the refracted beam's irradiance over the sun's: bent toward the vertical it widens, so
// a level bed receives what it did in air. Above the surface, outside the map or with the sun
// down: the sun in air, at the point itself.
struct SunPath { dir: vec3f, exit: vec3f, beam: f32 }
fn sun_path(world: vec3f) -> SunPath {
    let l = view.sun_dir.xyz;
    let air = SunPath(l, world, 1.0);
    if (caustics.on == 0.0 || l.y <= 0.0) { return air; }
    let q = (world.xz - caustics.origin) / caustics.texel - 0.5;
    if (any(q < vec2f(-0.5)) || any(q > vec2f(caustics.size) - 0.5)) { return air; }
    let depth = caustic_surface(q) - world.y;
    if (depth <= 0.0) { return air; }
    let dir = vec3f(0.75 * l.x, sqrt(1.0 - 0.5625 * (1.0 - l.y * l.y)), 0.75 * l.z);
    return SunPath(dir, world + dir * (min(depth, SUN_REACH_M) / dir.y), l.y / dir.y);
}
