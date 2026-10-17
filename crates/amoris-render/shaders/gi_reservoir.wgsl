// Single-secondary-surface ReSTIR GI prototype: area-measure RIS and reservoir reuse.
// Compose this file with the ray tracer. It does not trace visibility or supply the complete
// ReSTIR GI support/M correction or generalized ReSTIR PT path reconnection.
// Fixed surface-point reuse stays in the same area domain; the initial solid-angle -> area
// conversion already contains the geometry Jacobian. Do not apply this conversion twice.
// A caller MUST ray-test receiver -> selected second_position with an origin offset and verify
// selected identity.x (primitive) before passing visibility=1 to gi_reservoir_estimate.

// 64-byte candidate. identity = (primitive, material, valid=1, reserved).
struct GiCandidate {
    second_position: vec4f,
    second_normal: vec4f,
    outgoing_radiance: vec4f,
    identity: vec4u,
}

// 80-byte reservoir. M counts valid proposals, including proposals with zero target.
struct GiReservoir {
    candidate: GiCandidate,
    sum_weights: f32,
    selected_target: f32,
    M: u32,
    valid: u32,
}

fn gi_finite(value: f32) -> bool {
    return (bitcast<u32>(value) & 0x7f800000u) != 0x7f800000u;
}
fn gi_finite3(value: vec3f) -> bool {
    return all((bitcast<vec3u>(value) & vec3u(0x7f800000u)) != vec3u(0x7f800000u));
}
fn gi_unit_normal(normal: vec3f) -> bool {
    return gi_finite3(normal) && abs(dot(normal, normal) - 1.0) <= 0.001;
}
fn gi_candidate_valid(candidate: GiCandidate) -> bool {
    return candidate.identity.z == 1u && gi_finite3(candidate.second_position.xyz)
        && gi_unit_normal(candidate.second_normal.xyz)
        && gi_finite3(candidate.outgoing_radiance.xyz)
        && all(candidate.outgoing_radiance.xyz >= vec3f(0.0));
}

fn gi_reservoir_empty() -> GiReservoir {
    return GiReservoir(GiCandidate(vec4f(0.0), vec4f(0.0), vec4f(0.0), vec4u(0u)),
        0.0, 0.0, 0u, 0u);
}

// f_A = albedo / pi * Lo * (cos1 * cos2 / distance²), before selected-candidate visibility.
fn gi_candidate_evaluate(receiver_position: vec3f, receiver_normal: vec3f,
    receiver_albedo: vec3f, candidate: GiCandidate) -> vec3f {
    if (!gi_candidate_valid(candidate) || !gi_finite3(receiver_position)
        || !gi_unit_normal(receiver_normal) || !gi_finite3(receiver_albedo)
        || any(receiver_albedo < vec3f(0.0))) {
        return vec3f(0.0);
    }
    let delta = candidate.second_position.xyz - receiver_position;
    let d2 = dot(delta, delta);
    if (!gi_finite(d2) || d2 <= 1e-8) { return vec3f(0.0); }
    let direction = delta * inverseSqrt(d2);
    let cos1 = max(dot(receiver_normal, direction), 0.0);
    let cos2 = max(dot(candidate.second_normal.xyz, -direction), 0.0);
    let result = receiver_albedo * candidate.outgoing_radiance.xyz * (cos1 * cos2 / (3.14159265359 * d2));
    if (!gi_finite3(result)) { return vec3f(0.0); }
    return result;
}

fn gi_candidate_area_pdf(receiver_position: vec3f, candidate: GiCandidate, pdf_omega: f32) -> f32 {
    if (!gi_candidate_valid(candidate) || !gi_finite3(receiver_position)
        || !gi_finite(pdf_omega) || pdf_omega <= 0.0) { return 0.0; }
    let delta = candidate.second_position.xyz - receiver_position;
    let d2 = dot(delta, delta);
    if (!gi_finite(d2) || d2 <= 1e-8) { return 0.0; }
    let direction = delta * inverseSqrt(d2);
    let result = pdf_omega * max(dot(candidate.second_normal.xyz, -direction), 0.0) / d2;
    if (!gi_finite(result)) { return 0.0; }
    return result;
}

fn gi_candidate_target(contribution: vec3f) -> f32 {
    if (!gi_finite3(contribution) || any(contribution < vec3f(0.0))) { return 0.0; }
    let result = dot(contribution, vec3f(0.2126, 0.7152, 0.0722));
    if (!gi_finite(result)) { return 0.0; }
    return result;
}

fn gi_reservoir_normalization(reservoir: GiReservoir) -> f32 {
    if (reservoir.valid != 1u || reservoir.M == 0u || reservoir.selected_target <= 0.0
        || !gi_candidate_valid(reservoir.candidate) || !gi_finite(reservoir.sum_weights)
        || reservoir.sum_weights <= 0.0 || !gi_finite(reservoir.selected_target)) { return 0.0; }
    let result = reservoir.sum_weights / (f32(reservoir.M) * reservoir.selected_target);
    if (!gi_finite(result) || result < 0.0) { return 0.0; }
    return result;
}

// Returns whether candidate selection changed. Invalid inputs are rejected without mutation;
// accepted zero-target proposals count represented_M but retain any previous candidate.
fn gi_reservoir_update_weight(reservoir: ptr<function, GiReservoir>, candidate: GiCandidate,
    p_hat: f32, weight: f32, represented_M: u32, random: f32) -> bool {
    if (!gi_candidate_valid(candidate) || !gi_finite(p_hat) || p_hat < 0.0
        || !gi_finite(weight) || weight < 0.0 || !gi_finite(random)
        || random < 0.0 || random >= 1.0 || represented_M == 0u
        || ((p_hat == 0.0) != (weight == 0.0))) { return false; }
    let old = *reservoir;
    if (old.valid > 1u || !gi_finite(old.sum_weights) || old.sum_weights < 0.0
        || !gi_finite(old.selected_target) || old.selected_target < 0.0
        || old.M > 0xffffffffu - represented_M) { return false; }
    if (old.valid == 1u && gi_reservoir_normalization(old) <= 0.0) { return false; }
    if (old.valid != 1u && (old.sum_weights != 0.0 || old.selected_target != 0.0)) { return false; }
    let sum_weights = old.sum_weights + weight;
    if (!gi_finite(sum_weights)) { return false; }
    (*reservoir).M = old.M + represented_M;
    (*reservoir).sum_weights = sum_weights;
    let selected = weight > 0.0 && random * sum_weights < weight;
    if (selected) {
        (*reservoir).candidate = candidate;
        (*reservoir).selected_target = p_hat;
        (*reservoir).valid = 1u;
    }
    return selected;
}

fn gi_reservoir_update(reservoir: ptr<function, GiReservoir>, candidate: GiCandidate,
    p_hat: f32, pdf_area: f32, random: f32) -> bool {
    if (!gi_finite(pdf_area) || pdf_area <= 0.0) { return false; }
    return gi_reservoir_update_weight(reservoir, candidate, p_hat, p_hat / pdf_area, 1u, random);
}

// Call only after admitting a compatible source and reevaluating target at the new receiver.
// W_source * M_source is the source reservoir's effective sample weight in area measure.
fn gi_reservoir_merge(reservoir: ptr<function, GiReservoir>, source: GiReservoir,
    reevaluated_target: f32, random: f32) -> bool {
    let normalization = gi_reservoir_normalization(source);
    if (normalization <= 0.0) { return false; }
    return gi_reservoir_update_weight(reservoir, source.candidate, reevaluated_target,
        reevaluated_target * normalization * f32(source.M), source.M, random);
}

// Bound history influence before reuse. Keeps W unchanged; it does not remove reuse bias.
fn gi_reservoir_limit_history(reservoir: ptr<function, GiReservoir>, maximum_M: u32) {
    if (maximum_M == 0u) { return; }
    if ((*reservoir).M > maximum_M) {
        (*reservoir).sum_weights *= f32(maximum_M) / f32((*reservoir).M);
        (*reservoir).M = maximum_M;
    }
}

fn gi_reservoir_estimate(reservoir: GiReservoir, receiver_position: vec3f,
    receiver_normal: vec3f, receiver_albedo: vec3f, visibility: f32) -> vec3f {
    if (!gi_finite(visibility) || visibility <= 0.0 || visibility > 1.0) { return vec3f(0.0); }
    let result = gi_candidate_evaluate(receiver_position, receiver_normal, receiver_albedo,
        reservoir.candidate) * gi_reservoir_normalization(reservoir) * visibility;
    if (!gi_finite3(result)) { return vec3f(0.0); }
    return result;
}

// history_valid must include reprojection/depth/primitive validity; scene epochs cover static
// geometry/material/light configuration. Temporal source must be older, spatial source same-frame.
fn gi_reuse_allowed(current_epoch: u32, source_epoch: u32, current_frame: u32, source_frame: u32,
    temporal: bool, history_valid: bool, scene_static: bool, current_normal: vec3f,
    source_normal: vec3f, minimum_normal_cosine: f32) -> bool {
    return history_valid && scene_static && current_epoch == source_epoch
        && ((!temporal && current_frame == source_frame) || (temporal && source_frame < current_frame))
        && gi_unit_normal(current_normal) && gi_unit_normal(source_normal)
        && gi_finite(minimum_normal_cosine) && minimum_normal_cosine >= -1.0
        && minimum_normal_cosine <= 1.0 && dot(current_normal, source_normal) >= minimum_normal_cosine;
}
