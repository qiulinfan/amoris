// Neural texture decoding in single precision (devices without shader-f16): the weights stay
// halves in memory and are widened as they are read from `nt_data`. Composed before the profile
// constants and neural_texture.wgsl (neural.rs).
alias nt_t = f32;

fn nt_half4(w: vec2u) -> vec4f {
    return vec4f(unpack2x16float(w.x), unpack2x16float(w.y));
}

// The 4x4 weight block at data word `at` (two words, column-major halves).
fn nt_mat(at: u32) -> mat4x4f {
    let a = nt_data[at];
    let b = nt_data[at + 1u];
    return mat4x4f(nt_half4(a.xy), nt_half4(a.zw), nt_half4(b.xy), nt_half4(b.zw));
}

// The four biases at data word `at` (its first two words' halves).
fn nt_bias(at: u32) -> vec4f {
    return nt_half4(nt_data[at].xy);
}
