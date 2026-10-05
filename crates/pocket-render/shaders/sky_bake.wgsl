// The sky cube: one compute thread per texel of a face (atmosphere.wgsl is prepended).
struct SkyParams {
    sun: vec4f,              // toward the sun; w: illuminance
    color: vec4f,            // sun color; w: 1 for a flat color sky
    flat_color: vec4f,
    size: vec4u,             // face size
};

@group(0) @binding(0) var<uniform> sp: SkyParams;
@group(0) @binding(1) var out_cube: texture_storage_2d_array<rgba16float, write>;

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

@compute @workgroup_size(8, 8, 1)
fn bake(@builtin(global_invocation_id) id: vec3u) {
    let n = sp.size.x;
    if (id.x >= n || id.y >= n) {
        return;
    }
    let uv = (vec2f(id.xy) + 0.5) / f32(n);
    let dir = face_dir(id.z, uv);
    var c: vec3f;
    if (sp.color.w > 0.5) {
        c = sp.flat_color.rgb;
    } else {
        // Below the horizon the ground reflects a dim, sky-tinted light.
        let d = vec3f(dir.x, max(dir.y, 0.0), dir.z);
        c = atmosphere(normalize(d), sp.sun.xyz, 200.0) * sp.color.rgb * sp.sun.w;
        if (dir.y < 0.0) {
            let ground = vec3f(0.18, 0.17, 0.15) * max(sp.sun.y, 0.0) * sp.sun.w / PI;
            c = mix(c, ground + c * 0.3, smoothstep(0.0, -0.08, dir.y));
        }
    }
    textureStore(out_cube, vec2u(id.xy), id.z, vec4f(c, 1.0));
}

