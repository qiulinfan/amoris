// The real probe_gi.wgsl is prepended by the example. No inference logic is duplicated here.
struct NeuralProbeSample {
    world: vec4f,
    normal: vec4f,
};
@group(0) @binding(0) var<storage, read> samples: array<NeuralProbeSample>;
@group(0) @binding(1) var<storage, read_write> results: array<vec4f>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3u) {
    if id.x >= arrayLength(&samples) { return; }
    results[id.x] = neural_diffuse(samples[id.x].world.xyz, samples[id.x].normal.xyz);
}
