// Small online radiance cache. Training labels are uncached traced reflected path tails.
// Loss is MSE in log(1 + radiance / scale); the biased estimate is an explicit toy choice.
const NRC_INPUTS: u32 = 14u;
const NRC_HIDDEN: u32 = 32u;
const NRC_PARAMETERS: u32 = 1635u;
const NRC_SECOND: u32 = 480u;
const NRC_OUTPUT: u32 = 1536u;

struct NrcParams {
    origin: vec4f,
    extent: vec4f,
    // batch capacity, flags (train=1, query=2), Adam update number, warmup updates
    shape: vec4u,
    // learning rate, radiance scale, gradient clip, maximum predicted log radiance
    training: vec4f,
    // minimum query depth, reserved
    query: vec4u,
}

struct NrcRecord {
    features: array<vec4f, 4>,
    label: vec4f,
}

@group(1) @binding(0) var<uniform> nrc_params: NrcParams;
@group(1) @binding(1) var<storage, read_write> nrc_weights: array<f32>;
@group(1) @binding(2) var<storage, read_write> nrc_records: array<NrcRecord>;
@group(1) @binding(3) var<storage, read_write> nrc_gradients: array<f32>;
@group(1) @binding(4) var<storage, read_write> nrc_moments: array<vec2f>;
// requested records, queries, invalid records, invalid updates, overflow, trained count,
// invalid inference, persistent successful update count; first seven cleared each frame.
@group(1) @binding(5) var<storage, read_write> nrc_stats: array<atomic<u32>>;
@group(1) @binding(6) var<storage, read_write> nrc_losses: array<f32>;

fn nrc_finite3(v: vec3f) -> bool {
    return all(abs(v) < vec3f(1e30));
}

fn nrc_feature_record(position: vec3f, normal: vec3f, outgoing: vec3f,
                      base: vec3f, roughness: f32, metallic: f32, label: vec3f) -> NrcRecord {
    let p = clamp(2.0 * (position - nrc_params.origin.xyz) / nrc_params.extent.xyz - 1.0,
                  vec3f(-1.0), vec3f(1.0));
    let n = clamp(normal, vec3f(-1.0), vec3f(1.0));
    let o = clamp(outgoing, vec3f(-1.0), vec3f(1.0));
    let c = clamp(base, vec3f(0.0), vec3f(1.0));
    return NrcRecord(array<vec4f, 4>(vec4f(p, n.x), vec4f(n.yz, o.xy),
                        vec4f(o.z, c), vec4f(clamp(roughness, 0.0, 1.0),
                            clamp(metallic, 0.0, 1.0), 0.0, 0.0)), vec4f(label, 0.0));
}

fn nrc_train_enabled() -> bool { return (nrc_params.shape.y & 1u) != 0u; }

fn nrc_query_allowed(depth: u32, roughness: f32, transmission: f32) -> bool {
    return (nrc_params.shape.y & 2u) != 0u &&
        atomicLoad(&nrc_stats[7]) >= nrc_params.shape.w && depth >= nrc_params.query.x &&
        roughness >= 0.2 && transmission <= 0.001;
}

fn nrc_record(record: NrcRecord) {
    if !nrc_train_enabled() { return; }
    var valid = nrc_finite3(record.label.xyz) && all(record.label.xyz >= vec3f(0.0));
    for (var i = 0u; i < 4u; i++) { valid = valid && all(abs(record.features[i]) < vec4f(1e30)); }
    if !valid { atomicAdd(&nrc_stats[2], 1u); return; }
    let i = atomicAdd(&nrc_stats[0], 1u);
    if i >= nrc_params.shape.x { atomicAdd(&nrc_stats[4], 1u); return; }
    nrc_records[i] = record;
}

struct NrcForward {
    a: array<f32, 32>,
    b: array<f32, 32>,
    output: vec3f,
}

fn nrc_forward(record: NrcRecord) -> NrcForward {
    var result: NrcForward;
    for (var j = 0u; j < 32u; j++) {
        var v = nrc_weights[448u + j];
        for (var i = 0u; i < 14u; i++) { v += nrc_weights[j * 14u + i] * record.features[i / 4u][i % 4u]; }
        result.a[j] = max(v, 0.0);
    }
    for (var j = 0u; j < 32u; j++) {
        var v = nrc_weights[1504u + j];
        for (var i = 0u; i < 32u; i++) { v += nrc_weights[480u + j * 32u + i] * result.a[i]; }
        result.b[j] = max(v, 0.0);
    }
    for (var j = 0u; j < 3u; j++) {
        var v = nrc_weights[1632u + j];
        for (var i = 0u; i < 32u; i++) { v += nrc_weights[1536u + j * 32u + i] * result.b[i]; }
        result.output[j] = v;
    }
    return result;
}

fn nrc_infer(record: NrcRecord) -> vec3f {
    atomicAdd(&nrc_stats[1], 1u);
    let prediction = nrc_forward(record).output;
    if !nrc_finite3(prediction) { atomicAdd(&nrc_stats[6], 1u); return vec3f(0.0); }
    let radiance = (exp(clamp(prediction, vec3f(0.0), vec3f(nrc_params.training.w))) - 1.0) * nrc_params.training.y;
    if !nrc_finite3(radiance) { atomicAdd(&nrc_stats[6], 1u); return vec3f(0.0); }
    return radiance;
}

// One invocation owns a whole training record and its gradients. No float atomics or cross-lane
// accumulation: the following parameter pass averages gradients before applying Adam.
@compute @workgroup_size(32)
fn nrc_backward(@builtin(global_invocation_id) gid: vec3u) {
    let sample = gid.x;
    let count = min(atomicLoad(&nrc_stats[0]), nrc_params.shape.x);
    if sample >= count { return; }
    if sample == 0u { atomicAdd(&nrc_stats[7], 1u); }
    let record = nrc_records[sample];
    let f = nrc_forward(record);
    let label = log(1.0 + min(record.label.xyz / nrc_params.training.y, vec3f(1e20)));
    let difference = f.output - label;
    nrc_losses[sample] = dot(difference, difference) / 3.0;
    let dc = difference * (2.0 / 3.0);
    var db: array<f32, 32>;
    var da: array<f32, 32>;
    let start = sample * 1635u;
    for (var j = 0u; j < 3u; j++) {
        nrc_gradients[start + 1632u + j] = dc[j];
        for (var i = 0u; i < 32u; i++) { nrc_gradients[start + 1536u + j * 32u + i] = dc[j] * f.b[i]; }
    }
    for (var i = 0u; i < 32u; i++) {
        var d = 0.0;
        for (var j = 0u; j < 3u; j++) { d += nrc_weights[1536u + j * 32u + i] * dc[j]; }
        db[i] = select(0.0, d, f.b[i] > 0.0);
    }
    for (var j = 0u; j < 32u; j++) {
        nrc_gradients[start + 1504u + j] = db[j];
        for (var i = 0u; i < 32u; i++) { nrc_gradients[start + 480u + j * 32u + i] = db[j] * f.a[i]; }
    }
    for (var i = 0u; i < 32u; i++) {
        var d = 0.0;
        for (var j = 0u; j < 32u; j++) { d += nrc_weights[480u + j * 32u + i] * db[j]; }
        da[i] = select(0.0, d, f.a[i] > 0.0);
    }
    for (var j = 0u; j < 32u; j++) {
        nrc_gradients[start + 448u + j] = da[j];
        for (var i = 0u; i < 14u; i++) { nrc_gradients[start + j * 14u + i] = da[j] * record.features[i / 4u][i % 4u]; }
    }
}

@compute @workgroup_size(64)
fn nrc_adam(@builtin(global_invocation_id) gid: vec3u) {
    let parameter = gid.x;
    if parameter >= 1635u { return; }
    let count = min(atomicLoad(&nrc_stats[0]), nrc_params.shape.x);
    if count == 0u { return; }
    var gradient = 0.0;
    for (var sample = 0u; sample < count; sample++) { gradient += nrc_gradients[sample * 1635u + parameter]; }
    gradient /= f32(count);
    if !(abs(gradient) < 1e30) { atomicAdd(&nrc_stats[3], 1u); return; }
    gradient = clamp(gradient, -nrc_params.training.z, nrc_params.training.z);
    let old = nrc_moments[parameter];
    let m = 0.9 * old.x + 0.1 * gradient;
    let v = 0.999 * old.y + 0.001 * gradient * gradient;
    let t = f32(max(atomicLoad(&nrc_stats[7]), 1u));
    let step = nrc_params.training.x * (m / (1.0 - pow(0.9, t))) /
                    (sqrt(v / (1.0 - pow(0.999, t))) + 1e-8);
    let updated = nrc_weights[parameter] - step;
    if !(abs(updated) < 1e30) { atomicAdd(&nrc_stats[3], 1u); return; }
    nrc_moments[parameter] = vec2f(m, v);
    nrc_weights[parameter] = updated;
    if parameter == 0u { atomicStore(&nrc_stats[5], count); }
}
