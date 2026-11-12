// Game UI (docs/spec/ui.md 4): textured, coloured quads in pixels drawn over the final image.
// Glyph and solid quads (mode 0) take their coverage from the R8 atlas (bars and boxes from its
// white texel); image quads (mode 1) take the RGBA image bound as group 1, times their tint.

struct UiParams {
    screen: vec4f,     // width, height in device pixels; z: output is sRGB-encoded (1/0)
};

@group(0) @binding(0) var<uniform> up: UiParams;
@group(0) @binding(1) var atlas: texture_2d<f32>;
@group(0) @binding(2) var atlas_sampler: sampler;
@group(1) @binding(0) var image: texture_2d<f32>;

struct In {
    @location(0) pos: vec2f,
    @location(1) uv: vec2f,
    @location(2) color: vec4f,
    @location(3) mode: f32,
};

struct Out {
    @builtin(position) clip: vec4f,
    @location(0) uv: vec2f,
    @location(1) color: vec4f,
    @location(2) @interpolate(flat) mode: f32,
};

@vertex
fn vs(v: In) -> Out {
    var o: Out;
    let ndc = v.pos / up.screen.xy * vec2f(2.0, -2.0) + vec2f(-1.0, 1.0);
    o.clip = vec4f(ndc, 0.0, 1.0);
    o.uv = v.uv;
    o.color = v.color;
    o.mode = v.mode;
    return o;
}

// sRGB-encoded from linear rgb (an image texture is sampled linear).
fn ui_linear_to_srgb(c: vec3f) -> vec3f {
    return select(1.055 * pow(c, vec3f(1.0 / 2.4)) - 0.055, c * 12.92, c <= vec3f(0.0031308));
}

@fragment
fn fs(in: Out) -> @location(0) vec4f {
    // Both sampled in uniform control flow; the mode picks one.
    let a = textureSample(atlas, atlas_sampler, in.uv).r;
    let t = textureSample(image, atlas_sampler, in.uv);
    var c = in.color;
    if (up.screen.z > 0.5) {
        c = vec4f(srgb_to_linear(c.rgb), c.a);
    }
    if (in.mode > 0.5) {
        var rgb = t.rgb;
        if (up.screen.z < 0.5) {
            rgb = ui_linear_to_srgb(rgb);
        }
        return vec4f(c.rgb * rgb, c.a * t.a);
    }
    return vec4f(c.rgb, c.a * a);
}
