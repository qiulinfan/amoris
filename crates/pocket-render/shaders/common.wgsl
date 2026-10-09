// Shared declarations, prepended to every shader of the renderer (shaders.rs).

const PI: f32 = 3.14159265358979;
const MAX_VIEWS: u32 = 5u;          // the camera and four shadow cascades
const FLAG_ALIVE: u32 = 1u;
const FLAG_VISIBLE: u32 = 2u;
const FLAG_SHADOW: u32 = 4u;
// Bits 3-5: the pipeline variant (1 alpha-masked, 2 double-sided, 4 neural texture), so opaque
// pixels never run a shader containing `discard` (which turns off hidden-surface removal on
// tile-based GPUs) and only neural materials run the neural decoder.
const VARIANT_SHIFT: u32 = 3u;
const VARIANT_MASK: u32 = 7u;
const VARIANTS: u32 = 8u;
const NO_TEXTURE: u32 = 0xffffffffu;

// Per frame. Reversed-Z, infinite far plane: depth 1 at the near plane, 0 at infinity.
struct View {
    view_proj: mat4x4f,
    view: mat4x4f,
    proj: mat4x4f,
    inv_view_proj: mat4x4f,
    camera_pos: vec4f,       // w: seconds of simulated time
    viewport: vec4f,         // width, height, 1/width, 1/height
    sun_dir: vec4f,          // toward the sun, normalized; w: sun illuminance (0: no sun)
    sun_color: vec4f,        // linear rgb; w: ambient (image-based light) strength
    fog: vec4f,              // linear rgb; w: density per metre
    params: vec4f,           // x: interpolation alpha, y: exposure multiplier, z: bloom, w: sky kind
    sky_color: vec4f,        // the flat sky's color; w: shadows enabled (1/0)
    cascade_splits: vec4f,   // far distance of each cascade, metres
    cascades: array<mat4x4f, 4>,
    counts: vec4u,           // x: entries per view in the visible list (the shadow pass's cascade)
};

// One drawn thing, 80 bytes. Poses of the last two ticks; drawn at mix(prev, cur, alpha).
struct Instance {
    pos: vec3f,
    mesh: u32,
    rot: vec4f,
    scale: vec3f,
    material: u32,
    prev_pos: vec3f,
    flags: u32,
    prev_rot: vec4f,
};

// A visible instance of the camera view, its pose already interpolated by the culling pass (48
// bytes): the vertex shader does one read and no interpolation per vertex.
struct Drawn {
    pos: vec3f,
    material: u32,
    rot: vec4f,
    scale: vec3f,
    slot: u32,
};

struct MeshInfo {
    center: vec3f,           // the bounding sphere (frustum culling)
    radius: f32,
    index_count: u32,
    first_index: u32,
    base_vertex: i32,
    batch_offset: u32,       // where this mesh's region starts in each view's visible list
    box_center: vec3f,       // the bounding box (occlusion culling)
    // On a mesh's own row, its levels of detail (docs/spec/lod.md): the count, the full mesh
    // included, in the top 8 bits (below 2: none) and the row of level 1 in the low 24; 0 on the
    // rows of coarser levels.
    lods: u32,
    box_half: vec3f,
    lod_error: f32,          // the level's geometric error, in the mesh's units (0: the full mesh)
};

struct Material {
    base_color: vec4f,
    emissive: vec3f,
    metallic: f32,
    roughness: f32,
    alpha_cutoff: f32,
    flags: u32,              // 1: alpha mask, 2: double sided, 4: unlit, 8: neural texture
    neural: u32,             // the neural texture's descriptor word (neural.rs)
    // Layers in the texture arrays; NO_TEXTURE when absent.
    base_color_tex: u32,
    normal_tex: u32,
    metal_rough_tex: u32,
    emissive_tex: u32,
};

// A point or spot light (64 bytes).
struct Light {
    pos: vec3f,
    range: f32,
    color: vec3f,          // color times intensity
    kind: u32,             // 1 point, 2 spot
    dir: vec3f,            // spot axis
    cos_outer: f32,
    cos_inner: f32,
    _p0: f32,
    _p1: f32,
    _p2: f32,
};

// The cluster grid.
struct Clusters {
    grid: vec4u,           // x, y, z cells; w: most lights per cell
    params: vec4f,         // near, cells / ln(far / near), far, light count
};

fn quat_rotate(q: vec4f, v: vec3f) -> vec3f {
    let t = 2.0 * cross(q.xyz, v);
    return v + q.w * t + cross(q.xyz, t);
}

fn nlerp(a: vec4f, b: vec4f, t: f32) -> vec4f {
    let bb = select(b, -b, dot(a, b) < 0.0);
    return normalize(mix(a, bb, t));
}

struct Pose {
    pos: vec3f,
    rot: vec4f,
};

fn instance_pose(i: Instance, alpha: f32) -> Pose {
    var p: Pose;
    p.pos = mix(i.prev_pos, i.pos, alpha);
    p.rot = nlerp(i.prev_rot, i.rot, alpha);
    return p;
}

// Linear rgb from an sRGB-encoded value (for colors given in sRGB).
fn srgb_to_linear(c: vec3f) -> vec3f {
    return select(pow((c + 0.055) / 1.055, vec3f(2.4)), c / 12.92, c <= vec3f(0.04045));
}

fn luminance(c: vec3f) -> f32 {
    return dot(c, vec3f(0.2126, 0.7152, 0.0722));
}

// A PCG integer hash (Jarzynski and Olano, "Hash Functions for GPU Rendering", 2020): the same bits
// on every GPU, API and shader compiler. The usual `fract(sin(x) * 43758.5453)` is not: `sin` of a
// large argument rounds differently per vendor and compiler, and the multiplication amplifies it
// (the sea's wavelets on a Radeon differed between Direct3D 12 and Vulkan; docs/bench/dx12.md).
fn pcg_hash(v: u32) -> u32 {
    let s = v * 747796405u + 2891336453u;
    let w = ((s >> ((s >> 28u) + 4u)) ^ s) * 277803737u;
    return (w >> 22u) ^ w;
}

// A uniform value in [0, 1) from a hash's top 24 bits (exact in an f32).
fn unit_from_hash(h: u32) -> f32 {
    return f32(h >> 8u) * (1.0 / 16777216.0);
}
