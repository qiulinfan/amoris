// Numeric probe directly calls the renderer's production pt_bsdf.wgsl functions.
struct BsdfProbeInput { base: vec4<f32>, surface: vec4<f32>, wo: vec4<f32>, wi: vec4<f32>, random: vec4<f32> };
struct BsdfProbeOutput { evaluation: vec4<f32>, direction: vec4<f32>, weight: vec4<f32>, diagnostics: vec4<f32> };
@group(0) @binding(0) var<storage, read> probe_inputs: array<BsdfProbeInput>;
@group(0) @binding(1) var<storage, read_write> probe_outputs: array<BsdfProbeOutput>;
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= arrayLength(&probe_inputs) { return; }
    let i = probe_inputs[gid.x];
    let b = PtBsdf(i.base.rgb, i.base.a, i.surface.x, i.surface.y, i.surface.z);
    let evaluated = pt_bsdf_eval(b, i.wo.xyz, i.wi.xyz);
    let sampled = pt_bsdf_sample(b, i.wo.xyz, i.random);
    probe_outputs[gid.x] = BsdfProbeOutput(vec4<f32>(evaluated.value, evaluated.pdf),
        vec4<f32>(sampled.direction, sampled.pdf), vec4<f32>(sampled.weight, f32(sampled.delta)),
        vec4<f32>(sampled.eta_scale, pt_fresnel(i.wo.z, b.eta), 0.0, 0.0));
}
