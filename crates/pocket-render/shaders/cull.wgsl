// GPU culling: one thread per instance tests it against every view (the camera's frustum and each
// shadow cascade's box) and appends its index to that view's visible list in its mesh's region,
// counting the instances of each indirect draw. Counting is two-level: threads of a workgroup
// whose instance has the workgroup's common mesh add to a workgroup counter first, so a scene of
// one mesh pays one global atomic per workgroup and view instead of one per instance.

struct Planes {
    p: array<vec4f, 6>,
};

struct Cull {
    planes: array<Planes, 5>,
    instance_count: u32,
    view_count: u32,
    mesh_count: u32,
    view_stride: u32,        // entries per view in the visible list
    alpha: f32,
    lod_scale: f32,          // unused yet: screen-size LOD selection
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

var<workgroup> wg_batch: u32;
var<workgroup> wg_count: array<atomic<u32>, 5>;
var<workgroup> wg_base: array<u32, 5>;

fn sphere_in(view: u32, c: vec3f, r: f32) -> bool {
    for (var i = 0u; i < 6u; i++) {
        let pl = cull.planes[view].p[i];
        if (dot(pl.xyz, c) + pl.w < -r) {
            return false;
        }
    }
    return true;
}

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) gid: vec3u, @builtin(local_invocation_index) lid: u32,
        @builtin(workgroup_id) wid: vec3u, @builtin(num_workgroups) nwg: vec3u) {
    let idx = gid.x + gid.y * nwg.x * 256u;
    let first = (wid.x + wid.y * nwg.x) * 256u;
    if (lid == 0u) {
        if (first < cull.instance_count) {
            let f = instances[first];
            wg_batch = ((f.flags >> VARIANT_SHIFT) & 3u) * cull.mesh_count + f.mesh;
        } else {
            wg_batch = 0xffffffffu;
        }
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
    if (idx < cull.instance_count) {
        let inst = instances[idx];
        let need = FLAG_ALIVE | FLAG_VISIBLE;
        if ((inst.flags & need) == need && inst.mesh < cull.mesh_count) {
            live = true;
            mesh = inst.mesh;
            batch = ((inst.flags >> VARIANT_SHIFT) & 3u) * cull.mesh_count + mesh;
            let m = meshes[mesh];
            let pose = instance_pose(inst, cull.alpha);
            let c = pose.pos + quat_rotate(pose.rot, m.center * inst.scale);
            let s = abs(inst.scale);
            let r = m.radius * max(s.x, max(s.y, s.z));
            for (var v = 0u; v < cull.view_count; v++) {
                let shadow_ok = v == 0u || (inst.flags & FLAG_SHADOW) != 0u;
                inside[v] = shadow_ok && sphere_in(v, c, r);
                if (inside[v] && batch == wg_batch) {
                    local_slot[v] = atomicAdd(&wg_count[v], 1u);
                }
            }
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
        visible[v * cull.view_stride + region + slot] = idx;
    }
}
