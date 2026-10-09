// The compute tile rasterizer (docs/spec/splats.md 4.2; splat/tile.rs): the alternative to the
// quad draw. After the preprocess and the depth sort (back to front), the visible splats are
// binned into 16x16-pixel screen tiles and each tile is blended front to back by one workgroup that
// stops once every pixel is saturated or has reached the scene's depth.
//
//   tile_setup   (1 thread) the indirect dispatch over the binned splats: the visible ones, at most
//                as many as the per-splat buffers hold (the farthest beyond are left out);
//   tile_count   one thread per binned splat in front-to-back order p: its record for the
//                rasterizer, and the tiles its quad overlaps (the quad's bounding box, each tile
//                tested exactly against the oriented quad); per workgroup the number of pairs;
//   tile_scan    (1 workgroup) the workgroups' pair counts become offsets; the pair count, clamped
//                to the pair buffers' capacity, and the sort's dispatch go to the tile control;
//   tile_emit    each splat writes (tile, p) for each tile it overlaps at its offset; pairs past the
//                capacity are dropped (the farthest splats': p runs front to back);
//   (sort)       the portable radix sort (splat_sort.wgsl) orders the pairs by tile, stably, so
//                each tile's splats stay front to back;
//   tile_ranges  each tile's first and last pair;
//   tile_raster  one workgroup per tile, one thread per pixel.
//
// Only WebGPU core: no subgroups, no 64-bit atomics, 13 KiB of workgroup memory, at most 7 storage
// buffers per kernel, an rgba16float write-only storage texture. `params.tiles` is (tiles across,
// tiles down, pair capacity, splat capacity of the per-splat buffers).

const TILE_PX: u32 = 16u;          // tile.rs TILE_PX
const WG: u32 = 256u;
const PAIR_TILE: u32 = 4096u;      // pairs per sort tile and per tile_ranges workgroup (SORT_TILE)
const NO_TILE: u32 = 0xffffffffu;  // a pair to ignore: sorts after every tile

// One visible splat in front-to-back order, as the rasterizer reads it (32 bytes).
struct TileSplat {
    center: vec2f,       // pixels (y down)
    inv_axes: vec2u,     // f16 pairs: axis / |axis|^2 for both ellipse axes, pixels
    color: vec2u,        // f16 pairs: (r, g), (b, opacity), linear
    z: f32,              // the reversed-Z depth the quad path tests (from the sort key)
    k: f32,              // the quad's half extent in standard deviations
};

@group(0) @binding(0) var<uniform> params: SplatParams;
// [0]: visible splats (written by the preprocess).
@group(0) @binding(1) var<storage, read> control: array<u32>;
// [0] pairs to sort (at most the capacity), [1] pairs wanted, [2] splats binned, [4..6] the sort's
// and tile_ranges' dispatch, [8..10] the dispatch over the binned splats (x, y, 1). [0..4] are read
// back (splat/mod.rs SplatStats).
@group(0) @binding(2) var<storage, read_write> tile_control: array<u32>;
@group(0) @binding(3) var<storage, read> keys: array<u32>;          // depth-sorted, back to front
@group(0) @binding(4) var<storage, read> vals: array<u32>;
@group(0) @binding(5) var<storage, read> projected: array<Projected>;
// Per splat p: the tile rectangle (x0 | y0 << 16, x1 | y1 << 16) and its count of overlapped tiles.
@group(0) @binding(6) var<storage, read_write> rects: array<vec4u>;
@group(0) @binding(7) var<storage, read_write> tsplats: array<TileSplat>;
@group(0) @binding(8) var<storage, read_write> blocks: array<u32>;
@group(0) @binding(9) var<storage, read_write> pair_keys: array<u32>;
@group(0) @binding(10) var<storage, read_write> pair_vals: array<u32>;
// Read-only views for kernels dispatched indirectly from the tile control or reading sorted pairs.
@group(0) @binding(11) var<storage, read> tile_control_ro: array<u32>;
@group(0) @binding(12) var<storage, read> sorted_keys: array<u32>;
@group(0) @binding(13) var<storage, read> sorted_vals: array<u32>;
@group(0) @binding(14) var<storage, read_write> ranges: array<vec2u>;
@group(0) @binding(15) var<storage, read> ranges_ro: array<vec2u>;
@group(0) @binding(16) var<storage, read> tsplats_ro: array<TileSplat>;
@group(0) @binding(17) var scene_depth: texture_depth_multisampled_2d;
@group(0) @binding(18) var out_image: texture_storage_2d<rgba16float, write>;
// The raster's (pixel, splat) tests this frame, low and high words, when SPLAT_COUNT_TESTS is set:
// its work, read back.
@group(0) @binding(19) var<storage, read_write> visits: array<atomic<u32>, 2>;

var<workgroup> wg_scan: array<u32, 256>;
var<workgroup> wg_sum: atomic<u32>;

// The exclusive prefix sum of `v` over the workgroup (Hillis-Steele; uniform control flow only).
fn exclusive_scan(t: u32, v: u32) -> u32 {
    wg_scan[t] = v;
    workgroupBarrier();
    for (var o = 1u; o < WG; o = o << 1u) {
        var x = 0u;
        if (t >= o) {
            x = wg_scan[t - o];
        }
        workgroupBarrier();
        wg_scan[t] = wg_scan[t] + x;
        workgroupBarrier();
    }
    return wg_scan[t] - v;
}

// The quad's half extents along x and y (pixels) from its inverse axes (axis = ia / |ia|^2).
fn quad_extent(ia1: vec2f, ia2: vec2f, k: f32) -> vec2f {
    let l1 = dot(ia1, ia1);
    let l2 = dot(ia2, ia2);
    if (l1 == 0.0 || l2 == 0.0) {
        return vec2f(1e30);   // an axis too long for f16: covers everything
    }
    return k * (abs(ia1 / l1) + abs(ia2 / l2));
}

// Whether the quad (center `c`, inverse axes, half extent `k` standard deviations, bounding half
// extents `ext`) overlaps the axis-aligned box of half sizes `h` at `b`: separating axes x, y and
// the quad's two axes (in standard deviations along each, the box projects to dot(h, |ia|)).
fn quad_overlaps(c: vec2f, ia1: vec2f, ia2: vec2f, k: f32, ext: vec2f, b: vec2f, h: vec2f) -> bool {
    let d = b - c;
    if (any(abs(d) > ext + h)) {
        return false;
    }
    return abs(dot(d, ia1)) <= k + dot(h, abs(ia1)) && abs(dot(d, ia2)) <= k + dot(h, abs(ia2));
}

// Pixel centers of a tile lie within 7.5 pixels of its middle.
fn tile_middle(x: u32, y: u32) -> vec2f {
    return vec2f(f32(x * TILE_PX) + 8.0, f32(y * TILE_PX) + 8.0);
}

// The splats binned this frame: the visible ones, front to back, up to what the per-splat buffers
// hold (they are sized below the device's storage binding limit; tile.rs).
fn binned() -> u32 {
    return min(control[0], params.tiles.w);
}

@compute @workgroup_size(1)
fn tile_setup() {
    let n = binned();
    let groups = (n + WG - 1u) / WG;
    let gx = min(groups, 65535u);
    tile_control[2] = n;
    tile_control[8] = gx;
    tile_control[9] = (groups + 65534u) / 65535u;
    tile_control[10] = 1u;
}

@compute @workgroup_size(256)
fn tile_count(
    @builtin(workgroup_id) wid: vec3u,
    @builtin(num_workgroups) groups: vec3u,
    @builtin(local_invocation_index) lid: u32,
) {
    if (lid == 0u) {
        atomicStore(&wg_sum, 0u);
    }
    workgroupBarrier();
    let block = wid.y * groups.x + wid.x;
    let p = block * WG + lid;
    let n = binned();
    if (p < n) {
        let i = control[0] - 1u - p;              // front to back
        let s = projected[vals[i]];
        let w = params.viewport.x;
        let h = params.viewport.y;
        let center = vec2f((s.center.x + 1.0) * 0.5 * w, (1.0 - s.center.y) * 0.5 * h);
        let n1 = unpack2x16float(s.axes.x);
        let n2 = unpack2x16float(s.axes.y);
        let a1 = vec2f(n1.x * 0.5 * w, -n1.y * 0.5 * h);
        let a2 = vec2f(n2.x * 0.5 * w, -n2.y * 0.5 * h);
        let bo = unpack2x16float(s.color.y);
        let k = min(sqrt(max(2.0 * log(255.0 * bo.y), 0.0)), 3.0);
        let tz = key_depth(keys[i], params);
        var t: TileSplat;
        t.center = center;
        t.inv_axes = vec2u(
            pack2x16float(a1 / max(dot(a1, a1), 1e-12)),
            pack2x16float(a2 / max(dot(a2, a2), 1e-12)),
        );
        t.color = s.color;
        t.z = (params.proj.w - params.proj.z * tz) / tz;
        t.k = k;
        tsplats[p] = t;
        // The tiles of the quad's bounding box (clamped to the screen) that the quad overlaps,
        // tested with the f16 axes the rasterizer and tile_emit read.
        let ia1 = unpack2x16float(t.inv_axes.x);
        let ia2 = unpack2x16float(t.inv_axes.y);
        let ext = quad_extent(ia1, ia2, k);
        let tiles = vec2f(f32(params.tiles.x), f32(params.tiles.y));
        let lo = max(floor((center - ext) / f32(TILE_PX)), vec2f(0.0));
        let hi = min(floor((center + ext) / f32(TILE_PX)), tiles - 1.0);
        var r = vec4u(1u, 0u, 0u, 0u);
        if (all(lo <= hi)) {
            let l = vec2u(lo);
            let u = vec2u(hi);
            var c = 0u;
            for (var y = l.y; y <= u.y; y++) {
                for (var x = l.x; x <= u.x; x++) {
                    c += u32(quad_overlaps(center, ia1, ia2, k, ext, tile_middle(x, y), vec2f(7.5)));
                }
            }
            r = vec4u(l.x | (l.y << 16u), u.x | (u.y << 16u), c, 0u);
        }
        rects[p] = r;
        atomicAdd(&wg_sum, r.z);
    }
    workgroupBarrier();
    if (lid == 0u && block * WG < n) {
        blocks[block] = atomicLoad(&wg_sum);
    }
}

@compute @workgroup_size(256)
fn tile_scan(@builtin(local_invocation_index) t: u32) {
    let n = binned();
    let nb = (n + WG - 1u) / WG;
    let per = (nb + WG - 1u) / WG;
    let lo = min(t * per, nb);
    let hi = min(lo + per, nb);
    var sum = 0u;
    for (var i = lo; i < hi; i++) {
        sum += blocks[i];
    }
    var run = exclusive_scan(t, sum);
    for (var i = lo; i < hi; i++) {
        let c = blocks[i];
        blocks[i] = run;
        run += c;
    }
    if (t == WG - 1u) {
        let m = min(run, params.tiles.z);
        tile_control[0] = m;
        tile_control[1] = run;
        tile_control[4] = (m + PAIR_TILE - 1u) / PAIR_TILE;
        tile_control[5] = 1u;
        tile_control[6] = 1u;
    }
}

// Writes exactly the count tile_count stored: should the test disagree (floating-point contraction
// differs between kernels), surplus tiles are dropped and missing ones padded with NO_TILE.
@compute @workgroup_size(256)
fn tile_emit(
    @builtin(workgroup_id) wid: vec3u,
    @builtin(num_workgroups) groups: vec3u,
    @builtin(local_invocation_index) lid: u32,
) {
    let block = wid.y * groups.x + wid.x;
    let p = block * WG + lid;
    let n = binned();
    var r = vec4u(1u, 0u, 0u, 0u);
    if (p < n) {
        r = rects[p];
    }
    let local = exclusive_scan(lid, r.z);
    if (r.z == 0u) {
        return;
    }
    let s = tsplats[p];
    let ia1 = unpack2x16float(s.inv_axes.x);
    let ia2 = unpack2x16float(s.inv_axes.y);
    let ext = quad_extent(ia1, ia2, s.k);
    let cap = params.tiles.z;
    var o = blocks[block] + local;
    let end = min(o + r.z, cap);
    let x0 = r.x & 0xffffu;
    let x1 = r.y & 0xffffu;
    for (var y = r.x >> 16u; y <= (r.y >> 16u); y++) {
        for (var x = x0; x <= x1; x++) {
            if (o >= end) {
                return;
            }
            if (quad_overlaps(s.center, ia1, ia2, s.k, ext, tile_middle(x, y), vec2f(7.5))) {
                pair_keys[o] = y * params.tiles.x + x;
                pair_vals[o] = p;
                o++;
            }
        }
    }
    for (; o < end; o++) {
        pair_keys[o] = NO_TILE;
        pair_vals[o] = p;
    }
}

@compute @workgroup_size(256)
fn tile_ranges(@builtin(workgroup_id) wid: vec3u, @builtin(local_invocation_index) lid: u32) {
    let m = tile_control_ro[0];
    let tiles = arrayLength(&ranges);
    for (var r = 0u; r < PAIR_TILE / WG; r++) {
        let q = wid.x * PAIR_TILE + r * WG + lid;
        if (q < m) {
            let t = sorted_keys[q];
            if (t < tiles) {
                if (q == 0u || sorted_keys[q - 1u] != t) {
                    ranges[t].x = q;
                }
                if (q + 1u == m || sorted_keys[q + 1u] != t) {
                    ranges[t].y = q + 1u;
                }
            }
        }
    }
}

var<workgroup> s_pos: array<vec4f, 256>;     // center (px), z, k
var<workgroup> s_axes: array<vec4f, 256>;    // inverse axes
var<workgroup> s_color: array<vec4f, 256>;   // linear rgb, opacity
// Per 8x4-pixel sub-tile, 256 bits: bit j of word [sub * 8 + j / 32] when batch splat j's quad
// overlaps it.
var<workgroup> s_bits: array<atomic<u32>, 64>;
var<workgroup> wg_done: atomic<u32>;
var<workgroup> wg_flag: u32;
var<workgroup> wg_range: vec2u;
var<workgroup> wg_visits: array<atomic<u32>, 2>;

// One workgroup per tile, one thread per pixel: the tile's splats front to back in batches of 256
// staged in workgroup memory, with one 256-bit list per 8x4-pixel sub-tile of the splats whose
// quads overlap it. Invocations 32s..32s+31 are sub-tile s, so on 32-wide hardware a SIMD group
// walks one list in step (its reads of a splat are broadcasts). A pixel stops at the first splat
// behind the scene's depth (every later one is farther) or once its transmittance falls below
// 1/255; the workgroup stops when all its pixels have. Writes (premultiplied color,
// transmittance), composited by splat_composite.wgsl. With SPLAT_COUNT_TESTS it adds the (pixel,
// splat) pairs its pixels tested to `visits` (one global atomic per workgroup; off by default: the
// workgroup sum cost about 6% of the raster on the RTX 5060).
@compute @workgroup_size(256)
fn tile_raster(@builtin(workgroup_id) wid: vec3u, @builtin(local_invocation_index) lid: u32) {
    let sub = lid >> 5u;
    let local = vec2u((sub & 1u) * 8u + (lid & 7u), (sub >> 1u) * 4u + ((lid >> 3u) & 3u));
    let px = wid.xy * TILE_PX + local;
    let inside = all(vec2f(px) < params.viewport.xy);
    var zo = 0.0;   // reversed-Z: 0 is the far plane
    if (inside) {
        zo = textureLoad(scene_depth, vec2i(px), 0);
    }
    let pix = vec2f(px) + 0.5;
    let origin = vec2f(wid.xy * TILE_PX);
    var color = vec3f(0.0);
    var trans = 1.0;
    var done = !inside;
    var tested = 0u;
    if (lid == 0u) {
        atomicStore(&wg_done, 0u);
        atomicStore(&wg_visits[0], 0u);
        atomicStore(&wg_visits[1], 0u);
        wg_range = ranges_ro[wid.y * params.tiles.x + wid.x];
    }
    workgroupBarrier();
    if (done) {
        atomicAdd(&wg_done, 1u);
    }
    let range = workgroupUniformLoad(&wg_range);
    for (var b = range.x; b < range.y; b += WG) {
        if (lid < 64u) {
            atomicStore(&s_bits[lid], 0u);
        }
        workgroupBarrier();
        if (lid == 0u) {
            wg_flag = atomicLoad(&wg_done);
        }
        if (workgroupUniformLoad(&wg_flag) >= WG) {
            break;
        }
        let q = b + lid;
        if (q < range.y) {
            let s = tsplats_ro[sorted_vals[q]];
            let ia1 = unpack2x16float(s.inv_axes.x);
            let ia2 = unpack2x16float(s.inv_axes.y);
            s_pos[lid] = vec4f(s.center, s.z, s.k);
            s_axes[lid] = vec4f(ia1, ia2);
            s_color[lid] = vec4f(unpack2x16float(s.color.x), unpack2x16float(s.color.y));
            let ext = quad_extent(ia1, ia2, s.k);
            let bit = 1u << (lid & 31u);
            for (var m = 0u; m < 8u; m++) {
                let mid = origin + vec2f(f32(m & 1u) * 8.0 + 4.0, f32(m >> 1u) * 4.0 + 2.0);
                if (quad_overlaps(s.center, ia1, ia2, s.k, ext, mid, vec2f(3.5, 1.5))) {
                    atomicOr(&s_bits[m * 8u + (lid >> 5u)], bit);
                }
            }
        }
        workgroupBarrier();
        if (!done) {
            for (var w = 0u; w < 8u; w++) {
                var bits = atomicLoad(&s_bits[sub * 8u + w]);
                while (bits != 0u) {
                    let j = w * 32u + firstTrailingBit(bits);
                    bits &= bits - 1u;
                    tested += 1u;
                    let a = s_pos[j];
                    if (a.z <= zo) {
                        done = true;   // behind the scene's surface, as is every later splat
                        break;
                    }
                    let d = pix - a.xy;
                    let ia = s_axes[j];
                    let uv = vec2f(dot(d, ia.xy), dot(d, ia.zw));
                    if (max(abs(uv.x), abs(uv.y)) > a.w) {
                        continue;      // outside the quad the quad path would draw
                    }
                    let c = s_color[j];
                    let alpha = min(c.a * exp2(-0.72134752 * dot(uv, uv)), 0.99);
                    if (alpha < 1.0 / 255.0) {
                        continue;
                    }
                    color += c.rgb * (alpha * trans);
                    trans *= 1.0 - alpha;
                    if (trans < 1.0 / 255.0) {
                        done = true;
                        break;
                    }
                }
                if (done) {
                    break;
                }
            }
            if (done) {
                atomicAdd(&wg_done, 1u);
            }
        }
        workgroupBarrier();
    }
    // The workgroup's tests into the frame's 64-bit count (a carry wherever a low word wraps); the
    // flag is uniform, so the barrier may sit in the branch.
    if ((params.counts.w & SPLAT_COUNT_TESTS) != 0u) {
        let o = atomicAdd(&wg_visits[0], tested);
        if (o + tested < o) {
            atomicAdd(&wg_visits[1], 1u);
        }
        workgroupBarrier();
        if (lid == 0u) {
            let lo = atomicLoad(&wg_visits[0]);
            let hi = atomicLoad(&wg_visits[1]);
            if ((lo | hi) != 0u) {
                let g = atomicAdd(&visits[0], lo);
                atomicAdd(&visits[1], hi + u32(g + lo < g));
            }
        }
    }
    if (inside) {
        textureStore(out_image, vec2i(px), vec4f(color, trans));
    }
}
