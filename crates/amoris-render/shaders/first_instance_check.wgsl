// The first-instance check (gpu.rs, `indirect_draws_keep_their_first_instance`): instance i covers
// pixel column i of a one-pixel-high viewport and writes i + 1, so a draw whose first instance is
// lost (Direct3D 12 without wgpu's indirect validation, docs/bench/dx12.md 2.1) fills the wrong
// columns.

const COLUMNS: f32 = 8.0;

struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) id: u32,
}

@vertex
fn vs(@builtin(vertex_index) corner: u32, @builtin(instance_index) instance: u32) -> Out {
    let x = f32(instance + (corner & 1u)) / COLUMNS * 2.0 - 1.0;
    let y = select(-1.0, 1.0, (corner & 2u) != 0u);
    return Out(vec4(x, y, 0.0, 1.0), instance + 1u);
}

@fragment
fn fs(in: Out) -> @location(0) u32 {
    return in.id;
}
