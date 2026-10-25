// Rain falling around the camera (rain.rs; water.md 67): a box of world-anchored cells around the
// eye, each cell holding the same number of drops at places hashed from the cell and the drop's
// slot. A drop falls through the box at its terminal speed, carried by the wind, and wraps from the
// bottom to the top; it is drawn as a thin quad along its path over the camera's exposure, as wide
// as the drop or a pixel and as opaque as the drop covers of that, lit like the haze at the
// horizon. Blended over the scene and tested against its depth, so the water's surface and
// anything in front hide it; a drop under the surface of the water region under the eye (its field
// at group 3, water_field.wgsl) has landed and is not drawn, which keeps the rain out of the view
// from under that water. Every drop's place is a function of the drawn time: the rain stands still
// while time does.

struct RainParams {
    origin: vec4f,    // x, z: the first cell's corner (world); y: the box's bottom; w: its height
    cells: vec4u,     // x, y: cells along x and z; z: drops a cell; w: unused
    velocity: vec4f,  // xyz: the drops' velocity, m/s; w: the exposure, s
    look: vec4f,      // x: the cell's side, y: the drop's width, z: the fade's radius, w: opacity
    clock: vec4f,     // x: how far through the box the drops have fallen at the drawn time, 0 to 1
};

@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var<uniform> rain: RainParams;

// Whether `p` lies under the bound field's surface, by the column of the cell holding it; never
// for the zeroed field bound where no region is under the eye.
fn in_water(p: vec3f) -> bool {
    if (shallow.count.x == 0u) {
        return false;
    }
    let g = (p.xz - shallow.origin) / shallow.cell;
    if (any(g < vec2f(0.0)) || any(g >= vec2f(shallow.count.xy))) {
        return false;
    }
    let c = shallow_column(i32(g.x), i32(g.y));
    return c.y > SHALLOW_WET && p.y < c.x;
}

struct RainOut {
    @builtin(position) clip: vec4f,
    @location(0) across: f32,
    @location(1) alpha: f32,
};

@vertex
fn vs(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> RainOut {
    var o: RainOut;
    let cell = ii / rain.cells.z;
    let slot = ii % rain.cells.z;
    let side = rain.look.x;
    let corner = rain.origin.xz + vec2f(f32(cell % rain.cells.x), f32(cell / rain.cells.x)) * side;
    // The cell's place in the world names its drops, so a drop keeps its place as the box follows
    // the eye.
    let key = vec2i(round(corner / side));
    let h0 = pcg_hash((bitcast<u32>(key.x) * 73856093u) ^ (bitcast<u32>(key.y) * 19349663u)
        ^ (slot * 83492791u));
    let h1 = pcg_hash(h0);
    let h2 = pcg_hash(h1);
    let height = rain.origin.w;
    let fall = max(-rain.velocity.y, 0.1);
    // Seconds since the drop came in at the box's top, and where it is now.
    let age = fract(unit_from_hash(h2) + rain.clock.x) * height / fall;
    let p = vec3f(corner.x + unit_from_hash(h0) * side, rain.origin.y + height,
        corner.y + unit_from_hash(h1) * side) + rain.velocity.xyz * age;
    if (in_water(p)) {
        o.clip = vec4f(0.0, 0.0, 2.0, 1.0);
        return o;
    }
    let tail = p - rain.velocity.xyz * rain.velocity.w;
    let to_eye = view.camera_pos.xyz - p;
    let distance = length(to_eye);
    var across = cross(normalize(rain.velocity.xyz), to_eye / max(distance, 1e-4));
    if (dot(across, across) < 1e-8) {
        across = vec3f(1.0, 0.0, 0.0);
    }
    across = normalize(across);
    // A drop narrower than a pixel is drawn a pixel and a half wide, as faint as it covers of it.
    let pixel = 2.0 * distance / (view.proj[1][1] * view.viewport.y);
    let width = max(rain.look.y, 1.5 * pixel);
    // Faded toward the box's sides and right at the eye, so no drop pops in or out.
    let reach = length(p.xz - view.camera_pos.xz);
    let fade = (1.0 - smoothstep(rain.look.z * 0.7, rain.look.z, reach))
        * smoothstep(0.3, 1.2, distance);
    let quad = array<vec2f, 6>(vec2f(-1.0, 0.0), vec2f(1.0, 0.0), vec2f(1.0, 1.0),
        vec2f(-1.0, 0.0), vec2f(1.0, 1.0), vec2f(-1.0, 1.0));
    let q = quad[vi];
    let world = mix(p, tail, q.y) + across * (q.x * width * 0.5);
    o.clip = view.view_proj * vec4f(world, 1.0);
    o.across = q.x;
    o.alpha = rain.look.w * fade * rain.look.y / width;
    return o;
}

@fragment
fn fs(in: RainOut) -> @location(0) vec4f {
    let a = in.alpha * (1.0 - abs(in.across));
    // Under a liquid the view is its fog: no rain there.
    if (view.fog.w < 0.0 || a <= 0.0) {
        discard;
    }
    // A drop refracts and reflects the sky all around it, the bright sky above most: lit like the
    // haze at the horizon, twice over.
    let light = view.fog.rgb * view.sun_color.w * 2.2;
    return vec4f(light * a, a);
}
