// The top level of rt_shadows.rs and what a ray reads at a hit (docs/spec/rt-shadows.md; water.md
// 67): appended to the forward shader, with rt_shadows.wgsl, rt_reflections.wgsl or both, when the
// device has ray queries. Instances carry masks: RT_CASTS for the shadow rays (the casters the
// cascades would draw), RT_SEEN for the water's reflected rays (every opaque instance drawn).

const RT_CASTS: u32 = 1u;
const RT_SEEN: u32 = 2u;

// One top-level instance: its material row and its mesh's range in the pool (rt_shadows.rs).
struct RtCaster {
    material: u32,
    first_index: u32,
    base_vertex: i32,
    _pad: u32,
};

@group(1) @binding(11) var rt_casters: acceleration_structure;
@group(1) @binding(12) var<storage, read> rt_caster_info: array<RtCaster>;
// The mesh pool: indices, and vertices as 12 floats (position, normal, uv, tangent).
@group(1) @binding(13) var<storage, read> rt_indices: array<u32>;
@group(1) @binding(14) var<storage, read> rt_vertices: array<f32>;

fn rt_uv(base_vertex: i32, index: u32) -> vec2f {
    let v = u32(base_vertex + i32(index)) * 12u;
    return vec2f(rt_vertices[v + 6u], rt_vertices[v + 7u]);
}

// Whether a candidate hit on an alpha-masked caster (non-opaque geometry) blocks the sun: the
// masked shadow pass's test (fs_shadow_masked) at the hit's interpolated texture coordinate.
fn rt_blocks(hit: RayIntersection) -> bool {
    let caster = rt_caster_info[hit.instance_index];
    let m = materials[caster.material];
    var a = m.base_color.a;
    if (m.base_color_tex != NO_TEXTURE) {
        let i = caster.first_index + hit.primitive_index * 3u;
        let b = vec3f(1.0 - hit.barycentrics.x - hit.barycentrics.y, hit.barycentrics);
        let uv = rt_uv(caster.base_vertex, rt_indices[i]) * b.x
            + rt_uv(caster.base_vertex, rt_indices[i + 1u]) * b.y
            + rt_uv(caster.base_vertex, rt_indices[i + 2u]) * b.z;
        a *= textureSampleLevel(tex_srgb, tex_sampler, uv * m.uv_scale, m.base_color_tex, 0.0).a;
    }
    // As fs_shadow_masked: a transparent caster's opacity leaves out what it transmits.
    a *= 1.0 - m.transmission;
    return a >= m.alpha_cutoff;
}
