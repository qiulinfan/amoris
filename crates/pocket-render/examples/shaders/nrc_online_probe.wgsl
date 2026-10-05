// Synthetic correctness fixture only; real path tracing supplies its own uncached labels.
@group(0) @binding(0) var<storage, read_write> nrc_probe_predictions: array<vec4f>;

@compute @workgroup_size(32)
fn nrc_probe_fill(@builtin(global_invocation_id) gid: vec3u) {
    if gid.x >= nrc_params.shape.x { return; }
    let x = f32(gid.x) / f32(max(nrc_params.shape.x - 1u, 1u));
    let position = vec3f(2.0 * x - 1.0, 0.0, 0.0);
    let multiplier = select(1.0, 2.5, nrc_params.shape.z > 64u);
    let label = vec3f(0.4 + 0.4 * x, 0.2 + 0.2 * x, 0.1 + 0.1 * x) * multiplier;
    nrc_record(nrc_feature_record(position, vec3f(0.0, 1.0, 0.0), vec3f(0.0, 1.0, 0.0),
                                 vec3f(0.6), 0.8, 0.0, label));
}

@compute @workgroup_size(32)
fn nrc_probe_eval(@builtin(global_invocation_id) gid: vec3u) {
    let count = min(atomicLoad(&nrc_stats[0]), nrc_params.shape.x);
    if gid.x >= count { return; }
    nrc_probe_predictions[gid.x] = vec4f(nrc_infer(nrc_records[gid.x]), 1.0);
}
