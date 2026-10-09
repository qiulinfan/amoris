enable f16;
// Neural texture decoding in half precision (shader-f16): the network's arithmetic runs on halves,
// read as halves through a second view of the data uniform, `nt_blocks` (WGSL cannot bitcast a
// u32 to two halves, and naga has no 32-to-16-bit vector bitcast), bound at the includer's
// `NT_GROUP` and `NT_BLOCKS`. Composed first in its module (`enable` must lead), then the profile
// constants and neural_texture.wgsl (neural.rs).
alias nt_t = f16;

// Two data words: a 4x4 weight block, or (c0 or c2) a word of four biases.
struct NtBlock {
    c0: vec4<f16>,
    c1: vec4<f16>,
    c2: vec4<f16>,
    c3: vec4<f16>,
};

@group(NT_GROUP) @binding(NT_BLOCKS) var<uniform> nt_blocks: array<NtBlock, 2048>;

// The 4x4 weight block at data word `at` (even: blocks are aligned to two words).
fn nt_mat(at: u32) -> mat4x4<f16> {
    let b = nt_blocks[at / 2u];
    return mat4x4<f16>(b.c0, b.c1, b.c2, b.c3);
}

// The four biases at data word `at`.
fn nt_bias(at: u32) -> vec4<f16> {
    let b = nt_blocks[at / 2u];
    return select(b.c2, b.c0, at % 2u == 0u);
}
