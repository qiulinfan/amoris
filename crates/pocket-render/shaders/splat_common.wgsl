// Declarations shared by the Gaussian splat shaders (splat/mod.rs; docs/spec/splats.md).

const SORT_TILE: u32 = 4096u;   // keys per sort tile: splat_sort.wgsl TILE, sort.rs TILE
const DRAW_BATCH: u32 = 16383u; // quads per drawn instance: splat/mod.rs BATCH
const SPLAT_ANTIALIAS: u32 = 1u; // counts.w flag: opacity compensation for the low-pass
const SPLAT_COUNT_TESTS: u32 = 2u; // counts.w flag: the tile raster counts its (pixel, splat) tests

// Per frame, written by splat/mod.rs.
struct SplatParams {
    viewport: vec4f,   // width, height, 1/width, 1/height (pixels)
    proj: vec4f,       // the projection's P00, P11, P22, P32 (column-major [col][row])
    depth: vec4f,      // x: nearest drawn depth; y: log2 of the key range's near end;
                       // z: key range per log2 unit (1 / (log2 far - log2 near)); w: largest key
    counts: vec4u,     // x: splats over all drawn clouds; y: clouds; z: key bits (32: raw float
                       // bits); w: flags (SPLAT_ANTIALIAS, SPLAT_COUNT_TESTS)
    color: vec4f,      // x: radiance multiplier
    tiles: vec4u,      // the tile rasterizer (splat_tile.wgsl): tiles across, down; pair capacity;
                       // splat capacity
};

// A drawn cloud (splat/mod.rs CloudGpu), 112 bytes.
struct Cloud {
    model_view: mat4x4f,
    camera_local: vec4f,   // the camera's position in the cloud's frame
    first: u32,            // its first thread
    offset: u32,           // its first splat in `splats`
    count: u32,
    sh_degree: u32,
    sh_offset: u32,        // its first word in `sh`
    sh_words: u32,         // words per splat
    _p0: u32,
    _p1: u32,
};

// One drawn splat after the preprocess, 24 bytes.
struct Projected {
    center: vec2f,     // NDC
    axes: vec2u,       // f16 pairs: the ellipse's major and minor axes in NDC, one sigma long
    color: vec2u,      // f16 pairs: (r, g), (b, opacity); linear, premultiplied later
};

// The view depth (distance along the view axis) a sort key stands for.
fn key_depth(key: u32, p: SplatParams) -> f32 {
    if (p.counts.z >= 32u) {
        return bitcast<f32>(~key);
    }
    let x = 1.0 - f32(key) / p.depth.w;
    return exp2(p.depth.y + x / p.depth.z);
}

// Ascending keys run back to front: far splats first.
fn depth_key(tz: f32, p: SplatParams) -> u32 {
    if (p.counts.z >= 32u) {
        return ~bitcast<u32>(tz);
    }
    let x = clamp((log2(tz) - p.depth.y) * p.depth.z, 0.0, 1.0);
    return u32(round((1.0 - x) * p.depth.w));
}
