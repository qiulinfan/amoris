// Plunging lips and falling sheets (water.md 63.2): every face of the displayed field over which
// the resident kernel throws water ballistically, found by its own rules (water_compute.wgsl
// `fall_face` and `plunge_share`), becomes a ribbon a face wide along the jet's parabola, from where
// it leaves to where it meets the water or the bed again; `join` then gives neighbouring ribbons of
// one sheet the same edge, so they draw as one surface. Presentation only.
struct LipParams {
    // Columns along x and z, 1 when the columns are in tiles of 16 x 16, the ribbons a region holds.
    count: vec4u,
    // The grid's corner (world x, z), the column size, and the falls' drop (0: none).
    origin_cell: vec4f,
    // 1 with plunging breakers.
    flags: vec4f,
}
// A ribbon: where the jet leaves (world) and the thickness of the water leaving; its velocity
// there and the time it flies; half its width across, and its kind (0 a lip, 1 a fall).
struct Lip { launch: vec4f, velocity: vec4f, side: vec4f }
@group(0) @binding(0) var<uniform> p: LipParams;
@group(0) @binding(1) var<storage, read> columns: array<vec4f>;
@group(0) @binding(2) var<storage, read_write> lips: array<Lip>;
// The indirect draw's vertex count, instance count, first vertex and first instance.
@group(0) @binding(3) var<storage, read_write> args: array<atomic<u32>, 4>;
// Each ribbon's face, and each face's ribbon plus one (0: none): faces along x first, then along z.
@group(0) @binding(4) var<storage, read_write> faces: array<u32>;
@group(0) @binding(5) var<storage, read_write> slots: array<u32>;
// What draws (forward.wgsl `vs_lip`): each ribbon's two edges, where the jet leaves there and the
// thickness of the water leaving, its velocity and the time it flies; and the ribbon's `side`, its
// `w` the kind plus 2 if its left edge ends the sheet and 4 if its right edge does.
struct Sheet { left: vec4f, left_velocity: vec4f, right: vec4f, right_velocity: vec4f, side: vec4f }
@group(0) @binding(6) var<storage, read_write> sheets: array<Sheet>;

// As the kernel (water_compute.wgsl).
const PLUNGE_SLOPE: f32 = 0.5;
const PLUNGE_FROUDE: f32 = 0.5;
const PLUNGE_MAX: f32 = 0.5;
const PLUNGE_DEPTH: f32 = 0.3;
const STEPS: u32 = 96u;
// Faces passing less than this (m^2/s) draw no sheet.
const MIN_FLUX: f32 = 0.02;

fn column(c: vec2u) -> vec4f {
    if (p.count.z == 1u) {
        return columns[((c.x / 16u) + (p.count.x / 16u) * (c.y / 16u)) * 256u
            + (c.x % 16u) + 16u * (c.y % 16u)];
    }
    return columns[c.x + p.count.x * c.y];
}
// A column whose tile was received (missing tiles hold a negative depth or a huge height).
fn valid(c: vec4f) -> bool {
    return c.y >= 0.0 && abs(c.x) < 1e10;
}

fn face_index(a: vec2u, axis: u32) -> u32 {
    return axis * p.count.x * p.count.y + a.x + p.count.x * a.y;
}

fn face(a: vec2u, axis: u32) {
    // Every face this frame finds no ribbon on forgets the last frame's.
    slots[face_index(a, axis)] = 0u;
    let b = a + select(vec2u(0u, 1u), vec2u(1u, 0u), axis == 0u);
    if (any(b >= p.count.xy)) { return; }
    let ca = column(a);
    let cb = column(b);
    if (!valid(ca) || !valid(cb)) { return; }
    // The face's way from the centred velocities either side, and its speed from the water
    // arriving: over a front the face runs as fast as the water behind it, which the mean of a
    // column at speed and one at rest halves.
    let mean = 0.5 * (ca[2u + axis] + cb[2u + axis]);
    if (mean == 0.0) { return; }
    let forward = mean > 0.0;
    let donor = select(cb, ca, forward);
    let receiver = select(ca, cb, forward);
    let h = donor.y;
    if (h <= 1e-4) { return; }
    let v = select(-1.0, 1.0, forward) * max(abs(mean), max(select(-1.0, 1.0, forward) * donor[2u + axis], 0.0));
    var thickness = 0.0;
    var y0 = 0.0;
    var speed = abs(v);
    var kind = 0.0;
    let bed = donor.x - h;
    if (p.origin_cell.w > 0.0 && bed - receiver.x > p.origin_cell.w) {
        // A fall: all the flow over the lip, leaving at its mid-depth.
        thickness = h;
        y0 = bed + 0.5 * h;
        kind = 1.0;
    } else if (p.flags.x > 0.0 && h >= PLUNGE_DEPTH) {
        let slope = (donor.x - receiver.x) / p.origin_cell.z;
        let froude = abs(v) / sqrt(9.81 * h);
        let share = PLUNGE_MAX * clamp(slope / PLUNGE_SLOPE - 1.0, 0.0, 1.0)
            * clamp(froude / PLUNGE_FROUDE - 1.0, 0.0, 1.0);
        if (share <= 0.0) { return; }
        // The crest's top layer, leaving at the crest's long-wave speed.
        thickness = share * h;
        y0 = donor.x - 0.5 * thickness;
        speed = max(speed, sqrt(9.81 * h));
    } else {
        return;
    }
    if (speed * thickness < MIN_FLUX) { return; }
    let direction = select(-1.0, 1.0, forward);
    let along = select(vec2f(0.0, direction), vec2f(direction, 0.0), axis == 0u);
    // The donor's velocity across the face carries the jet sideways.
    let across = select(donor.z, donor.w, axis == 0u);
    let flat = along * speed + select(vec2f(across, 0.0), vec2f(0.0, across), axis == 0u);
    let lip = (vec2f(a) + vec2f(b)) * 0.5 + vec2f(0.5);
    let cell = p.origin_cell.z;
    let rate = max(length(flat), 1e-4);
    // March the parabola half a column at a time to the first column whose water or bed it meets.
    var flight = 0.0;
    for (var s = 0u; s < STEPS; s++) {
        let t = (f32(s) * 0.5 + 0.5) * cell / rate;
        let at = lip + flat * (t / cell);
        flight = t;
        if (any(at < vec2f(0.0)) || any(at >= vec2f(p.count.xy))) { break; }
        let c = column(vec2u(floor(at)));
        if (!valid(c) || y0 - 4.905 * t * t <= c.x) { break; }
    }
    let n = atomicAdd(&args[1], 1u);
    if (n >= p.count.w) {
        atomicSub(&args[1], 1u);
        return;
    }
    let world = vec3f(p.origin_cell.x + lip.x * cell, y0, p.origin_cell.y + lip.y * cell);
    let side = select(vec3f(0.5 * cell, 0.0, 0.0), vec3f(0.0, 0.0, 0.5 * cell), axis == 0u);
    lips[n] = Lip(vec4f(world, thickness), vec4f(flat.x, 0.0, flat.y, flight), vec4f(side, kind));
    faces[n] = face_index(a, axis);
    slots[face_index(a, axis)] = n + 1u;
}

@compute @workgroup_size(8, 8)
fn find(@builtin(global_invocation_id) id: vec3u) {
    if (any(id.xy >= p.count.xy)) { return; }
    face(id.xy, 0u);
    face(id.xy, 1u);
}

// The ribbon beside ribbon `me` (on face `a`, `axis`) toward `d` along the sheet, if it is part of the
// same sheet: thrown the same way, of the same kind, from about the same height.
fn beside(me: Lip, a: vec2u, axis: u32, d: i32) -> i32 {
    let at = vec2i(a) + select(vec2i(d, 0), vec2i(0, d), axis == 0u);
    if (any(at < vec2i(0)) || any(at >= vec2i(p.count.xy))) { return -1; }
    let slot = slots[face_index(vec2u(at), axis)];
    if (slot == 0u) { return -1; }
    let o = lips[slot - 1u];
    if (o.side.w != me.side.w || dot(o.velocity.xz, me.velocity.xz) <= 0.0
        || abs(o.launch.y - me.launch.y) > p.origin_cell.z) {
        return -1;
    }
    return i32(slot - 1u);
}

// Each ribbon's edges: where the next ribbon of its sheet lies beside it, the mean of the two, which
// both work out to the same bits, so the sheet has no gap or crack; at the sheet's ends its own.
@compute @workgroup_size(64)
fn join(@builtin(global_invocation_id) id: vec3u) {
    let i = id.x;
    if (i >= min(atomicLoad(&args[1]), p.count.w)) { return; }
    let me = lips[i];
    let f = faces[i];
    let half = p.count.x * p.count.y;
    let axis = f / half;
    let a = vec2u((f % half) % p.count.x, (f % half) / p.count.x);
    var edge = array<vec4f, 2>();
    var velocity = array<vec4f, 2>();
    // The sheet's free ends, which fray (forward.wgsl `fs_lip`): 2 the left, 4 the right.
    var ends = 0.0;
    for (var k = 0u; k < 2u; k++) {
        let d = select(-1, 1, k == 1u);
        let j = beside(me, a, axis, d);
        if (j >= 0) {
            let o = lips[u32(j)];
            edge[k] = 0.5 * (me.launch + o.launch);
            velocity[k] = 0.5 * (me.velocity + o.velocity);
        } else {
            edge[k] = me.launch + vec4f(me.side.xyz * f32(d), 0.0);
            velocity[k] = me.velocity;
            ends += select(2.0, 4.0, k == 1u);
        }
    }
    sheets[i] = Sheet(edge[0], velocity[0], edge[1], velocity[1], vec4f(me.side.xyz, me.side.w + ends));
}
