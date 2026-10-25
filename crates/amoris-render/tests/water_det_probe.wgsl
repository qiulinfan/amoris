// tests/water_det.rs's probe of the deterministic arithmetic: from each input's four operands, every
// operation the generator rounds and keeps apart, the patterns compilers fuse or reorder, and every
// function of water_det.wgsl. The test runs the generated form and compares each result's bits with
// the same arithmetic on the CPU.
struct Params { count: u32, unused: vec3u }
@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var<storage, read> inputs: array<vec4f>;
@group(0) @binding(2) var<storage, read_write> outputs: array<f32>;

const OUTPUTS: u32 = 18u;

@compute @workgroup_size(64)
fn probe(@builtin(global_invocation_id) id: vec3u) {
    if (id.x >= p.count) { return; }
    let v = inputs[id.x];
    let a = v.x; let b = v.y; let c = v.z; let d = v.w;
    let o = id.x * OUTPUTS;
    outputs[o] = a + b;
    outputs[o + 1u] = a - b;
    outputs[o + 2u] = a * b;
    // A multiply-add a device may fuse, a sum it may reorder, and a product that underflows.
    outputs[o + 3u] = a * b + c;
    outputs[o + 4u] = (a + b) + c;
    outputs[o + 5u] = a * 1e-20 * 1e-20;
    outputs[o + 6u] = -(a * 0.0);
    outputs[o + 7u] = a / b;
    outputs[o + 8u] = sqrt(abs(a));
    outputs[o + 9u] = exp(clamp(c, -80.0, 80.0));
    outputs[o + 10u] = log(abs(a) + 1.0);
    outputs[o + 11u] = pow(abs(b) + 0.5, clamp(c, -4.0, 4.0));
    outputs[o + 12u] = sin(c);
    outputs[o + 13u] = cos(c);
    outputs[o + 14u] = tanh(d);
    outputs[o + 15u] = acosh(abs(d) + 1.0);
    outputs[o + 16u] = smoothstep(-1.0, 1.0, d);
    // A conversion of an integer f32 cannot hold exactly.
    outputs[o + 17u] = f32(bitcast<u32>(d) | 0x01000001u);
}
