// Clustered light assignment: the view frustum split into a grid of cells (screen tiles times
// logarithmic depth slices); one thread per cell lists the point and spot lights whose sphere
// touches the cell's view-space box. The forward pass reads its pixel's cell. A cell holds at most
// `grid.w` lights; overflow is counted, never silent.

@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var<storage, read> lights: array<Light>;
@group(0) @binding(2) var<uniform> clusters: Clusters;
@group(0) @binding(3) var<storage, read_write> cluster_lights: array<u32>;
@group(0) @binding(4) var<storage, read_write> overflow: atomic<u32>;

fn slice_depth(k: f32) -> f32 {
    return clusters.params.x * exp(k / clusters.params.y);
}

@compute @workgroup_size(64, 1, 1)
fn assign(@builtin(global_invocation_id) id: vec3u) {
    let g = clusters.grid;
    let cells = g.x * g.y * g.z;
    let cell = id.x;
    if (cell >= cells) {
        return;
    }
    let x = cell % g.x;
    let y = (cell / g.x) % g.y;
    let z = cell / (g.x * g.y);
    let z0 = slice_depth(f32(z));
    let z1 = select(slice_depth(f32(z + 1u)), 1e9, z + 1u == g.z);
    // Tile corners in NDC (y up), scaled to view space at each depth.
    let nx0 = f32(x) / f32(g.x) * 2.0 - 1.0;
    let nx1 = f32(x + 1u) / f32(g.x) * 2.0 - 1.0;
    let ny0 = 1.0 - f32(y + 1u) / f32(g.y) * 2.0;
    let ny1 = 1.0 - f32(y) / f32(g.y) * 2.0;
    let px = view.proj[0][0];
    let py = view.proj[1][1];
    var lo = vec3f(1e9);
    var hi = vec3f(-1e9);
    for (var i = 0u; i < 2u; i++) {
        let d = select(z0, min(z1, 1e6), i == 1u);
        let xs = vec2f(nx0, nx1) * d / px;
        let ys = vec2f(ny0, ny1) * d / py;
        lo = min(lo, vec3f(min(xs.x, xs.y), min(ys.x, ys.y), -d));
        hi = max(hi, vec3f(max(xs.x, xs.y), max(ys.x, ys.y), -d));
    }
    let per = g.w;
    let base = cell * (per + 1u);
    var count = 0u;
    let n = u32(clusters.params.w);
    for (var i = 0u; i < n; i++) {
        let l = lights[i];
        let c = (view.view * vec4f(l.pos, 1.0)).xyz;
        let q = clamp(c, lo, hi);
        let d = c - q;
        if (dot(d, d) <= l.range * l.range) {
            if (count < per) {
                cluster_lights[base + 1u + count] = i;
            } else {
                atomicAdd(&overflow, 1u);
            }
            count++;
        }
    }
    cluster_lights[base] = min(count, per);
}
