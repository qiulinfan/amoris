// A sprite flashing white when hit (docs/design/sprites.md, Materials): params.x from 0 (as drawn)
// to 1 (white), its shape kept.
fn material(texel: vec4f, tint: vec4f, uv: vec2f, params: vec4f, time: f32) -> vec4f {
    let c = texel * tint;
    return vec4f(mix(c.rgb, vec3f(1.0), clamp(params.x, 0.0, 1.0)), c.a);
}
