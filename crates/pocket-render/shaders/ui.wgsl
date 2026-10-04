// Game UI: textured, coloured quads in pixels (glyphs from the atlas, bars from its white texel),
// drawn over the final image.

struct UiParams {
    screen: vec4f,     // width, height in device pixels; z: output is sRGB-encoded (1/0)
};

@group(0) @binding(0) var<uniform> up: UiParams;
@group(0) @binding(1) var atlas: texture_2d<f32>;
@group(0) @binding(2) var atlas_sampler: sampler;

struct In {
    @location(0) pos: vec2f,
    @location(1) uv: vec2f,
    @location(2) color: vec4f,
};

struct Out {
    @builtin(position) clip: vec4f,
    @location(0) uv: vec2f,
    @location(1) color: vec4f,
};

@vertex
fn vs(v: In) -> Out {
    var o: Out;
    let ndc = v.pos / up.screen.xy * vec2f(2.0, -2.0) + vec2f(-1.0, 1.0);
    o.clip = vec4f(ndc, 0.0, 1.0);
    o.uv = v.uv;
    o.color = v.color;
    return o;
}

@fragment
fn fs(in: Out) -> @location(0) vec4f {
    let a = textureSample(atlas, atlas_sampler, in.uv).r;
    var c = in.color;
    if (up.screen.z > 0.5) {
        c = vec4f(srgb_to_linear(c.rgb), c.a);
    }
    return vec4f(c.rgb, c.a * a);
}
