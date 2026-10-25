// The liquid surface (fluid_surface.rs; screen-space fluid rendering after W. J. van der Laan,
// S. Green and M. Sainz, "Screen space fluid rendering with curvature flow", I3D 2009, and the
// passes of S. Lague's Fluid-Sim, github.com/SebLague/Fluid-Sim at 4717b72, MIT, Copyright (c) 2023
// Sebastian Lague). Its particles are an MLS-MPM solver's and the spray's entry crowns (water.md
// 52), whose sheets it draws where they are wide enough on the screen (`crown_share`, shared with
// the spray's streaks in water_spray_common.wgsl, which this source is given). Once the frame's
// last colour pass has drawn, the liquid's particles are drawn as spheres into textures of their
// own, tested against the scene's depth: the nearest front's
// distance with the particle's filter radius and air, and every sphere's chord added up (the
// liquid's thickness and its absorption along it). A separable bilateral filter, its reach a world
// radius per pixel, smooths the depth and the thickness together. The composite rebuilds each
// pixel's point and normal from the smoothed depth and shades it as water: the frame behind seen
// through it bent along the refracted ray by its thickness and absorbed along it (Beer), the sky
// reflected by the exact dielectric Fresnel, the sun's glint, white where the liquid is full of air,
// the frame's fog. Depths are distances along the view direction, metres; 0 is no liquid.

struct Particle {
    x: vec3f,
    age: f32,
    v: vec3f,
    tag: u32,
    c: mat3x3f,
};

struct Region {
    origin: vec4f,        // the region's corner in the world; w: metres per cell (0: closed)
    absorb: vec4f,        // the liquid's absorption per metre (rgb)
    generation: vec4u,    // x: the generation open in this slot
};

// An MLS-MPM source's splats.
struct MpmSplat {
    size: vec4f,          // x: the depth sphere's radius in cells; y: lifetime (s, 0: for ever);
                          // z: the thickness sphere's radius in cells; w: the filter's radius over
                          // the depth sphere's
    look: vec4f,          // x: air (0 clear water, 1 white)
};

struct FilterStep {
    dir: vec4f,           // xy: the step direction, (1, 0) or (0, 1)
};

const DEAD: u32 = 0xffffffffu;
// The filter's reach at most, pixels, and its steepness (Lague's `strength`: a sixth of the reach
// over this is one standard deviation).
const MAX_FILTER_PX: i32 = 32;
const FILTER_STRENGTH: f32 = 0.45;
// How far the refracted ray runs inside the liquid, times its thickness.
const REFRACTION: f32 = 1.0;
// Water's index over air's, inverted: light passing from air into water.
const ETA: f32 = 0.75;
// The liquid's scattering per metre, rgb alike: water with a little in it.
const SCATTER: f32 = 0.15;

@group(0) @binding(0) var<uniform> view: View;
// The scene's depth (reversed-Z), as many samples as the opaque pass.
@group(0) @binding(1) var depth: texture_depth_multisampled_2d;

// --- The splats ----------------------------------------------------------------------------------

@group(1) @binding(0) var<storage, read> particles: array<Particle>;
@group(1) @binding(1) var<uniform> regions: array<Region, 4>;
@group(1) @binding(2) var<uniform> mpm: MpmSplat;

struct Splat {
    @builtin(position) clip: vec4f,
    @location(0) uv: vec2f,
    @location(1) center: vec3f,                     // view space
    @location(2) radius: f32,                       // the sphere's, metres
    @location(3) @interpolate(flat) absorb: vec3f,  // per metre
    @location(4) @interpolate(flat) attrs: vec4f,   // x: filter radius (m), y: air, z: share,
                                                    // w: the thickness over the sphere's chord
};

// The scene's nearest and farthest sample at `px`, as reversed-Z depths.
fn scene_depths(px: vec2i) -> vec2f {
    var near = 0.0;
    var far = 1.0;
    for (var s = 0u; s < textureNumSamples(depth); s++) {
        let d = textureLoad(depth, px, i32(s));
        near = max(near, d);
        far = min(far, d);
    }
    return vec2f(near, far);
}

// The distance along the view direction of a reversed-Z depth (`proj[3][2] / (ndc + proj[2][2])`;
// the far plane, 0, at infinity).
fn view_distance(ndc: f32) -> f32 {
    if (ndc <= 0.0) {
        return 1e30;
    }
    return view.proj[3][2] / (ndc + view.proj[2][2]);
}

fn hidden_splat() -> Splat {
    var o: Splat;
    o.clip = vec4f(0.0, 0.0, 2.0, 1.0);
    return o;
}

// Particle `ii` of an MLS-MPM source as a sphere `radius_cells` cells across.
fn mpm_splat(vi: u32, ii: u32, radius_cells: f32) -> Splat {
    let p = particles[ii];
    let slot = p.tag & 7u;
    let r = regions[min(slot, 3u)];
    if (p.tag == DEAD || r.origin.w <= 0.0 || r.generation.x != (p.tag >> 3u)) {
        return hidden_splat();
    }
    let world = r.origin.xyz + p.x * r.origin.w;
    // In its last half second a particle shrinks away.
    var fade = 1.0;
    if (mpm.size.y > 0.0) {
        fade = clamp((mpm.size.y - p.age) * 2.0, 0.0, 1.0);
    }
    let radius = radius_cells * r.origin.w * fade;
    if (radius <= 0.0) {
        return hidden_splat();
    }
    let corners = array<vec2f, 6>(vec2f(-1.0, -1.0), vec2f(1.0, -1.0), vec2f(1.0, 1.0),
                                  vec2f(-1.0, -1.0), vec2f(1.0, 1.0), vec2f(-1.0, 1.0));
    let c = corners[vi];
    let center = (view.view * vec4f(world, 1.0)).xyz;
    var o: Splat;
    o.center = center;
    o.uv = c;
    o.radius = radius;
    o.absorb = r.absorb.rgb;
    o.attrs = vec4f(mpm.size.x * mpm.size.w * r.origin.w * fade, mpm.look.x, 1.0, 1.0);
    // The quad faces the eye at the sphere's front so it is not cut by the near plane early.
    o.clip = view.proj * vec4f(center + vec3f(c * radius, radius), 1.0);
    return o;
}

@vertex
fn mpm_depth_vs(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> Splat {
    return mpm_splat(vi, ii, mpm.size.x);
}

@vertex
fn mpm_thick_vs(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> Splat {
    return mpm_splat(vi, ii, mpm.size.z);
}

// --- The spray's entry crowns (water_spray.rs; water.md 52) ------------------------------------

// The spray's state: its counters, then the slots of its parcels alive, one an instance.
struct SprayList { header: array<u32, 8>, drawn: array<u32> };
@group(1) @binding(0) var<storage, read> droplets: array<Droplet>;
@group(1) @binding(1) var<storage, read> list: SprayList;
@group(1) @binding(2) var<uniform> spray: SprayParams;
// The thickness of a crown's sheet over its parcel's sphere's chord at birth (about a centimetre
// for a parcel 4 cm across), thinning as the sheet stretches: its radius grows as the square root
// of the stretch, so its chord's share falls as the stretch to the -1.5.
const SHEET: f32 = 0.25;

// A crown's parcel, wide enough on the screen, as its share of the sheet; any other parcel none.
@vertex
fn spray_vs(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> Splat {
    let d = droplets[list.drawn[ii]];
    if (d.info.x != 6u || d.velocity.w <= 0.0 || d.position.w >= d.velocity.w) {
        return hidden_splat();
    }
    let c = crown_unpack(d.info.w);
    let age = d.position.w;
    let radius = crown_radius(c, age);
    let pixel = 2.0 * distance(view.camera_pos.xyz, d.position.xyz)
        / (view.proj[1][1] * view.viewport.y);
    // As the streak it takes over from fades out at the end of its life.
    let share = crown_share(radius, pixel) * (1.0 - smoothstep(0.85, 1.0, age / d.velocity.w));
    if (radius <= 0.0 || share <= 0.0) {
        return hidden_splat();
    }
    let corners = array<vec2f, 6>(vec2f(-1.0, -1.0), vec2f(1.0, -1.0), vec2f(1.0, 1.0),
                                  vec2f(-1.0, -1.0), vec2f(1.0, 1.0), vec2f(-1.0, 1.0));
    let corner = corners[vi];
    let center = (view.view * vec4f(d.position.xyz, 1.0)).xyz;
    var o: Splat;
    o.center = center;
    o.uv = corner;
    o.radius = radius;
    o.absorb = spray.absorb.xyz;
    o.attrs = vec4f(1.5 * radius, crown_air(d.info.y), share,
                    SHEET * pow(crown_stretch(c, age), -1.5));
    o.clip = view.proj * vec4f(center + vec3f(corner * radius, radius), 1.0);
    return o;
}

struct DepthOut {
    @location(0) raw: vec2f,       // distance, air
    @location(1) attrs: vec4f,     // filter radius, share
    @builtin(frag_depth) z: f32,
};

// The nearest front: a sphere's, where the scene does not hide it in every sample.
@fragment
fn splat_depth_fs(in: Splat) -> DepthOut {
    let d = dot(in.uv, in.uv);
    if (d > 1.0) {
        discard;
    }
    let p = in.center + vec3f(in.uv, sqrt(1.0 - d)) * in.radius;
    let clip = view.proj * vec4f(p, 1.0);
    let ndc = clip.z / clip.w;
    if (ndc <= scene_depths(vec2i(in.clip.xy)).y) {
        discard;
    }
    var o: DepthOut;
    o.raw = vec2f(-p.z, in.attrs.y);
    o.attrs = vec4f(in.attrs.x, in.attrs.z, 0.0, 1.0);
    o.z = ndc;
    return o;
}

struct ThickOut {
    @location(2) thick: vec4f,     // rgb: optical depth (absorption times length), a: length
};

// The chord through the sphere at this pixel, cut where the scene's farthest sample lies in it.
@fragment
fn splat_thick_fs(in: Splat) -> ThickOut {
    let d = dot(in.uv, in.uv);
    if (d > 1.0) {
        discard;
    }
    let half_chord = in.radius * sqrt(1.0 - d);
    let front = -in.center.z - half_chord;
    let back = -in.center.z + half_chord;
    let scene = view_distance(scene_depths(vec2i(in.clip.xy)).y);
    let chord = clamp(min(back, scene) - max(front, 0.0), 0.0, 2.0 * half_chord) * in.attrs.w;
    if (chord <= 0.0) {
        discard;
    }
    var o: ThickOut;
    o.thick = vec4f(in.absorb * chord, chord);
    return o;
}

// --- Full-screen passes ---------------------------------------------------------------------------

struct Screen {
    @builtin(position) clip: vec4f,
};

@vertex
fn screen_vs(@builtin(vertex_index) vi: u32) -> Screen {
    let p = vec2f(f32((vi << 1u) & 2u), f32(vi & 2u)) * 2.0 - 1.0;
    var o: Screen;
    o.clip = vec4f(p, 0.0, 1.0);
    return o;
}

// The splats as drawn: the nearest front's distance and air, its filter radius and share, and the
// summed thickness. The filter reads its last pass's depth and thickness from `smooth_in` and
// `thick_in` and writes the next into `smooth_out` and `thick_out`, only where there is liquid.
@group(1) @binding(0) var raw_in: texture_2d<f32>;
@group(1) @binding(1) var attrs_in: texture_2d<f32>;
@group(1) @binding(2) var smooth_in: texture_2d<f32>;
@group(1) @binding(3) var thick_in: texture_2d<f32>;
@group(1) @binding(4) var<uniform> pass_step: FilterStep;
@group(1) @binding(5) var smooth_out: texture_storage_2d<rg32float, write>;
@group(1) @binding(6) var thick_out: texture_storage_2d<rgba16float, write>;

// One pass of a separable bilateral filter along `pass_step.dir` (S. Lague's BilateralPass.hlsl): a
// Gaussian over a reach that is the nearest front's filter radius on the screen, each neighbour
// weighted too by how close its unfiltered depth is, so the surface smooths without melting into
// what lies behind it. Neighbours with no liquid are left out. A compute pass, so the pixels
// without liquid cost one read and are neither cleared nor stored: no pass reads them.
@compute @workgroup_size(8, 8)
fn filter_cs(@builtin(global_invocation_id) id: vec3u) {
    let px = vec2i(id.xy);
    let size = vec2i(textureDimensions(raw_in));
    if (any(px >= size)) {
        return;
    }
    let z0 = textureLoad(raw_in, px, 0).x;
    if (z0 <= 0.0) {
        return;
    }
    let world = textureLoad(attrs_in, px, 0).x;
    let reach = world * view.proj[1][1] * view.viewport.y * 0.5 / z0;
    let radius = clamp(i32(ceil(reach)), 2, MAX_FILTER_PX);
    let sigma = max(min(reach, f32(MAX_FILTER_PX)) / (6.0 * FILTER_STRENGTH), 0.3);
    let range = max(0.75 * world, 1e-4);
    let dir = vec2i(pass_step.dir.xy);
    var smoothed = vec2f(0.0);
    var thick = vec4f(0.0);
    var weights = 0.0;
    for (var i = -radius; i <= radius; i++) {
        let q = clamp(px + dir * i, vec2i(0), size - 1);
        let z = textureLoad(raw_in, q, 0).x;
        if (z <= 0.0) {
            continue;
        }
        let r = f32(i) / sigma;
        let dz = (z - z0) / range;
        let w = exp(-0.5 * r * r - dz * dz);
        smoothed += textureLoad(smooth_in, q, 0).xy * w;
        thick += textureLoad(thick_in, q, 0) * w;
        weights += w;
    }
    let total = max(weights, 1e-6);
    textureStore(smooth_out, px, vec4f(smoothed / total, 0.0, 0.0));
    textureStore(thick_out, px, thick / total);
}

// The composite reads the smoothed depth and thickness, the frame as it stood before (the
// refraction's background), the sky and the sun's shadow.
@group(2) @binding(0) var background: texture_2d<f32>;
@group(2) @binding(1) var linear_sampler: sampler;
@group(2) @binding(2) var env_cube: texture_cube<f32>;
@group(2) @binding(3) var env_sampler: sampler;
@group(2) @binding(4) var<uniform> sh: array<vec4f, 9>;
@group(2) @binding(5) var shadow_map: texture_depth_2d_array;
@group(2) @binding(6) var shadow_sampler: sampler_comparison;

// The view-space point under pixel `px` at distance `z` (with the projection's jitter).
fn at(px: vec2f, z: f32) -> vec3f {
    let ndc = vec2f(px.x * view.viewport.z * 2.0 - 1.0, 1.0 - px.y * view.viewport.w * 2.0);
    return vec3f((ndc.x + view.proj[2][0]) * z / view.proj[0][0],
                 (ndc.y + view.proj[2][1]) * z / view.proj[1][1], -z);
}

fn sh_irradiance(n: vec3f) -> vec3f {
    let c1 = 0.429043; let c2 = 0.511664; let c3 = 0.743125; let c4 = 0.886227; let c5 = 0.247708;
    return max(vec3f(0.0),
        c1 * sh[8].xyz * (n.x * n.x - n.y * n.y) + c3 * sh[6].xyz * n.z * n.z + c4 * sh[0].xyz
        - c5 * sh[6].xyz + 2.0 * c1 * (sh[4].xyz * n.x * n.y + sh[7].xyz * n.x * n.z + sh[5].xyz * n.y * n.z)
        + 2.0 * c2 * (sh[3].xyz * n.x + sh[1].xyz * n.y + sh[2].xyz * n.z));
}

// The sun's shadow at a world point, one comparison tap of its cascade (as the spray's).
fn sun_shadow(world: vec3f) -> f32 {
    let view_depth = -(view.view * vec4f(world, 1.0)).z;
    if (view.sky_color.w < 0.5 || view_depth < 0.0) {
        return 1.0;
    }
    var c = 0u;
    for (var i = 0u; i < 4u; i++) {
        if (view_depth > view.cascade_splits[i]) {
            c = i + 1u;
        }
    }
    if (c >= 4u) {
        return 1.0;
    }
    let q = view.cascades[c] * vec4f(world, 1.0);
    let uv = q.xy * vec2f(0.5, -0.5) + 0.5;
    if (any(uv < vec2f(0.0)) || any(uv > vec2f(1.0)) || q.z > 1.0) {
        return 1.0;
    }
    return textureSampleCompareLevel(shadow_map, shadow_sampler, uv, c, q.z - 0.0005);
}

// How much of the colour at `world` the fog replaces (forward.wgsl's `fog_amount`).
fn fog_amount(world: vec3f) -> f32 {
    let density = view.fog.w;
    let d = distance(world, view.camera_pos.xyz);
    if (density < 0.0) {
        return clamp(1.0 - exp(density * d), 0.0, 1.0);
    }
    if (density == 0.0) {
        return 0.0;
    }
    let h = max(world.y, 0.0);
    return clamp(1.0 - exp(-density * d * exp(-h * 0.02)), 0.0, 1.0);
}

// The smoothed distance at `q`, or `fallback` where no liquid was drawn.
fn smoothed_at(q: vec2i, fallback: f32) -> f32 {
    let size = vec2i(textureDimensions(raw_in));
    let c = clamp(q, vec2i(0), size - 1);
    if (textureLoad(raw_in, c, 0).x <= 0.0) {
        return fallback;
    }
    return textureLoad(smooth_in, c, 0).x;
}

@fragment
fn composite_fs(in: Screen) -> @location(0) vec4f {
    let px = vec2i(in.clip.xy);
    let raw = textureLoad(raw_in, px, 0);
    if (raw.x <= 0.0) {
        discard;
    }
    let smoothed = textureLoad(smooth_in, px, 0);
    let z = select(raw.x, smoothed.x, smoothed.x > 0.0);
    let air = smoothed.y;
    let share = textureLoad(attrs_in, px, 0).y;
    let p = at(in.clip.xy, z);
    // The normal from the nearer of each pair of neighbours, so edges do not bleed (S. Lague's
    // NormalsFromDepth.shader).
    let pr = at(in.clip.xy + vec2f(1.0, 0.0), smoothed_at(px + vec2i(1, 0), z));
    let pl = at(in.clip.xy - vec2f(1.0, 0.0), smoothed_at(px - vec2i(1, 0), z));
    let pd = at(in.clip.xy + vec2f(0.0, 1.0), smoothed_at(px + vec2i(0, 1), z));
    let pu = at(in.clip.xy - vec2f(0.0, 1.0), smoothed_at(px - vec2i(0, 1), z));
    let ddx = select(p - pl, pr - p, abs(pr.z - p.z) < abs(p.z - pl.z));
    let ddy = select(pu - p, p - pd, abs(pd.z - p.z) < abs(p.z - pu.z));
    let rot = mat3x3f(view.view[0].xyz, view.view[1].xyz, view.view[2].xyz);
    let world = view.camera_pos.xyz + transpose(rot) * p;
    let to_eye = normalize(view.camera_pos.xyz - world);
    var n = normalize(transpose(rot) * normalize(cross(ddx, ddy)));
    if (dot(n, to_eye) < 0.0) {
        n = -n;
    }
    let nv = clamp(dot(n, to_eye), 0.0, 1.0);
    let fresnel = shallow_fresnel(nv, ETA);
    let thick = textureLoad(thick_in, px, 0);
    let length_m = thick.a;
    let transmit = exp(-thick.rgb);
    // What lies behind, seen along the ray refracted at the front surface and run through the
    // liquid's thickness (S. Lague's FluidRender.shader): where that lands off the frame, or on
    // something nearer than the liquid, the frame straight behind.
    var uv = in.clip.xy * view.viewport.zw;
    let bent = refract(-to_eye, n, ETA);
    if (dot(bent, bent) > 0.0) {
        let exit = world + bent * length_m * REFRACTION;
        let c = view.view_proj * vec4f(exit, 1.0);
        if (c.w > 0.0) {
            let e = c.xy / c.w;
            let euv = vec2f(e.x * 0.5 + 0.5, 0.5 - e.y * 0.5);
            let liquid = view.view_proj * vec4f(world, 1.0);
            let epx = min(vec2i(euv * view.viewport.xy), vec2i(view.viewport.xy) - 1);
            if (all(euv >= vec2f(0.0)) && all(euv <= vec2f(1.0))
                    && scene_depths(epx).x <= liquid.z / liquid.w) {
                uv = euv;
            }
        }
    }
    let behind = textureSampleLevel(background, linear_sampler, uv, 0.0).rgb;
    // The sky reflected, a little rough, and below the horizon the sky's ground (a drop's
    // underside faces down, unlike a flat surface); the sun's glint on a smooth surface (GGX,
    // a = 0.02).
    let r = reflect(-to_eye, n);
    let levels = f32(textureNumLevels(env_cube) - 1);
    let sky = textureSampleLevel(env_cube, env_sampler, r, 0.08 * levels).rgb * view.sun_color.w;
    let sun_dir = view.sun_dir.xyz;
    let sun = view.sun_dir.w * view.sun_color.rgb * sun_shadow(world);
    var glint = vec3f(0.0);
    if (view.sun_dir.w > 0.0) {
        let h = normalize(sun_dir + to_eye);
        let nl = max(dot(n, sun_dir), 0.0);
        let a = 0.02;
        glint = d_ggx(max(dot(n, h), 0.0), a) * v_smith(nv, nl, a)
            * f_schlick(vec3f(0.02), max(dot(h, to_eye), 0.0)) * sun * nl;
    }
    // Liquid full of air scatters white, lit by the sky above and the sun as a white surface is.
    let white = air * (1.0 - exp(-40.0 * length_m));
    let above = sh_irradiance(vec3f(0.0, 1.0, 0.0)) * view.sun_color.w / PI;
    let lit_white = above + sun * (0.5 + 0.5 * max(dot(n, sun_dir), 0.0)) / PI;
    // The light the liquid scatters toward the eye in the share it stops, a little turbid
    // (SCATTER per metre against its absorption): single scattering, the sky above and the sun
    // scattered evenly. Where it absorbs red, it scatters blue.
    let absorb = thick.rgb / max(length_m, 1e-4);
    let albedo = SCATTER / (absorb + SCATTER);
    let scattered = albedo * (1.0 - transmit) * (above + sun / (4.0 * PI));
    let clear = (behind * transmit + scattered) * (1.0 - fresnel) + sky * fresnel + glint;
    let color = mix(clear, lit_white, white);
    let fogged = mix(color, view.fog.rgb * view.sun_color.w, fog_amount(world));
    // Where the scene stands in front of some of the pixel's samples, only the rest show liquid.
    let ndc = view.view_proj * vec4f(world, 1.0);
    let liquid_z = ndc.z / ndc.w;
    var shown = 0u;
    for (var s = 0u; s < textureNumSamples(depth); s++) {
        if (textureLoad(depth, px, i32(s)) < liquid_z) {
            shown += 1u;
        }
    }
    let coverage = f32(shown) / f32(textureNumSamples(depth));
    let a = coverage * share * (1.0 - exp(-length_m / 0.001));
    return vec4f(fogged * a, a);
}
