// GPU culling: one thread per instance tests it against every view (the camera's frustum and each
// shadow cascade's box) and appends its index to that view's visible list in its mesh's region,
// counting the instances of each indirect draw. Counting is two-level: threads of a workgroup
// whose instance has the workgroup's common mesh add to a workgroup counter first, so a scene of
// one mesh pays one global atomic per workgroup and view instead of one per instance.
//
// With occlusion culling on (docs/spec/occlusion.md) the camera view is culled in two phases:
// `main` (the early pass) keeps only the instances visible at the end of the last frame, which are
// drawn and give the depth pyramid (hiz.wgsl); `late` then tests every instance in the frustum
// against the pyramid, draws the visible ones the early pass did not draw (through the late
// arguments, into the same regions of `drawn`, which the early draws no longer need) and records
// what is visible for the next frame. The shadow cascades keep frustum culling only.

struct Planes {
    p: array<vec4f, 6>,
};

struct Cull {
    planes: array<Planes, 5>,
    view_proj: mat4x4f,      // the camera's (the occlusion test)
    instance_count: u32,
    view_count: u32,
    mesh_count: u32,
    view_stride: u32,        // entries per view in the visible list
    alpha: f32,
    lod_scale: f32,          // unused yet: screen-size LOD selection
    occlusion: u32,          // 1: two-phase occlusion culling this frame
    hiz_levels: u32,         // the depth pyramid's levels
    viewport: vec2f,         // the depth target's size, pixels
    _pad0: u32,
    _pad1: u32,
};

struct DrawArgs {
    index_count: u32,
    instance_count: atomic<u32>,
    first_index: u32,
    base_vertex: i32,
    first_instance: u32,
};

@group(0) @binding(0) var<uniform> cull: Cull;
@group(0) @binding(1) var<storage, read> instances: array<Instance>;
@group(0) @binding(2) var<storage, read> meshes: array<MeshInfo>;
@group(0) @binding(3) var<storage, read_write> draws: array<DrawArgs>;
@group(0) @binding(4) var<storage, read_write> visible: array<u32>;
// Each batch's region in a view's visible list; a batch is (variant, mesh) at variant * meshes + mesh.
@group(0) @binding(5) var<storage, read> batch_offsets: array<u32>;
// The camera view's visible instances with their interpolated poses (view 0 writes here instead
// of `visible`).
@group(0) @binding(6) var<storage, read_write> drawn: array<Drawn>;
// Per instance slot, the occlusion state: VIS_VISIBLE between frames (written by `late`),
// VIS_FRUSTUM | VIS_EARLY between the two passes of a frame (written by `main`).
@group(0) @binding(7) var<storage, read_write> state: array<u32>;
// The depth pyramid (late pass only).
@group(0) @binding(8) var hiz: texture_2d<f32>;
// What the late pass found (late pass only): instances in the frustum, occluded, drawn early,
// drawn late; triangles in the frustum and occluded, each a (low, high) pair of words.
@group(0) @binding(9) var<storage, read_write> stats: array<atomic<u32>, 8>;

const VIS_VISIBLE: u32 = 1u;
const VIS_FRUSTUM: u32 = 2u;
const VIS_EARLY: u32 = 4u;
// The late arguments' set among the draw arguments: after the camera and the four cascades.
const LATE: u32 = 5u;

var<workgroup> wg_batch: u32;
var<workgroup> wg_count: array<atomic<u32>, 5>;
var<workgroup> wg_base: array<u32, 5>;
var<workgroup> wg_stats: array<atomic<u32>, 6>;

fn sphere_in(view: u32, c: vec3f, r: f32) -> bool {
    for (var i = 0u; i < 6u; i++) {
        let pl = cull.planes[view].p[i];
        if (dot(pl.xyz, c) + pl.w < -r) {
            return false;
        }
    }
    return true;
}

// The flattened workgroup index (dispatches wider than 65535 groups wrap into y).
fn group_first(wid: vec3u, nwg: vec3u) -> u32 {
    return (wid.x + wid.y * nwg.x) * 256u;
}

// The batch of the workgroup's first instance (the one most of its instances share), or none.
fn first_batch(first: u32) -> u32 {
    if (first < cull.instance_count) {
        let f = instances[first];
        return ((f.flags >> VARIANT_SHIFT) & 3u) * cull.mesh_count + f.mesh;
    }
    return 0xffffffffu;
}

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) gid: vec3u, @builtin(local_invocation_index) lid: u32,
        @builtin(workgroup_id) wid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let idx = gid.x + gid.y * nwg.x * 256u;
    if (lid == 0u) {
        wg_batch = first_batch(group_first(wid, nwg));
        for (var v = 0u; v < 5u; v++) {
            atomicStore(&wg_count[v], 0u);
        }
    }
    workgroupBarrier();

    var inside: array<bool, 5>;
    var local_slot: array<u32, 5>;
    var mesh = 0xffffffffu;
    var batch = 0xffffffffu;
    var live = false;
    var d: Drawn;
    if (idx < cull.instance_count) {
        let inst = instances[idx];
        let need = FLAG_ALIVE | FLAG_VISIBLE;
        var occl = 0u;
        if ((inst.flags & need) == need && inst.mesh < cull.mesh_count) {
            live = true;
            mesh = inst.mesh;
            batch = ((inst.flags >> VARIANT_SHIFT) & 3u) * cull.mesh_count + mesh;
            let m = meshes[mesh];
            let pose = instance_pose(inst, cull.alpha);
            d.pos = pose.pos;
            d.rot = pose.rot;
            d.scale = inst.scale;
            d.material = inst.material;
            d.slot = idx;
            let c = pose.pos + quat_rotate(pose.rot, m.center * inst.scale);
            let s = abs(inst.scale);
            let r = m.radius * max(s.x, max(s.y, s.z));
            for (var v = 0u; v < cull.view_count; v++) {
                let shadow_ok = v == 0u || (inst.flags & FLAG_SHADOW) != 0u;
                var in_view = shadow_ok && sphere_in(v, c, r);
                if (v == 0u && cull.occlusion != 0u) {
                    // The early pass draws what was visible at the end of the last frame.
                    let was = (state[idx] & VIS_VISIBLE) != 0u;
                    occl = select(0u, VIS_FRUSTUM, in_view);
                    in_view = in_view && was;
                    occl |= select(0u, VIS_EARLY, in_view);
                }
                inside[v] = in_view;
                if (inside[v] && batch == wg_batch) {
                    local_slot[v] = atomicAdd(&wg_count[v], 1u);
                }
            }
        }
        if (cull.occlusion != 0u) {
            state[idx] = occl;
        }
    }
    workgroupBarrier();
    let batches = cull.mesh_count * VARIANTS;
    if (lid == 0u && wg_batch != 0xffffffffu) {
        for (var v = 0u; v < cull.view_count; v++) {
            let n = atomicLoad(&wg_count[v]);
            if (n > 0u) {
                wg_base[v] = atomicAdd(&draws[v * batches + wg_batch].instance_count, n);
            }
        }
    }
    workgroupBarrier();
    if (!live) {
        return;
    }
    let region = batch_offsets[batch];
    for (var v = 0u; v < cull.view_count; v++) {
        if (!inside[v]) {
            continue;
        }
        var slot: u32;
        if (batch == wg_batch) {
            slot = wg_base[v] + local_slot[v];
        } else {
            slot = atomicAdd(&draws[v * batches + batch].instance_count, 1u);
        }
        if (v == 0u) {
            drawn[region + slot] = d;
        } else {
            visible[v * cull.view_stride + region + slot] = idx;
        }
    }
}

// Whether a box (world-space centre `c` and half axes `a0`, `a1`, `a2`) is hidden from the camera
// `vp` (view-projection, reversed Z) over a `size`-pixel target whose pyramid has `levels` levels:
// its nearest point is farther than the farthest depth the pyramid holds over every pixel it can
// cover. A box reaching behind the camera or past the near plane is never hidden. (The view comes
// in as arguments so that tests/hiz.rs can run this function on its own.)
fn occluded(vp: mat4x4f, size: vec2f, levels: u32, c: vec3f, a0: vec3f, a1: vec3f, a2: vec3f)
    -> bool {
    let cc = vp * vec4f(c, 1.0);
    let x = vp * vec4f(a0, 0.0);
    let y = vp * vec4f(a1, 0.0);
    let z = vp * vec4f(a2, 0.0);
    var lo = vec2f(1e30);
    var hi = vec2f(-1e30);
    var near = 0.0;
    for (var i = 0u; i < 8u; i++) {
        let s = vec3f(f32(i & 1u), f32((i >> 1u) & 1u), f32((i >> 2u) & 1u)) * 2.0 - 1.0;
        let p = cc + x * s.x + y * s.y + z * s.z;
        if (p.w <= 1e-6) {
            return false;
        }
        let n = p.xyz / p.w;
        lo = min(lo, n.xy);
        hi = max(hi, n.xy);
        near = max(near, n.z);
    }
    if (near >= 1.0) {
        return false;
    }
    // The pixels the box can cover (y down), with a pixel of margin, clamped to the target.
    let top = size - 1.0;
    let p0 = clamp(vec2f(lo.x * 0.5 + 0.5, 0.5 - hi.y * 0.5) * size - 1.0, vec2f(0.0), top);
    let p1 = clamp(vec2f(hi.x * 0.5 + 0.5, 0.5 - lo.y * 0.5) * size + 1.0, vec2f(0.0), top);
    let i0 = vec2u(p0);
    let i1 = vec2u(p1);
    // The finest level whose texels (2^(k+1) pixels) put the footprint within 2x2 of them.
    let extent = max(i1.x - i0.x, i1.y - i0.y) + 1u;
    var k = 0u;
    if (extent > 2u) {
        k = firstLeadingBit(extent - 1u);
    }
    k = min(k, levels - 1u);
    let last = textureDimensions(hiz, k) - 1u;
    let t0 = min(i0 >> vec2u(k + 1u), last);
    let t1 = min(i1 >> vec2u(k + 1u), last);
    let far = min(
        min(textureLoad(hiz, t0, k).x, textureLoad(hiz, vec2u(t1.x, t0.y), k).x),
        min(textureLoad(hiz, vec2u(t0.x, t1.y), k).x, textureLoad(hiz, t1, k).x),
    );
    // A relative margin: coplanar surfaces (a wall of touching boxes seen head-on) stay visible.
    return near < far * (1.0 - 1e-5);
}

// Adds to a (low, high) pair of stats words.
fn add_wide(at: u32, v: u32) {
    let old = atomicAdd(&stats[at], v);
    if (old + v < old) {
        atomicAdd(&stats[at + 1u], 1u);
    }
}

@compute @workgroup_size(256)
fn late(@builtin(global_invocation_id) gid: vec3u, @builtin(local_invocation_index) lid: u32,
        @builtin(workgroup_id) wid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let idx = gid.x + gid.y * nwg.x * 256u;
    if (lid == 0u) {
        wg_batch = first_batch(group_first(wid, nwg));
        atomicStore(&wg_count[0], 0u);
        for (var i = 0u; i < 6u; i++) {
            atomicStore(&wg_stats[i], 0u);
        }
    }
    workgroupBarrier();

    var draw = false;
    var batch = 0xffffffffu;
    var local_slot = 0u;
    var d: Drawn;
    if (idx < cull.instance_count) {
        let st = state[idx];
        var seen = false;
        if ((st & VIS_FRUSTUM) != 0u) {
            // In the frustum (so alive, visible and of a known mesh) by the early pass's own test.
            let inst = instances[idx];
            let m = meshes[inst.mesh];
            batch = ((inst.flags >> VARIANT_SHIFT) & 3u) * cull.mesh_count + inst.mesh;
            let pose = instance_pose(inst, cull.alpha);
            let c = pose.pos + quat_rotate(pose.rot, m.box_center * inst.scale);
            let h = m.box_half * inst.scale;
            seen = !occluded(
                cull.view_proj,
                cull.viewport,
                cull.hiz_levels,
                c,
                quat_rotate(pose.rot, vec3f(h.x, 0.0, 0.0)),
                quat_rotate(pose.rot, vec3f(0.0, h.y, 0.0)),
                quat_rotate(pose.rot, vec3f(0.0, 0.0, h.z)),
            );
            let tris = m.index_count / 3u;
            atomicAdd(&wg_stats[0], 1u);
            atomicAdd(&wg_stats[4], tris);
            if (!seen) {
                atomicAdd(&wg_stats[1], 1u);
                atomicAdd(&wg_stats[5], tris);
            }
            if ((st & VIS_EARLY) != 0u) {
                atomicAdd(&wg_stats[2], 1u);
            } else if (seen) {
                draw = true;
                atomicAdd(&wg_stats[3], 1u);
                d.pos = pose.pos;
                d.rot = pose.rot;
                d.scale = inst.scale;
                d.material = inst.material;
                d.slot = idx;
                if (batch == wg_batch) {
                    local_slot = atomicAdd(&wg_count[0], 1u);
                }
            }
        }
        state[idx] = select(0u, VIS_VISIBLE, seen);
    }
    workgroupBarrier();
    let batches = cull.mesh_count * VARIANTS;
    if (lid == 0u) {
        if (wg_batch != 0xffffffffu) {
            let n = atomicLoad(&wg_count[0]);
            if (n > 0u) {
                wg_base[0] = atomicAdd(&draws[LATE * batches + wg_batch].instance_count, n);
            }
        }
        for (var i = 0u; i < 4u; i++) {
            let n = atomicLoad(&wg_stats[i]);
            if (n > 0u) {
                atomicAdd(&stats[i], n);
            }
        }
        add_wide(4u, atomicLoad(&wg_stats[4]));
        add_wide(6u, atomicLoad(&wg_stats[5]));
    }
    workgroupBarrier();
    if (!draw) {
        return;
    }
    var slot: u32;
    if (batch == wg_batch) {
        slot = wg_base[0] + local_slot;
    } else {
        slot = atomicAdd(&draws[LATE * batches + batch].instance_count, 1u);
    }
    drawn[batch_offsets[batch] + slot] = d;
}
