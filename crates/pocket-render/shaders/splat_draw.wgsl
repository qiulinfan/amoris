// Drawing the sorted splats (docs/spec/splats.md 6): one quad per splat, back to front,
// spanning the ellipse's axes out to where the Gaussian falls below 1/255; the fragment evaluates
// the Gaussian and blends premultiplied into the HDR target. The quad sits at the splat center's
// depth, tested (never written) against the scene's depth, so meshes in front hide it.
//
// The same quads also draw into the entity-id pass (picking.rs, vs_splat_id and fs_splat_id): a
// pixel belongs to the nearest splat whose Gaussian reaches half opacity there, and the id names
// the splat's drawn cloud (SPLAT_PICK | cloud), resolved to its entity on the CPU.

@group(0) @binding(0) var<uniform> params: SplatParams;
@group(0) @binding(1) var<storage, read> projected: array<Projected>;
@group(0) @binding(2) var<storage, read> keys: array<u32>;
@group(0) @binding(3) var<storage, read> vals: array<u32>;
@group(0) @binding(4) var<storage, read> control: array<u32>;   // [0]: splats to draw
@group(0) @binding(5) var<storage, read> clouds: array<Cloud>;   // the id pass only

const SPLAT_PICK: u32 = 0x80000000u;   // splat/mod.rs SPLAT_PICK: ids of splat clouds

struct SplatOut {
    @builtin(position) clip: vec4f,
    @location(0) uv: vec2f,          // in standard deviations from the center
    @location(1) color: vec4f,       // linear rgb, opacity
};

// Instance `ii` draws quads ii * DRAW_BATCH onward; four vertices per quad (two triangles from the
// index buffer), so the vertex cache shades each corner once.
@vertex
fn vs_splat(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> SplatOut {
    return splat_corner(vi, ii * DRAW_BATCH + (vi >> 2u));
}

// Corner `vi & 3` of sorted splat `i`'s quad.
fn splat_corner(vi: u32, i: u32) -> SplatOut {
    var o: SplatOut;
    if (i >= control[0]) {
        o.clip = vec4f(2.0, 2.0, 2.0, 1.0);   // outside the clip volume: dropped
        return o;
    }
    let s = projected[vals[i]];
    let tz = key_depth(keys[i], params);
    let a1 = unpack2x16float(s.axes.x);
    let a2 = unpack2x16float(s.axes.y);
    let bo = unpack2x16float(s.color.y);
    let opacity = bo.y;
    let k = min(sqrt(max(2.0 * log(255.0 * opacity), 0.0)), 3.0);
    let corner = vec2f(f32(vi & 1u), f32((vi >> 1u) & 1u)) * 2.0 - 1.0;
    // Reversed-Z: z_ndc = (P22 z + P32) / -z with z = -tz.
    let z = (params.proj.w - params.proj.z * tz) / tz;
    o.clip = vec4f(s.center + (corner.x * a1 + corner.y * a2) * k, z, 1.0);
    o.uv = corner * k;
    o.color = vec4f(unpack2x16float(s.color.x), bo.x, opacity);
    return o;
}

@fragment
fn fs_splat(in: SplatOut) -> @location(0) vec4f {
    var a = min(in.color.a * exp2(-0.72134752 * dot(in.uv, in.uv)), 0.99);   // exp(-x/2) = 2^(-x/(2 ln 2))
    // Below 1/255 contributes nothing; no `discard` (blending a zero is free of side effects).
    a = select(a, 0.0, a < 1.0 / 255.0);
    return vec4f(in.color.rgb * a, a);
}

struct IdOut {
    @builtin(position) clip: vec4f,
    @location(0) uv: vec2f,
    @location(1) opacity: f32,
    @location(2) @interpolate(flat) id: u32,
};

// The drawn cloud thread `g` belongs to: the last whose `first` is <= g.
fn find_cloud(g: u32) -> u32 {
    var lo = 0u;
    var hi = params.counts.y;
    while (hi - lo > 1u) {
        let mid = (lo + hi) / 2u;
        if (clouds[mid].first <= g) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    return lo;
}

@vertex
fn vs_splat_id(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> IdOut {
    let i = ii * DRAW_BATCH + (vi >> 2u);
    let c = splat_corner(vi, i);
    var o: IdOut;
    o.clip = c.clip;
    o.uv = c.uv;
    o.opacity = c.color.a;
    if (i < control[0]) {
        o.id = SPLAT_PICK | find_cloud(vals[i]);
    }
    return o;
}

@fragment
fn fs_splat_id(in: IdOut) -> @location(0) u32 {
    if (in.opacity * exp2(-0.72134752 * dot(in.uv, in.uv)) < 0.5) {
        discard;
    }
    return in.id;
}
