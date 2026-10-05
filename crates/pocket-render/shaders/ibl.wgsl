// Image-based light from the sky cube: a box-filtered mip chain of the raw sky, the GGX-prefiltered
// radiance cube (one mip per roughness step), and the 9 spherical-harmonics coefficients of the
// irradiance.

struct Params {
    size: vec4u,             // x: output face size, y: mip level of the output, z: mip count
    rough: vec4f,            // x: roughness of this level
};

@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var src: texture_cube<f32>;
@group(0) @binding(2) var src_sampler: sampler;
@group(0) @binding(3) var dst: texture_storage_2d_array<rgba16float, write>;

fn face_dir(face: u32, uv: vec2f) -> vec3f {
    let a = uv * 2.0 - 1.0;
    switch face {
        case 0u: { return normalize(vec3f(1.0, -a.y, -a.x)); }
        case 1u: { return normalize(vec3f(-1.0, -a.y, a.x)); }
        case 2u: { return normalize(vec3f(a.x, 1.0, a.y)); }
        case 3u: { return normalize(vec3f(a.x, -1.0, -a.y)); }
        case 4u: { return normalize(vec3f(a.x, -a.y, 1.0)); }
        default: { return normalize(vec3f(-a.x, -a.y, -1.0)); }
    }
}

// A downsampled mip of the raw sky: average of the source at the matching direction (the sampler's
// linear filter on the finer mip does the 2x2 box).
@compute @workgroup_size(8, 8, 1)
fn downsample(@builtin(global_invocation_id) id: vec3u) {
    let n = p.size.x;
    if (id.x >= n || id.y >= n) {
        return;
    }
    let dir = face_dir(id.z, (vec2f(id.xy) + 0.5) / f32(n));
    let c = textureSampleLevel(src, src_sampler, dir, f32(p.size.y) - 1.0);
    textureStore(dst, vec2u(id.xy), id.z, c);
}

fn hammersley(i: u32, n: u32) -> vec2f {
    var b = i;
    b = (b << 16u) | (b >> 16u);
    b = ((b & 0x55555555u) << 1u) | ((b & 0xAAAAAAAAu) >> 1u);
    b = ((b & 0x33333333u) << 2u) | ((b & 0xCCCCCCCCu) >> 2u);
    b = ((b & 0x0F0F0F0Fu) << 4u) | ((b & 0xF0F0F0F0u) >> 4u);
    b = ((b & 0x00FF00FFu) << 8u) | ((b & 0xFF00FF00u) >> 8u);
    return vec2f(f32(i) / f32(n), f32(b) * 2.3283064365386963e-10);
}

// GGX-prefiltered radiance for this level's roughness, sampling the source's mips by each sample's
// solid angle (Karis' filtered importance sampling) so 64 samples suffice.
@compute @workgroup_size(8, 8, 1)
fn prefilter(@builtin(global_invocation_id) id: vec3u) {
    let n = p.size.x;
    if (id.x >= n || id.y >= n) {
        return;
    }
    let nrm = face_dir(id.z, (vec2f(id.xy) + 0.5) / f32(n));
    let rough = p.rough.x;
    if (rough <= 0.0) {
        textureStore(dst, vec2u(id.xy), id.z, textureSampleLevel(src, src_sampler, nrm, 0.0));
        return;
    }
    let a = rough * rough;
    let up = select(vec3f(0.0, 1.0, 0.0), vec3f(1.0, 0.0, 0.0), abs(nrm.y) > 0.999);
    let tx = normalize(cross(up, nrm));
    let ty = cross(nrm, tx);
    let src_size = f32(textureDimensions(src).x);
    let texel_solid = 4.0 * PI / (6.0 * src_size * src_size);
    var sum = vec3f(0.0);
    var weight = 0.0;
    let count = 64u;
    for (var i = 0u; i < count; i++) {
        let xi = hammersley(i, count);
        let phi = 2.0 * PI * xi.x;
        let ct = sqrt((1.0 - xi.y) / (1.0 + (a * a - 1.0) * xi.y));
        let st = sqrt(1.0 - ct * ct);
        let h = tx * (st * cos(phi)) + ty * (st * sin(phi)) + nrm * ct;
        let l = 2.0 * dot(nrm, h) * h - nrm;
        let nl = dot(nrm, l);
        if (nl > 0.0) {
            let nh = ct;
            let a2 = a * a;
            let dd = nh * nh * (a2 - 1.0) + 1.0;
            let d = a2 / (PI * dd * dd);
            let pdf = d / 4.0;
            let sample_solid = 1.0 / (f32(count) * pdf + 1e-4);
            let mip = clamp(0.5 * log2(sample_solid / texel_solid) + 1.0, 0.0, f32(p.size.z) - 1.0);
            sum += textureSampleLevel(src, src_sampler, l, mip).rgb * nl;
            weight += nl;
        }
    }
    textureStore(dst, vec2u(id.xy), id.z, vec4f(sum / max(weight, 1e-4), 1.0));
}

// --- Spherical harmonics of the irradiance: one workgroup sums every texel of a small mip ---------

@group(0) @binding(4) var<storage, read_write> sh_out: array<vec4f, 9>;

var<workgroup> partial: array<array<vec3f, 9>, 64>;
var<workgroup> wsum: array<f32, 64>;

@compute @workgroup_size(64, 1, 1)
fn project_sh(@builtin(local_invocation_index) lid: u32) {
    let n = p.size.x;
    let total = n * n * 6u;
    var acc: array<vec3f, 9>;
    var w = 0.0;
    for (var t = lid; t < total; t += 64u) {
        let face = t / (n * n);
        let xy = vec2u(t % n, (t / n) % n);
        let uv = (vec2f(xy) + 0.5) / f32(n);
        let d = face_dir(face, uv);
        // Texel solid angle (up to a constant): (1 + u^2 + v^2)^(-3/2).
        let st = uv * 2.0 - 1.0;
        let dw = pow(1.0 + dot(st, st), -1.5);
        let c = textureSampleLevel(src, src_sampler, d, f32(p.size.y)).rgb * dw;
        acc[0] += c * 0.282095;
        acc[1] += c * 0.488603 * d.y;
        acc[2] += c * 0.488603 * d.z;
        acc[3] += c * 0.488603 * d.x;
        acc[4] += c * 1.092548 * d.x * d.y;
        acc[5] += c * 1.092548 * d.y * d.z;
        acc[6] += c * 0.315392 * (3.0 * d.z * d.z - 1.0);
        acc[7] += c * 1.092548 * d.x * d.z;
        acc[8] += c * 0.546274 * (d.x * d.x - d.y * d.y);
        w += dw;
    }
    partial[lid] = acc;
    wsum[lid] = w;
    workgroupBarrier();
    if (lid == 0u) {
        var total_w = 0.0;
        var out: array<vec3f, 9>;
        for (var i = 0u; i < 64u; i++) {
            total_w += wsum[i];
            for (var k = 0u; k < 9u; k++) {
                out[k] += partial[i][k];
            }
        }
        let norm = 4.0 * PI / total_w;
        for (var k = 0u; k < 9u; k++) {
            sh_out[k] = vec4f(out[k] * norm, 0.0);
        }
    }
}
