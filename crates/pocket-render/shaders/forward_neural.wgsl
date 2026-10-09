// The forward pass's neural-texture variant (docs/spec/neural-textures.md 5): the material's
// channels come from the neural decoder (neural_texture.wgsl) instead of the texture arrays; the
// rest of the shading is forward.wgsl's. Composed after forward.wgsl and the decoder, only into the
// neural variants' module (shaders.rs), so the other materials' shaders do not contain it.

// The decoder's resources, beside the texture arrays in group 2 (binding 5: the f16 view).
const NT_GROUP: u32 = 2u;
const NT_BLOCKS: u32 = 5u;
@group(2) @binding(3) var nt_latents: texture_2d<u32>;
@group(2) @binding(4) var<uniform> nt_data: array<vec4u, 4096>;

// Mip transitions blend two decodes only within this fraction of a mip level ("brilinear"): one
// decode elsewhere, so the cost stays near one network evaluation per pixel.
const NT_BLEND_BAND: f32 = 0.25;

fn srgb_to_linear3(c: vec3f) -> vec3f {
    let low = c / 12.92;
    let high = pow((c + 0.055) / 1.055, vec3f(2.4));
    return select(high, low, c <= vec3f(0.04045));
}

// Output `i` of a decode clamped to [0, 1], or `fallback` when the texture lacks it (0xff).
fn nt_get(y: array<vec4<nt_t>, NT_OUT4>, i: u32, fallback: f32) -> f32 {
    if i >= NT_OUT {
        return fallback;
    }
    return clamp(nt_channel(y, i), 0.0, 1.0);
}

fn surface_neural(in: VsOut, facing: bool) -> Surface {
    let m = materials[in.material];
    let base = m.neural;
    let extent = nt_extent(base);
    // The isotropic level of detail of hardware filtering (the longer footprint axis).
    let du = dpdx(in.uv) * vec2f(extent.xy);
    let dv = dpdy(in.uv) * vec2f(extent.xy);
    let lod = 0.5 * log2(max(max(dot(du, du), dot(dv, dv)), 1e-8));
    let top = f32(extent.z - 1u);
    let level = clamp(lod, 0.0, top);
    let m0 = u32(floor(level));
    let t = clamp((level - f32(m0) - (0.5 - 0.5 * NT_BLEND_BAND)) / NT_BLEND_BAND, 0.0, 1.0);
    // One decode, or two blended in the band: a loop around a single call, so the shader holds
    // one copy of the unrolled network (four inlined copies, one per branch, overflowed the
    // instruction caches where neighbouring pixels took different branches).
    let last = extent.z - 1u;
    let first_mip = select(m0, m0 + 1u, t >= 1.0 && m0 < last);
    let count = select(1u, 2u, t > 0.0 && t < 1.0 && m0 < last);
    var y: array<vec4<nt_t>, NT_OUT4>;
    for (var i = 0u; i < count; i++) {
        let d = nt_decode(base, first_mip + i, in.uv);
        let w = select(nt_t(1.0), select(nt_t(1.0 - t), nt_t(t), i == 1u), count == 2u);
        y[0] += d[0] * w;
        if NT_OUT4 > 1u { y[min(1u, NT_OUT4 - 1u)] += d[min(1u, NT_OUT4 - 1u)] * w; }
        if NT_OUT4 > 2u { y[min(2u, NT_OUT4 - 1u)] += d[min(2u, NT_OUT4 - 1u)] * w; }
        if NT_OUT4 > 3u { y[min(3u, NT_OUT4 - 1u)] += d[min(3u, NT_OUT4 - 1u)] * w; }
    }
    // Where each channel group is (descriptor word 1, a byte each; 0xff absent).
    let at = nt_data[base + 1u];
    let base_at = at.x & 0xffu;
    let normal_at = (at.x >> 8u) & 0xffu;
    let occlusion_at = (at.x >> 16u) & 0xffu;
    let rough_at = at.x >> 24u;
    let metal_at = at.y & 0xffu;
    let emissive_at = (at.y >> 8u) & 0xffu;

    var s: Surface;
    var albedo = vec3f(1.0);
    if base_at < NT_OUT {
        albedo = srgb_to_linear3(vec3f(nt_get(y, base_at, 1.0), nt_get(y, base_at + 1u, 1.0),
            nt_get(y, base_at + 2u, 1.0)));
    }
    s.albedo = albedo * m.base_color.rgb;
    s.alpha = m.base_color.a;
    s.metallic = m.metallic * nt_get(y, metal_at, 1.0);
    s.occlusion = nt_get(y, occlusion_at, 1.0);
    var tn = vec3f(0.0, 0.0, 1.0);
    let mapped = normal_at < NT_OUT;
    if mapped {
        let xy = vec2f(nt_get(y, normal_at, 0.5), nt_get(y, normal_at + 1u, 0.5)) * 2.0 - 1.0;
        tn = vec3f(xy, sqrt(max(1.0 - dot(xy, xy), 0.0)));
    }
    let n = shading_normal(in, facing, tn, mapped);
    s.n = n;
    var emissive = vec3f(1.0);
    if emissive_at < NT_OUT {
        emissive = srgb_to_linear3(vec3f(nt_get(y, emissive_at, 0.0), nt_get(y, emissive_at + 1u, 0.0),
            nt_get(y, emissive_at + 2u, 0.0)));
    }
    s.emissive = m.emissive * emissive;
    s.roughness = antialiased_roughness(n, m.roughness * nt_get(y, rough_at, 1.0));
    return s;
}

@fragment
fn fs_neural(in: VsOut, @builtin(front_facing) facing: bool) -> @location(0) vec4f {
    return shade(in, surface_neural(in, facing));
}
