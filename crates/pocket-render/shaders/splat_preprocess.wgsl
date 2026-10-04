// The splat preprocess (docs/spec/splats.md 4): one thread per splat of every drawn cloud. It
// moves the splat into view space with its cloud's pose, culls it (behind the near plane, outside
// the frustum by more than its extent, transparent), projects its 3D covariance to a 2D screen
// covariance (EWA splatting: the Jacobian of the perspective projection at the splat's center),
// takes the ellipse's axes from the eigen-decomposition, evaluates the spherical harmonics toward
// the camera, and appends the result with its depth key for the sort. Appends count per workgroup
// first, so 3M splats pay 12k global atomics, not 3M.

// One Gaussian as stored (cloud.rs PackedSplat), 32 bytes.
struct Splat {
    pos: vec3f,
    rot: u32,          // smallest-three quaternion
    scale_opacity: vec2u,
    color: vec2u,
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

@group(0) @binding(0) var<uniform> params: SplatParams;
@group(0) @binding(1) var<storage, read> splats: array<Splat>;
@group(0) @binding(2) var<storage, read> sh: array<u32>;
@group(0) @binding(3) var<storage, read> clouds: array<Cloud>;
@group(0) @binding(4) var<storage, read_write> projected: array<Projected>;
@group(0) @binding(5) var<storage, read_write> keys: array<u32>;
@group(0) @binding(6) var<storage, read_write> vals: array<u32>;
// [0] visible count; [1] their quads' area in units of 16 pixels (each clipped to the screen's
// size: a fill estimate); [4..7] the sort's indirect dispatch; [8..13] the indexed indirect draw.
@group(0) @binding(7) var<storage, read_write> control: array<atomic<u32>>;

var<workgroup> wg_count: atomic<u32>;
var<workgroup> wg_area: atomic<u32>;
var<workgroup> wg_base: u32;

fn unpack_quat(p: u32) -> vec4f {
    let largest = p >> 30u;
    let r = (vec3f(f32((p >> 20u) & 1023u), f32((p >> 10u) & 1023u), f32(p & 1023u)) / 1023.0 * 2.0 - 1.0)
        * 0.70710678;
    let m = sqrt(max(0.0, 1.0 - dot(r, r)));
    var q: vec4f;
    switch largest {
        case 0u: { q = vec4f(m, r.x, r.y, r.z); }
        case 1u: { q = vec4f(r.x, m, r.y, r.z); }
        case 2u: { q = vec4f(r.x, r.y, m, r.z); }
        default: { q = vec4f(r.x, r.y, r.z, m); }
    }
    return normalize(q);
}

// Rotation matrix of a unit quaternion (x, y, z, w), columns.
fn quat_mat(q: vec4f) -> mat3x3f {
    let x = q.x;
    let y = q.y;
    let z = q.z;
    let w = q.w;
    return mat3x3f(
        vec3f(1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y + w * z), 2.0 * (x * z - w * y)),
        vec3f(2.0 * (x * y - w * z), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z + w * x)),
        vec3f(2.0 * (x * z + w * y), 2.0 * (y * z - w * x), 1.0 - 2.0 * (x * x + y * y)),
    );
}

// SH coefficient j (1-based band index) of the splat whose terms start at word `base`.
fn sh_coeff(base: u32, j: u32) -> vec3f {
    let h = (j - 1u) * 3u;
    let w = base + h / 2u;
    let a = unpack2x16float(sh[w]);
    let b = unpack2x16float(sh[w + 1u]);
    if ((h & 1u) == 0u) {
        return vec3f(a.x, a.y, b.x);
    }
    return vec3f(a.y, b.x, b.y);
}

// The view-dependent terms toward direction `d` (degree 1 to 3; the reference implementation's
// constants and signs).
fn sh_color(base: u32, degree: u32, d: vec3f) -> vec3f {
    let x = d.x;
    let y = d.y;
    let z = d.z;
    var c = 0.4886025119029199 * (-y * sh_coeff(base, 1u) + z * sh_coeff(base, 2u) - x * sh_coeff(base, 3u));
    if (degree > 1u) {
        let xx = x * x;
        let yy = y * y;
        let zz = z * z;
        c += 1.0925484305920792 * x * y * sh_coeff(base, 4u)
            - 1.0925484305920792 * y * z * sh_coeff(base, 5u)
            + 0.31539156525252005 * (2.0 * zz - xx - yy) * sh_coeff(base, 6u)
            - 1.0925484305920792 * x * z * sh_coeff(base, 7u)
            + 0.5462742152960396 * (xx - yy) * sh_coeff(base, 8u);
        if (degree > 2u) {
            c += -0.5900435899266435 * y * (3.0 * xx - yy) * sh_coeff(base, 9u)
                + 2.890611442640554 * x * y * z * sh_coeff(base, 10u)
                - 0.4570457994644658 * y * (4.0 * zz - xx - yy) * sh_coeff(base, 11u)
                + 0.3731763325901154 * z * (2.0 * zz - 3.0 * xx - 3.0 * yy) * sh_coeff(base, 12u)
                - 0.4570457994644658 * x * (4.0 * zz - xx - yy) * sh_coeff(base, 13u)
                + 1.445305721320277 * z * (xx - yy) * sh_coeff(base, 14u)
                - 0.5900435899266435 * x * (xx - 3.0 * yy) * sh_coeff(base, 15u);
        }
    }
    return c;
}

// The cloud thread `g` belongs to: the last whose `first` is <= g.
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

@compute @workgroup_size(256)
fn preprocess(
    @builtin(workgroup_id) wid: vec3u,
    @builtin(num_workgroups) groups: vec3u,
    @builtin(local_invocation_index) lid: u32,
) {
    if (lid == 0u) {
        atomicStore(&wg_count, 0u);
        atomicStore(&wg_area, 0u);
    }
    workgroupBarrier();
    let g = (wid.y * groups.x + wid.x) * 256u + lid;
    var visible = false;
    var out: Projected;
    var key = 0u;
    if (g < params.counts.x) {
        let cloud = clouds[find_cloud(g)];
        let s = splats[cloud.offset + g - cloud.first];
        let t = (cloud.model_view * vec4f(s.pos, 1.0)).xyz;
        let tz = -t.z;
        let so = unpack2x16float(s.scale_opacity.y);
        let opacity = so.y;
        if (tz > params.depth.x && opacity >= 1.0 / 255.0) {
            // Covariance: (A R S)(A R S)^T with A the model-view's linear part; then the 2D
            // covariance J W Sigma W^T J^T in pixels, with W folded into A.
            let sxy = unpack2x16float(s.scale_opacity.x);
            let r = quat_mat(unpack_quat(s.rot));
            let a = mat3x3f(cloud.model_view[0].xyz, cloud.model_view[1].xyz, cloud.model_view[2].xyz);
            let m = a * mat3x3f(r[0] * sxy.x, r[1] * sxy.y, r[2] * so.x);
            let fx = params.proj.x * params.viewport.x * 0.5;
            let fy = params.proj.y * params.viewport.y * 0.5;
            // The Jacobian at the center, its tangents clamped to 1.3x the frustum's (as the
            // reference does) so splats far off screen do not explode.
            let lim = vec2f(1.3 / params.proj.x, 1.3 / params.proj.y);
            let tan_c = clamp(t.xy / tz, -lim, lim);
            let j0 = vec3f(fx / tz, 0.0, fx * tan_c.x / tz);
            let j1 = vec3f(0.0, fy / tz, fy * tan_c.y / tz);
            let g0 = vec3f(dot(j0, m[0]), dot(j0, m[1]), dot(j0, m[2]));
            let g1 = vec3f(dot(j1, m[0]), dot(j1, m[1]), dot(j1, m[2]));
            // A 0.3 px^2 low-pass, as in training: no splat is thinner than about half a pixel.
            let ca = dot(g0, g0) + 0.3;
            let cb = dot(g0, g1);
            let cc = dot(g1, g1) + 0.3;
            let mid = 0.5 * (ca + cc);
            let disc = sqrt(max(0.1, mid * mid - (ca * cc - cb * cb)));
            let l1 = mid + disc;
            let l2 = max(mid - disc, 0.1);
            var v1 = vec2f(cb, l1 - ca);
            if (dot(v1, v1) < 1e-12) {
                v1 = select(vec2f(0.0, 1.0), vec2f(1.0, 0.0), ca >= cc);
            }
            v1 = normalize(v1);
            let v2 = vec2f(-v1.y, v1.x);
            // Cut where the Gaussian falls below 1/255 of its peak (at most 3 sigma).
            let k = min(sqrt(max(2.0 * log(255.0 * opacity), 0.0)), 3.0);
            let ndc = vec2f(params.proj.x, params.proj.y) * t.xy / tz;
            let extent = k * sqrt(vec2f(ca, cc)) * 2.0 * params.viewport.zw;
            if (all(abs(ndc) <= vec2f(1.0) + extent)) {
                visible = true;
                let to_ndc = 2.0 * params.viewport.zw;
                // (Clamped into f16's range: only a splat around the camera gets near it.)
                let ax1 = clamp(v1 * sqrt(l1) * to_ndc, vec2f(-3e4), vec2f(3e4));
                let ax2 = clamp(v2 * sqrt(l2) * to_ndc, vec2f(-3e4), vec2f(3e4));
                let rg = unpack2x16float(s.color.x);
                var c = vec3f(rg, unpack2x16float(s.color.y).x);
                if (cloud.sh_degree > 0u) {
                    let dir = normalize(s.pos - cloud.camera_local.xyz);
                    let base = cloud.sh_offset + (g - cloud.first) * cloud.sh_words;
                    c += sh_color(base, cloud.sh_degree, dir);
                }
                // Trained colors are display-encoded; the HDR target is linear light.
                c = srgb_to_linear(max(c, vec3f(0.0))) * params.color.x;
                out.center = ndc;
                out.axes = vec2u(pack2x16float(ax1), pack2x16float(ax2));
                out.color = vec2u(pack2x16float(c.rg), pack2x16float(vec2f(c.b, opacity)));
                key = depth_key(tz, params);
                let area = min(4.0 * k * k * sqrt(l1 * l2), params.viewport.x * params.viewport.y);
                atomicAdd(&wg_area, u32(area / 16.0));
            }
        }
    }
    var local = 0u;
    if (visible) {
        local = atomicAdd(&wg_count, 1u);
    }
    workgroupBarrier();
    if (lid == 0u) {
        wg_base = atomicAdd(&control[0], atomicLoad(&wg_count));
        atomicAdd(&control[1], atomicLoad(&wg_area));
    }
    let base = workgroupUniformLoad(&wg_base);
    if (visible) {
        let slot = base + local;
        projected[slot] = out;
        keys[slot] = key;
        vals[slot] = slot;
    }
}

// After the preprocess: the sort's dispatch (one workgroup per tile) and the draw's arguments.
@compute @workgroup_size(1)
fn finish() {
    let n = atomicLoad(&control[0]);
    atomicStore(&control[4], (n + SORT_TILE - 1u) / SORT_TILE);
    atomicStore(&control[5], 1u);
    atomicStore(&control[6], 1u);
    atomicStore(&control[8], 6u * DRAW_BATCH);
    atomicStore(&control[9], (n + DRAW_BATCH - 1u) / DRAW_BATCH);
    atomicStore(&control[10], 0u);
    atomicStore(&control[11], 0u);
    atomicStore(&control[12], 0u);
}
