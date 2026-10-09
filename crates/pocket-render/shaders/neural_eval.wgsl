// Decodes every texel of one mip through the runtime's decoder (neural_texture.wgsl), for the
// encoder's quality report and the parity tests against the CPU reference. Composed after the
// precision, the profile constants and neural_texture.wgsl.

// Where neural_f16.wgsl's second view of the data uniform binds (unused in single precision).
const NT_GROUP: u32 = 0u;
const NT_BLOCKS: u32 = 4u;

@group(0) @binding(0) var nt_latents: texture_2d<u32>;
@group(0) @binding(1) var<uniform> nt_data: array<vec4u, 4096>;
// The texture's descriptor base, the mip, its width and height.
@group(0) @binding(2) var<uniform> eval: vec4u;
// width * height * NT_OUT floats, channel fastest.
@group(0) @binding(3) var<storage, read_write> decoded: array<f32>;

@compute @workgroup_size(8, 8)
fn decode_mip(@builtin(global_invocation_id) gid: vec3u) {
    if gid.x >= eval.z || gid.y >= eval.w {
        return;
    }
    let uv = (vec2f(gid.xy) + 0.5) / vec2f(eval.zw);
    let y = nt_decode(eval.x, eval.y, uv);
    let at = (gid.y * eval.z + gid.x) * NT_OUT;
    for (var c = 0u; c < NT_OUT; c++) {
        decoded[at + c] = nt_channel(y, c);
    }
}
