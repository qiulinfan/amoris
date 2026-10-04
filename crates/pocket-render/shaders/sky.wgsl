// Drawing the sky behind the scene: the baked cube plus the sun disk, fog toward the horizon.

@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var sky_cube: texture_cube<f32>;
@group(0) @binding(2) var sky_sampler: sampler;

struct SkyOut {
    @builtin(position) clip: vec4f,
    @location(0) ndc: vec2f,
};

@vertex
fn vs_sky(@builtin(vertex_index) i: u32) -> SkyOut {
    let p = vec2f(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
    var o: SkyOut;
    o.clip = vec4f(p, 0.0, 1.0);
    o.ndc = p;
    return o;
}

@fragment
fn fs_sky(in: SkyOut) -> @location(0) vec4f {
    let far = view.inv_view_proj * vec4f(in.ndc, 1e-7, 1.0);
    let dir = normalize(far.xyz / far.w - view.camera_pos.xyz);
    var c = textureSampleLevel(sky_cube, sky_sampler, dir, 0.0).rgb;
    if (view.params.w < 0.5 && view.sun_dir.w > 0.0) {
        let cos_sun = dot(dir, view.sun_dir.xyz);
        let disk = 0.99995;
        if (cos_sun > disk) {
            let x = (cos_sun - disk) / (1.0 - disk);
            let limb = pow(clamp(x, 0.0, 1.0), 0.4);
            c += view.sun_color.rgb * view.sun_dir.w * 2000.0 * limb * smoothstep(-0.02, 0.02, view.sun_dir.y);
        }
    }
    if (view.fog.w > 0.0) {
        let horizon = exp(-abs(dir.y) * 8.0);
        c = mix(c, view.fog.rgb * view.sun_color.w, clamp(horizon * min(view.fog.w * 200.0, 1.0), 0.0, 1.0));
    }
    return vec4f(c, 1.0);
}
