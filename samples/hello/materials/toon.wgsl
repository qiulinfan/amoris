// Toon shading (docs/design/rendering.md, Materials): the sun's light in three flat steps over the
// surface's colour, a rim of light where the surface turns away from the eye, and params.x mixing
// in a flat colour (params.yzw) for a hit's flash.
fn material(lit: vec4f, s: Surface) -> vec4f {
    let ndl = max(dot(s.normal, s.sun_dir), 0.0);
    let steps = select(select(0.35, 0.7, ndl > 0.2), 1.0, ndl > 0.6);
    let rim = pow(1.0 - max(dot(s.normal, s.view), 0.0), 3.0) * 0.4;
    let c = s.base.rgb * s.sun_color * steps + s.base.rgb * 0.15 + vec3f(rim);
    return vec4f(mix(c, s.params.yzw, clamp(s.params.x, 0.0, 1.0)), 1.0);
}
