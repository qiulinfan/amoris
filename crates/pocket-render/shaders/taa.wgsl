// Temporal anti-aliasing (docs/spec/taa-gtao.md). The opaque pass drew this frame with a sub-pixel
// jitter; this compute pass blends it with the history (the last output, reprojected):
//   - the 3x3 neighbourhood of every pixel comes from workgroup memory: each 8x8 group loads its
//     10x10 tile of this frame once, already tone-mapped and in YCoCg, with its depth (a fragment
//     pass doing nine loads and conversions per pixel cost 2.5 times as much on an RTX 5060);
//   - reprojection: the nearest depth of the 3x3 neighbourhood (so edges follow the foreground)
//     gives the surface's world position with this frame's unjittered matrices, last frame's
//     matrices give where it was, and the object motion the forward shader wrote adds the object's
//     own movement (skinning included). The sky reprojects as a direction (w = 0).
//   - the history is read with a 5-tap Catmull-Rom filter (sharper than bilinear);
//   - disocclusion (optional): the history keeps each pixel's view depth in alpha; when none of the
//     four texels around the reprojected position saw a surface near the depth this one had last
//     frame (5% plus the 3x3 neighbourhood's own depth spread, so slopes and edges do not count),
//     the history belongs to something else and is dropped;
//   - variance clipping (Salvi 2016) in YCoCg of tone-mapped values (c / (1 + luma), so bright
//     samples do not dominate): the history is clipped toward the box mean +- `gamma` sigmas of the
//     current 3x3 neighbourhood, intersected with its min/max;
//   - blend: weight max(1 / (frames since reset + 1), `alpha`) for the current frame, more when the
//     history moved by many pixels (resampling blurs it), in tone-mapped space.
// Output: the new history (rgb, view depth), which the rest of the frame reads, and when splats
// will be drawn over it a copy for them (`resolve_copy`), so they never enter the history.

struct Taa {
    inv_view_proj: mat4x4f,  // this frame, unjittered
    prev_view_proj: mat4x4f, // last frame, unjittered
    view_z: vec4f,           // row 2 of this frame's view matrix (view-space z of a point)
    prev_view_z: vec4f,      // row 2 of last frame's
    size: vec4f,             // width, height, 1/width, 1/height
    params: vec4f,           // x: least current weight, y: clip box half size in sigmas,
                             // z: mode (0 TAA, 1 this frame alone), w: frames since reset
    flags: vec4f,            // x: use object motion, y: disocclusion by depth, z: weight by speed,
                             // w: clip box size in sigmas where nothing moved (0: as elsewhere)
    blend: vec4f,            // x: current weight while the history is inside the box (0: always x)
};

@group(0) @binding(0) var<uniform> taa: Taa;
@group(0) @binding(1) var current: texture_2d<f32>;
@group(0) @binding(2) var history: texture_2d<f32>;
@group(0) @binding(3) var motion: texture_2d<f32>;
@group(0) @binding(4) var depth: texture_depth_multisampled_2d;
@group(0) @binding(5) var linear_sampler: sampler;
@group(0) @binding(6) var history_out: texture_storage_2d<rgba16float, write>;
@group(0) @binding(7) var image_out: texture_storage_2d<rgba16float, write>;

const FAR: f32 = 65000.0;
const GROUP: i32 = 8;
const TILE: i32 = 10;

var<workgroup> tile_color: array<vec3f, 100>;
var<workgroup> tile_depth: array<f32, 100>;

fn tonemap(c: vec3f) -> vec3f {
    return c / (1.0 + luminance(c));
}

fn untonemap(c: vec3f) -> vec3f {
    return c / max(1.0 - luminance(c), 1e-4);
}

fn to_ycocg(c: vec3f) -> vec3f {
    return vec3f(0.25 * c.r + 0.5 * c.g + 0.25 * c.b, 0.5 * c.r - 0.5 * c.b,
        -0.25 * c.r + 0.5 * c.g - 0.25 * c.b);
}

fn from_ycocg(c: vec3f) -> vec3f {
    return vec3f(c.x + c.y - c.z, c.x + c.z, c.x - c.y - c.z);
}

// The history at `uv` through a Catmull-Rom filter, in 5 bilinear taps (the corners dropped).
fn history_catmull_rom(uv: vec2f) -> vec3f {
    let size = taa.size.xy;
    let p = uv * size;
    let t1 = floor(p - 0.5) + 0.5;
    let f = p - t1;
    let w0 = f * (-0.5 + f * (1.0 - 0.5 * f));
    let w1 = 1.0 + f * f * (-2.5 + 1.5 * f);
    let w2 = f * (0.5 + f * (2.0 - 1.5 * f));
    let w3 = f * f * (-0.5 + 0.5 * f);
    let w12 = w1 + w2;
    let o12 = w2 / w12;
    let t0 = (t1 - 1.0) * taa.size.zw;
    let t3 = (t1 + 2.0) * taa.size.zw;
    let t12 = (t1 + o12) * taa.size.zw;
    var s = textureSampleLevel(history, linear_sampler, vec2f(t12.x, t0.y), 0.0).rgb * (w12.x * w0.y);
    s += textureSampleLevel(history, linear_sampler, vec2f(t0.x, t12.y), 0.0).rgb * (w0.x * w12.y);
    s += textureSampleLevel(history, linear_sampler, t12, 0.0).rgb * (w12.x * w12.y);
    s += textureSampleLevel(history, linear_sampler, vec2f(t3.x, t12.y), 0.0).rgb * (w3.x * w12.y);
    s += textureSampleLevel(history, linear_sampler, vec2f(t12.x, t3.y), 0.0).rgb * (w12.x * w3.y);
    let w = w12.x * w0.y + w0.x * w12.y + w12.x * w12.y + w3.x * w12.y + w12.x * w3.y;
    return max(s / w, vec3f(0.0));
}

// Clips `h` toward the centre of the box [lo, hi] (Playdead's clip_aabb).
fn clip_box(h: vec3f, lo: vec3f, hi: vec3f) -> vec3f {
    let c = 0.5 * (hi + lo);
    let e = 0.5 * (hi - lo) + 1e-5;
    let v = h - c;
    let u = abs(v / e);
    let m = max(u.x, max(u.y, u.z));
    return select(h, c + v / m, m > 1.0);
}

// The view depth of the point at `ndc` and device depth `d` with the matrix rows `z` (with the
// infinite projection depth 0 is a direction: FAR).
fn view_depth(ndc: vec2f, d: f32, z: vec4f) -> f32 {
    let p = taa.inv_view_proj * vec4f(ndc, d, 1.0);
    if (abs(p.w) <= 1e-9) {
        return FAR;
    }
    return min(-dot(z, vec4f(p.xyz / p.w, 1.0)), FAR);
}

// Loads the group's 10x10 tile (its 8x8 pixels and a border of one) into workgroup memory. Every
// invocation calls it (it ends with a barrier).
fn load_tile(group: vec2i, lid: u32) {
    let size = vec2i(taa.size.xy);
    let origin = group * GROUP - 1;
    for (var i = i32(lid); i < TILE * TILE; i += GROUP * GROUP) {
        let q = clamp(origin + vec2i(i % TILE, i / TILE), vec2i(0), size - 1);
        tile_color[i] = to_ycocg(tonemap(textureLoad(current, q, 0).rgb));
        tile_depth[i] = textureLoad(depth, q, 0);
    }
    workgroupBarrier();
}

// Pixel `p`'s new history: rgb, and its view depth in alpha. `local` is its place in the group.
fn resolve(p: vec2i, local: vec2i) -> vec4f {
    // The 3x3 neighbourhood: its moments, its extremes, its nearest and farthest depth.
    var m1 = vec3f(0.0);
    var m2 = vec3f(0.0);
    var lo = vec3f(1e9);
    var hi = vec3f(-1e9);
    var nearest = -1.0;
    var farthest = 2.0;
    var nearest_off = vec2i(0);
    for (var k = 0; k < 9; k++) {
        let o = vec2i(k % 3 - 1, k / 3 - 1);
        let i = (local.y + 1 + o.y) * TILE + local.x + 1 + o.x;
        let y = tile_color[i];
        m1 += y;
        m2 += y * y;
        lo = min(lo, y);
        hi = max(hi, y);
        // Reversed-Z: nearer is larger.
        let d = tile_depth[i];
        if (d > nearest) {
            nearest = d;
            nearest_off = o;
        }
        farthest = min(farthest, d);
    }
    let centre = textureLoad(current, p, 0).rgb;
    let uv = (vec2f(p) + 0.5) * taa.size.zw;
    let ndc = vec2f(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    // The surface (or, at depth 0 with the infinite projection, the direction) this pixel shows.
    let world = taa.inv_view_proj * vec4f(ndc, nearest, 1.0);
    let depth_now = view_depth(ndc, nearest, taa.view_z);
    if (u32(taa.params.z) == 1u) {
        return vec4f(centre, depth_now);
    }
    let then = taa.prev_view_proj * world;
    var prev_ndc = then.xy / then.w;
    if (taa.flags.x > 0.5) {
        let q = clamp(p + nearest_off, vec2i(0), vec2i(taa.size.xy) - 1);
        prev_ndc += textureLoad(motion, q, 0).xy;
    }
    let prev_uv = vec2f(prev_ndc.x * 0.5 + 0.5, 0.5 - prev_ndc.y * 0.5);
    var valid = then.w > 0.0 && all(prev_uv >= vec2f(0.0)) && all(prev_uv <= vec2f(1.0));
    if (valid && taa.flags.y > 0.5) {
        let expected = view_depth(ndc, nearest, taa.prev_view_z);
        let spread = view_depth(ndc, farthest, taa.view_z) - depth_now;
        if (expected < FAR && spread < FAR * 0.5) {
            let seen = textureGather(3, history, linear_sampler, prev_uv);
            let off = min(min(abs(seen.x - expected), abs(seen.y - expected)),
                min(abs(seen.z - expected), abs(seen.w - expected)));
            valid = off <= 0.05 * expected + spread;
        }
    }
    if (!valid) {
        return vec4f(centre, depth_now);
    }
    let h = history_catmull_rom(prev_uv);
    let mean = m1 / 9.0;
    let sigma = sqrt(max(m2 / 9.0 - mean * mean, vec3f(0.0)));
    let speed = length((uv - prev_uv) * taa.size.xy);
    var gamma = taa.params.y;
    var box_lo = lo;
    var box_hi = hi;
    if (taa.flags.w > 0.0 && speed < 0.01) {
        // Nothing moved here: a wider box keeps sub-pixel detail that only some jitter positions
        // see (thin wires), at the cost of lag where only the lighting changes.
        gamma = taa.flags.w;
        box_lo = vec3f(-1e9);
        box_hi = vec3f(1e9);
    }
    box_lo = max(box_lo, mean - gamma * sigma);
    box_hi = min(box_hi, mean + gamma * sigma);
    let hy = to_ycocg(tonemap(h));
    let clipped = from_ycocg(clip_box(hy, box_lo, box_hi));
    var alpha = max(1.0 / (taa.params.w + 1.0), taa.params.x);
    if (taa.blend.x > 0.0) {
        // A history inside the box agrees with this frame: a longer window there averages more
        // jitter positions; one clipped far outside it takes the usual weight.
        let e = 0.5 * (box_hi - box_lo) + 1e-5;
        let u = abs(hy - 0.5 * (box_hi + box_lo)) / e;
        let outside = clamp(max(u.x, max(u.y, u.z)) - 1.0, 0.0, 1.0);
        alpha = max(1.0 / (taa.params.w + 1.0), mix(taa.blend.x, taa.params.x, outside));
    }
    if (taa.flags.z > 0.5) {
        // Resampling a moving history blurs it: trust the current frame a little more.
        alpha = mix(alpha, max(alpha, 0.25), clamp(speed / 8.0, 0.0, 1.0));
    }
    return vec4f(untonemap(mix(clipped, tonemap(centre), alpha)), depth_now);
}

@compute @workgroup_size(8, 8)
fn resolve_history(@builtin(global_invocation_id) id: vec3u,
        @builtin(workgroup_id) group: vec3u, @builtin(local_invocation_index) lid: u32,
        @builtin(local_invocation_id) local: vec3u) {
    load_tile(vec2i(group.xy), lid);
    if (any(id.xy >= vec2u(taa.size.xy))) {
        return;
    }
    textureStore(history_out, id.xy, resolve(vec2i(id.xy), vec2i(local.xy)));
}

// With splats: the history, and a copy the splats are drawn into.
@compute @workgroup_size(8, 8)
fn resolve_copy(@builtin(global_invocation_id) id: vec3u,
        @builtin(workgroup_id) group: vec3u, @builtin(local_invocation_index) lid: u32,
        @builtin(local_invocation_id) local: vec3u) {
    load_tile(vec2i(group.xy), lid);
    if (any(id.xy >= vec2u(taa.size.xy))) {
        return;
    }
    let r = resolve(vec2i(id.xy), vec2i(local.xy));
    textureStore(history_out, id.xy, r);
    textureStore(image_out, id.xy, vec4f(r.rgb, 1.0));
}
