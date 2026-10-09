// Independent WGSL diffuse tracer and SHaRC-style Update/Resolve/Query prototype.
// Algorithm reference: NVIDIA-RTX/SHARC/docs/Integration.md (retrieved 2026-10-05).
// No NVIDIA SDK shader code is included. The exact hash key and float32 history are portable.
enable wgpu_ray_query;

const PI: f32 = 3.141592653589793;
const INVALID: u32 = 0xffffffffu;
const MAX_BOUNCES: u32 = 8u;
const QUANTIZATION: f32 = 512.0;
const MAX_CELL_SAMPLES: u32 = 65535u;
// 65535 accepted samples * 128 * 512 <= u32::MAX, including worst-case rounding.
const MAX_CACHE_RADIANCE: f32 = 128.0;

struct Parameters {
    image: vec4<u32>, // width, height, samples per pixel, frame index
    limits: vec4<u32>, // path vertices, cache enabled, cache slots, minimum confidence samples
    grid: vec4<f32>, // smallest cell size, epsilon, total emitter area, ray distance
    camera_position: vec4<f32>,
    camera_right: vec4<f32>, // w = aspect
    camera_up: vec4<f32>,
    camera_forward: vec4<f32>, // w = tan(vertical FOV / 2)
    cache: vec4<u32>, // emitter count, maximum history weight, stale frames, seed
    restir: vec4<u32>, // fresh candidates, reuse bits (temporal=1/spatial=2), history M cap, scene epoch
};
struct Triangle { p: array<vec4<f32>, 3>, n: array<vec4<f32>, 3> };
struct Material { albedo: vec4<f32>, emission: vec4<f32> }; // emission.w = double sided
struct Emitter { triangle: u32, area: f32, area_cdf: f32, padding: u32 };
// Metadata is immutable after insertion until a later Resolve pass evicts the slot.
// Current-frame inserts never accept another thread's deposit, avoiding relaxed-atomic publication.
struct HashEntry {
    birth_frame: atomic<u32>,
    x: atomic<i32>, y: atomic<i32>, z: atomic<i32>,
    normal_horizon_level: atomic<u32>, material: atomic<u32>,
    last_frame: atomic<u32>, padding: atomic<u32>,
};
struct Accumulation { r: atomic<u32>, g: atomic<u32>, b: atomic<u32>, count: atomic<u32> };
struct Hit { valid: u32, t: f32, primitive: u32, padding: u32, position: vec3<f32>, normal: vec3<f32> };
struct Key { cell: vec3<i32>, packed: u32, material: u32, size: f32 };
struct Vertex {
    position: vec3<f32>, normal: vec3<f32>, albedo: vec3<f32>,
    emission: vec3<f32>, direct: vec3<f32>, material: u32, horizon: u32, arrival_weight: f32,
};

@group(0) @binding(0) var<uniform> params: Parameters;
@group(0) @binding(1) var scene: acceleration_structure;
@group(0) @binding(2) var<storage, read> triangles: array<Triangle>;
@group(0) @binding(3) var<storage, read> materials: array<Material>;
@group(0) @binding(4) var<storage, read> emitters: array<Emitter>;
@group(0) @binding(5) var<storage, read_write> image: array<vec4<f32>>;
@group(0) @binding(6) var<storage, read_write> hashes: array<HashEntry>;
@group(0) @binding(7) var<storage, read_write> accumulation: array<Accumulation>;
@group(0) @binding(8) var<storage, read_write> resolved: array<vec4<f32>>;
@group(0) @binding(9) var<storage, read_write> counters: array<atomic<u32>>;

// Real receiver / secondary-path reservoirs. These are separate from the final color buffer.
struct GiPrimary {
    position: vec4<f32>, normal: vec4<f32>, albedo: vec4<f32>,
    direct: vec4<f32>, identity: vec4<u32>,
};
@group(0) @binding(10) var<storage, read_write> primary_now: array<GiPrimary>;
@group(0) @binding(11) var<storage, read_write> primary_previous: array<GiPrimary>;
@group(0) @binding(12) var<storage, read_write> reservoirs_initial: array<GiReservoir>;
@group(0) @binding(13) var<storage, read_write> reservoirs_temporal: array<GiReservoir>;
@group(0) @binding(14) var<storage, read_write> reservoirs_final: array<GiReservoir>;
@group(0) @binding(15) var<storage, read_write> reservoirs_previous: array<GiReservoir>;

fn random(state: ptr<function, u32>) -> f32 {
    *state ^= *state << 13u;
    *state ^= *state >> 17u;
    *state ^= *state << 5u;
    return f32(*state >> 8u) * (1.0 / 16777216.0);
}
fn mix(value: u32) -> u32 {
    var x = value;
    x ^= x >> 16u; x *= 0x7feb352du;
    x ^= x >> 15u; x *= 0x846ca68bu;
    return x ^ (x >> 16u);
}
fn cosine_direction(normal: vec3<f32>, state: ptr<function, u32>) -> vec3<f32> {
    let u = random(state);
    let angle = 2.0 * PI * random(state);
    let tangent = normalize(cross(select(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(0.0, 1.0, 0.0), abs(normal.z) > 0.9), normal));
    let bitangent = cross(normal, tangent);
    return normalize(tangent * (sqrt(u) * cos(angle)) + bitangent * (sqrt(u) * sin(angle)) + normal * sqrt(1.0 - u));
}
fn camera_ray(pixel: vec2<u32>, state: ptr<function, u32>) -> vec3<f32> {
    let jitter = vec2<f32>(random(state), random(state));
    let uv = (vec2<f32>(pixel) + jitter) / vec2<f32>(params.image.xy);
    let ndc = vec2<f32>(2.0 * uv.x - 1.0, 1.0 - 2.0 * uv.y);
    return normalize(params.camera_forward.xyz + params.camera_forward.w *
        (params.camera_right.xyz * (ndc.x * params.camera_right.w) + params.camera_up.xyz * ndc.y));
}

fn trace(origin: vec3<f32>, direction: vec3<f32>, maximum: f32, shadow: bool, update: bool) -> Hit {
    var offset = 0.0;
    // Naga 30's MSL lowering cannot reset an intersection_query through assignment. Declare it
    // outside the loop and reinitialize it through rayQueryInitialize for each skipped back face.
    var query: ray_query;
    // A nonfinite ray or an empty interval misses instead of reaching rayQueryInitialize, as naga's
    // query tracking would make it; the lean shader variant (RayLighting::with_shaders) leaves
    // that check out.
    if !(all(abs(origin) < vec3<f32>(1e30)) && all(abs(direction) < vec3<f32>(1e30))
        && maximum < 1e30 && maximum >= params.grid.y) {
        return Hit(0u, -1.0, 0u, 0u, vec3<f32>(0.0), vec3<f32>(0.0));
    }
    // A material may be single-sided. Skip its back faces explicitly, rather than treating every
    // opaque triangle as two-sided. Shadow rays test all opaque blockers, including back faces.
    for (var skip = 0u; skip < 16u; skip++) {
        atomicAdd(&counters[select(5u, 1u, update) + select(0u, 1u, shadow)], 1u);
        rayQueryInitialize(&query, scene, RayDesc(RAY_FLAG_NONE, 255u, params.grid.y, maximum - offset,
            origin + direction * offset, direction));
        while rayQueryProceed(&query) {}
        let hit = rayQueryGetCommittedIntersection(&query);
        if hit.kind != RAY_QUERY_INTERSECTION_TRIANGLE {
            return Hit(0u, -1.0, 0u, 0u, vec3<f32>(0.0), vec3<f32>(0.0));
        }
        let triangle = triangles[hit.primitive_index];
        let material = materials[u32(triangle.n[0].w)];
        let weights = vec3<f32>(1.0 - hit.barycentrics.x - hit.barycentrics.y, hit.barycentrics);
        var normal = normalize(triangle.n[0].xyz * weights.x + triangle.n[1].xyz * weights.y + triangle.n[2].xyz * weights.z);
        let geometric = normalize(cross(triangle.p[1].xyz - triangle.p[0].xyz, triangle.p[2].xyz - triangle.p[0].xyz));
        if !shadow && dot(geometric, direction) > 0.0 && material.emission.w < 0.5 {
            offset += hit.t + params.grid.y;
            if offset >= maximum - params.grid.y { break; }
            continue;
        }
        if dot(normal, direction) > 0.0 { normal = -normal; }
        let distance = hit.t + offset;
        return Hit(1u, distance, hit.primitive_index, 0u, origin + direction * distance, normal);
    }
    return Hit(0u, -1.0, 0u, 0u, vec3<f32>(0.0), vec3<f32>(0.0));
}

fn direct_light(hit: Hit, albedo: vec3<f32>, state: ptr<function, u32>, update: bool, horizon: u32) -> vec3<f32> {
    if params.cache.x == 0u { return vec3<f32>(0.0); }
    let sample_area = random(state) * params.grid.z;
    var index = params.cache.x - 1u;
    for (var i = 0u; i < params.cache.x; i++) {
        if sample_area < emitters[i].area_cdf { index = i; break; }
    }
    let triangle = triangles[emitters[index].triangle];
    let material = materials[u32(triangle.n[0].w)];
    let root = sqrt(random(state));
    let second = random(state);
    let point = triangle.p[0].xyz * (1.0 - root) + triangle.p[1].xyz * (root * (1.0 - second)) + triangle.p[2].xyz * (root * second);
    let delta = point - hit.position;
    let distance_squared = dot(delta, delta);
    if distance_squared < 16.0 * params.grid.y * params.grid.y { return vec3<f32>(0.0); }
    let distance = sqrt(distance_squared);
    let direction = delta / distance;
    let light_normal = normalize(cross(triangle.p[1].xyz - triangle.p[0].xyz, triangle.p[2].xyz - triangle.p[0].xyz));
    let surface_cosine = max(0.0, dot(hit.normal, direction));
    let light_cosine = select(max(0.0, dot(light_normal, -direction)), abs(dot(light_normal, -direction)), material.emission.w > 0.5);
    if surface_cosine * light_cosine <= 0.0 { return vec3<f32>(0.0); }
    let blocker = trace(hit.position + hit.normal * params.grid.y * 2.0, direction, max(params.grid.y * 2.0, distance - params.grid.y * 4.0), true, update);
    if blocker.valid != 0u { return vec3<f32>(0.0); }
    // Area sampling alone has extreme variance near a large emitter. Balance it against the
    // cosine BSDF's solid-angle density. At the final vertex there is no BSDF continuation.
    let light_pdf = distance_squared / (light_cosine * params.grid.z);
    let bsdf_pdf = surface_cosine / PI;
    let mis = select(light_pdf * light_pdf / (light_pdf * light_pdf + bsdf_pdf * bsdf_pdf), 1.0, horizon == 1u);
    return albedo * material.emission.xyz * (surface_cosine * mis / (PI * light_pdf));
}

fn emission_weight(hit: Hit, origin: vec3<f32>, direction: vec3<f32>, previous_normal: vec3<f32>) -> f32 {
    if params.cache.x == 0u { return 1.0; }
    let triangle = triangles[hit.primitive];
    let material = materials[u32(triangle.n[0].w)];
    let normal = normalize(cross(triangle.p[1].xyz - triangle.p[0].xyz, triangle.p[2].xyz - triangle.p[0].xyz));
    let light_cosine = select(max(0.0, dot(normal, -direction)), abs(dot(normal, -direction)), material.emission.w > 0.5);
    if light_cosine <= 0.0 { return 0.0; }
    let light_pdf = dot(hit.position - origin, hit.position - origin) / (light_cosine * params.grid.z);
    let bsdf_pdf = max(0.0, dot(previous_normal, direction)) / PI;
    return bsdf_pdf * bsdf_pdf / max(1e-20, bsdf_pdf * bsdf_pdf + light_pdf * light_pdf);
}

fn cache_key(position: vec3<f32>, normal: vec3<f32>, material: u32, horizon: u32) -> Key {
    let distance = length(position - params.camera_position.xyz);
    let level = u32(clamp(floor(log2(max(1.0, distance / 4.0))), 0.0, 4.0));
    let size = params.grid.x * exp2(f32(level));
    var oct = normal.xy / (abs(normal.x) + abs(normal.y) + abs(normal.z));
    if normal.z < 0.0 {
        let signs = select(vec2<f32>(-1.0), vec2<f32>(1.0), oct >= vec2<f32>(0.0));
        oct = (vec2<f32>(1.0) - abs(oct.yx)) * signs;
    }
    let bins = vec2<u32>(clamp(floor((oct * 0.5 + 0.5) * 8.0), vec2<f32>(0.0), vec2<f32>(7.0)));
    let packed = bins.x | (bins.y << 3u) | (horizon << 6u) | (level << 12u);
    return Key(vec3<i32>(floor(position / size)), packed, material, size);
}
fn first_slot(key: Key) -> u32 {
    return mix(bitcast<u32>(key.cell.x) ^ mix(bitcast<u32>(key.cell.y)) ^
        mix(bitcast<u32>(key.cell.z) + key.packed) ^ mix(key.material)) & (params.limits.z - 1u);
}
fn matches(index: u32, key: Key) -> bool {
    return atomicLoad(&hashes[index].x) == key.cell.x && atomicLoad(&hashes[index].y) == key.cell.y &&
        atomicLoad(&hashes[index].z) == key.cell.z && atomicLoad(&hashes[index].normal_horizon_level) == key.packed &&
        atomicLoad(&hashes[index].material) == key.material;
}
fn insert_slot(key: Key) -> u32 {
    let first = first_slot(key);
    for (var probe = 0u; probe < 16u; probe++) {
        let index = (first + probe) & (params.limits.z - 1u);
        let birth = atomicLoad(&hashes[index].birth_frame);
        if birth == 0u {
            let claim = atomicCompareExchangeWeak(&hashes[index].birth_frame, 0u, params.image.w + 1u);
            if !claim.exchanged { return INVALID; }
            atomicStore(&hashes[index].x, key.cell.x);
            atomicStore(&hashes[index].y, key.cell.y);
            atomicStore(&hashes[index].z, key.cell.z);
            atomicStore(&hashes[index].normal_horizon_level, key.packed);
            atomicStore(&hashes[index].material, key.material);
            atomicStore(&hashes[index].last_frame, params.image.w + 1u);
            return index;
        }
        // Never rely on another workgroup publishing new metadata within this pass.
        if birth == params.image.w + 1u { return INVALID; }
        if matches(index, key) {
            atomicStore(&hashes[index].last_frame, params.image.w + 1u);
            return index;
        }
        atomicAdd(&counters[12], 1u);
    }
    return INVALID;
}
fn deposit(vertex: Vertex, radiance: vec3<f32>) {
    let key = cache_key(vertex.position, vertex.normal, vertex.material, vertex.horizon);
    let index = insert_slot(key);
    if index == INVALID { atomicAdd(&counters[11], 1u); return; }
    var accepted = false;
    // Bounded sample count makes 32-bit fixed-point sums provably safe from overflow.
    for (var attempt = 0u; attempt < 16u; attempt++) {
        let count = atomicLoad(&accumulation[index].count);
        if count >= MAX_CELL_SAMPLES { break; }
        let claim = atomicCompareExchangeWeak(&accumulation[index].count, count, count + 1u);
        if claim.exchanged { accepted = true; break; }
    }
    if !accepted { atomicAdd(&counters[11], 1u); return; }
    if any(radiance > vec3<f32>(MAX_CACHE_RADIANCE)) { atomicAdd(&counters[13], 1u); }
    let quantized = vec3<u32>(round(clamp(radiance, vec3<f32>(0.0), vec3<f32>(MAX_CACHE_RADIANCE)) * QUANTIZATION));
    atomicAdd(&accumulation[index].r, quantized.x);
    atomicAdd(&accumulation[index].g, quantized.y);
    atomicAdd(&accumulation[index].b, quantized.z);
    atomicAdd(&counters[10], 1u);
}
fn query_cache(key: Key, segment_length: f32) -> vec4<f32> {
    if segment_length < key.size { return vec4<f32>(0.0); }
    atomicAdd(&counters[8], 1u);
    let first = first_slot(key);
    for (var probe = 0u; probe < 16u; probe++) {
        let index = (first + probe) & (params.limits.z - 1u);
        let birth = atomicLoad(&hashes[index].birth_frame);
        if birth == 0u { return vec4<f32>(0.0); }
        if birth + 2u > params.image.w + 1u { continue; }
        if matches(index, key) && resolved[index].w >= f32(params.limits.w) {
            atomicAdd(&counters[9], 1u);
            return vec4<f32>(resolved[index].xyz, 1.0);
        }
    }
    return vec4<f32>(0.0);
}

fn path_with_hit(origin: vec3<f32>, direction: vec3<f32>, state: ptr<function, u32>, update: bool, horizon: u32, first_hit: Hit, known_hit: bool) -> vec3<f32> {
    var ray_origin = origin;
    var ray_direction = direction;
    var throughput = vec3<f32>(1.0);
    var radiance = vec3<f32>(0.0);
    var previous_position = origin;
    var previous_normal = vec3<f32>(0.0);
    var vertices: array<Vertex, 8>;
    var vertex_count = 0u;
    for (var bounce = 0u; bounce < horizon; bounce++) {
        var hit = first_hit;
        if bounce > 0u || !known_hit { hit = trace(ray_origin, ray_direction, params.grid.w, false, update); }
        if hit.valid == 0u { break; } // Explicit black environment.
        atomicAdd(&counters[select(7u, 3u, update)], 1u);
        let material_index = u32(triangles[hit.primitive].n[0].w);
        let material = materials[material_index];
        var arrival_weight = 1.0;
        if bounce > 0u { arrival_weight = emission_weight(hit, previous_position, ray_direction, previous_normal); }
        radiance += throughput * material.emission.xyz * arrival_weight;
        if !update && params.limits.y != 0u && bounce > 0u {
            let cached = query_cache(cache_key(hit.position, hit.normal, material_index, horizon - bounce), hit.t);
            if cached.w > 0.0 {
                // Own emission was just evaluated with the incoming ray's MIS weight. Cached
                // diffuse outgoing radiance includes unit-weight own emission, so remove it.
                radiance += throughput * max(vec3<f32>(0.0), cached.xyz - material.emission.xyz);
                break;
            }
        }
        let direct = direct_light(hit, material.albedo.xyz, state, update, horizon - bounce);
        radiance += throughput * direct;
        if update {
            vertices[vertex_count] = Vertex(hit.position, hit.normal, material.albedo.xyz,
                material.emission.xyz, direct, material_index, horizon - bounce, arrival_weight);
            vertex_count++;
        }
        throughput *= material.albedo.xyz;
        previous_position = hit.position;
        previous_normal = hit.normal;
        ray_origin = hit.position + hit.normal * params.grid.y * 2.0;
        ray_direction = cosine_direction(hit.normal, state);
    }
    if update {
        // Accumulate complete independently traced path suffixes, never this frame's own cache.
        var tail = vec3<f32>(0.0);
        var child_emission = vec3<f32>(0.0);
        var child_arrival_weight = 1.0;
        for (var reverse = vertex_count; reverse > 0u; reverse--) {
            let vertex = vertices[reverse - 1u];
            tail = vertex.emission + vertex.direct + vertex.albedo *
                max(vec3<f32>(0.0), tail - child_emission * (1.0 - child_arrival_weight));
            deposit(vertex, tail);
            child_emission = vertex.emission;
            child_arrival_weight = vertex.arrival_weight;
        }
    }
    return radiance;
}

fn path(origin: vec3<f32>, direction: vec3<f32>, state: ptr<function, u32>, update: bool) -> vec3<f32> {
    return path_with_hit(origin, direction, state, update, params.limits.x,
        Hit(0u, -1.0, 0u, 0u, vec3<f32>(0.0), vec3<f32>(0.0)), false);
}

@compute @workgroup_size(64)
fn update(@builtin(global_invocation_id) id: vec3<u32>) {
    let tiles = (params.image.xy + vec2<u32>(4u)) / 5u;
    if id.x >= tiles.x * tiles.y { return; }
    var state = mix(id.x ^ mix(params.image.w + params.cache.w + 0x7139a71bu)) | 1u;
    let tile = vec2<u32>(id.x % tiles.x, id.x / tiles.x);
    let offset = vec2<u32>(min(4u, u32(random(&state) * 5.0)), min(4u, u32(random(&state) * 5.0)));
    let pixel = min(tile * 5u + offset, params.image.xy - vec2<u32>(1u));
    for (var sample = 0u; sample < params.image.z; sample++) {
        atomicAdd(&counters[0], 1u);
        path(params.camera_position.xyz, camera_ray(pixel, &state), &state, true);
    }
}

@compute @workgroup_size(64)
fn resolve(@builtin(global_invocation_id) id: vec3<u32>) {
    let index = id.x;
    if index >= params.limits.z { return; }
    let birth = atomicLoad(&hashes[index].birth_frame);
    if birth == 0u { return; }
    let count = atomicLoad(&accumulation[index].count);
    if count > 0u {
        let sum = vec3<f32>(f32(atomicLoad(&accumulation[index].r)), f32(atomicLoad(&accumulation[index].g)), f32(atomicLoad(&accumulation[index].b))) / QUANTIZATION;
        let previous = resolved[index];
        let history_weight = min(previous.w, f32(params.cache.y));
        let weight = history_weight + f32(count);
        resolved[index] = vec4<f32>((previous.xyz * history_weight + sum) / weight, min(weight, f32(params.cache.y)));
    } else if params.image.w + 1u - atomicLoad(&hashes[index].last_frame) > params.cache.z {
        atomicStore(&hashes[index].birth_frame, 0u);
        resolved[index] = vec4<f32>(0.0);
        return;
    }
    atomicStore(&accumulation[index].r, 0u);
    atomicStore(&accumulation[index].g, 0u);
    atomicStore(&accumulation[index].b, 0u);
    atomicStore(&accumulation[index].count, 0u);
    atomicAdd(&counters[14], 1u);
    if resolved[index].w >= f32(params.limits.w) && birth + 2u <= params.image.w + 1u {
        atomicAdd(&counters[15], 1u);
    }
}

@compute @workgroup_size(64)
fn render(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= params.image.x * params.image.y { return; }
    var state = mix(id.x ^ mix(params.image.w + params.cache.w)) | 1u;
    let pixel = vec2<u32>(id.x % params.image.x, id.x / params.image.x);
    var color = vec3<f32>(0.0);
    for (var sample = 0u; sample < params.image.z; sample++) {
        atomicAdd(&counters[4], 1u);
        color += path(params.camera_position.xyz, camera_ray(pixel, &state), &state, false);
    }
    image[id.x] += vec4<f32>(color / f32(params.image.z), 1.0);
}

fn restir_zero_proposal(reservoir: ptr<function, GiReservoir>, represented: u32) {
    // A traced miss has known zero contribution. It is still one of the K directional proposals.
    // Dropping it from M would turn RIS into sampling conditioned on finding a surface.
    (*reservoir).M += represented;
}

fn restir_merge(reservoir: ptr<function, GiReservoir>, source: GiReservoir,
    receiver: GiPrimary, state: ptr<function, u32>) -> bool {
    var admitted = source;
    gi_reservoir_limit_history(&admitted, params.restir.z);
    if admitted.M == 0u { return false; }
    if admitted.valid == 0u && admitted.sum_weights == 0.0 {
        restir_zero_proposal(reservoir, admitted.M);
        return false;
    }
    let contribution = gi_candidate_evaluate(receiver.position.xyz, receiver.normal.xyz,
        receiver.albedo.xyz, admitted.candidate);
    return gi_reservoir_merge(reservoir, admitted, gi_candidate_target(contribution), random(state));
}

fn restir_compatible(current: GiPrimary, previous: GiPrimary, temporal: bool) -> bool {
    let delta = current.position.xyz - previous.position.xyz;
    let tolerance = select(params.grid.x * 4.0, params.grid.x, temporal);
    return current.normal.w == 1.0 && previous.normal.w == 1.0
        && current.identity.x == previous.identity.x && current.identity.y == previous.identity.y
        && dot(delta, delta) <= tolerance * tolerance
        && gi_reuse_allowed(current.identity.w, previous.identity.w, current.identity.z, previous.identity.z,
            temporal, true, true, current.normal.xyz, previous.normal.xyz, 0.95);
}

@compute @workgroup_size(64)
fn restir_initial(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= params.image.x * params.image.y { return; }
    var state = mix(id.x ^ mix(params.image.w + params.cache.w)) | 1u;
    let pixel = vec2<u32>(id.x % params.image.x, id.x / params.image.x);
    atomicAdd(&counters[4], 1u);
    let direction = camera_ray(pixel, &state);
    let primary = trace(params.camera_position.xyz, direction, params.grid.w, false, false);
    var reservoir = gi_reservoir_empty();
    var receiver = GiPrimary(vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0),
        vec4<u32>(INVALID, INVALID, params.image.w, params.restir.w));
    if primary.valid == 0u {
        primary_now[id.x] = receiver;
        reservoirs_initial[id.x] = reservoir;
        return;
    }
    atomicAdd(&counters[7], 1u);
    let material_index = u32(triangles[primary.primitive].n[0].w);
    let material = materials[material_index];
    // Direct emission + the original primary area-light MIS strategy.
    var direct = material.emission.xyz + direct_light(primary, material.albedo.xyz, &state, false, params.limits.x);
    var bsdf_direct = vec3<f32>(0.0);
    for (var candidate_index = 0u; candidate_index < params.restir.x; candidate_index++) {
        atomicAdd(&counters[16], 1u);
        if params.limits.x == 1u {
            // The baseline final vertex uses full-weight NEE with no BSDF continuation.
            restir_zero_proposal(&reservoir, 1u);
            atomicAdd(&counters[28], 1u);
            continue;
        }
        let continuation = cosine_direction(primary.normal, &state);
        let origin = primary.position + primary.normal * params.grid.y * 2.0;
        let secondary = trace(origin, continuation, params.grid.w, false, false);
        if secondary.valid == 0u {
            restir_zero_proposal(&reservoir, 1u);
            atomicAdd(&counters[28], 1u);
            continue;
        }
        atomicAdd(&counters[17], 1u);
        let second_material_index = u32(triangles[secondary.primitive].n[0].w);
        let second_material = materials[second_material_index];
        // Complete the primary direct-light MIS pair outside the indirect reservoir.
        let arrival_weight = emission_weight(secondary, primary.position, continuation, primary.normal);
        bsdf_direct += material.albedo.xyz * second_material.emission.xyz * arrival_weight;
        // The tail begins at the already traced secondary surface and has one less vertex than
        // the raw camera path. For Lambertian transport its reflected Lo is view independent.
        let outgoing = path_with_hit(origin, continuation, &state, false, params.limits.x - 1u, secondary, true);
        let reflected = max(vec3<f32>(0.0), outgoing - second_material.emission.xyz);
        let candidate = GiCandidate(vec4<f32>(secondary.position, 1.0), vec4<f32>(secondary.normal, 0.0),
            vec4<f32>(reflected, 0.0), vec4<u32>(secondary.primitive, second_material_index, 1u, 0u));
        let delta = secondary.position - primary.position;
        let pdf_omega = max(0.0, dot(primary.normal, normalize(delta))) / PI;
        let pdf_area = gi_candidate_area_pdf(primary.position, candidate, pdf_omega);
        if pdf_area <= 0.0 {
            restir_zero_proposal(&reservoir, 1u);
            atomicAdd(&counters[29], 1u);
            continue;
        }
        let value = gi_candidate_evaluate(primary.position, primary.normal, material.albedo.xyz, candidate);
        let selected = gi_reservoir_update(&reservoir, candidate, gi_candidate_target(value), pdf_area, random(&state));
        if selected { atomicAdd(&counters[24], 1u); }
        if gi_candidate_target(value) == 0.0 { atomicAdd(&counters[28], 1u); }
    }
    direct += bsdf_direct / f32(params.restir.x);
    if reservoir.M != params.restir.x { atomicAdd(&counters[31], 1u); }
    receiver = GiPrimary(vec4<f32>(primary.position, primary.t), vec4<f32>(primary.normal, 1.0),
        vec4<f32>(material.albedo.xyz, 0.0), vec4<f32>(direct, 0.0),
        vec4<u32>(primary.primitive, material_index, params.image.w, params.restir.w));
    primary_now[id.x] = receiver;
    reservoirs_initial[id.x] = reservoir;
}

@compute @workgroup_size(64)
fn restir_temporal(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= params.image.x * params.image.y { return; }
    var result = reservoirs_initial[id.x];
    let receiver = primary_now[id.x];
    if (params.restir.y & 1u) != 0u && receiver.normal.w == 1.0 {
        let previous = primary_previous[id.x];
        if restir_compatible(receiver, previous, true) {
            atomicAdd(&counters[18], 1u);
            var state = mix(id.x ^ mix(params.image.w + 0x659dae11u)) | 1u;
            if restir_merge(&result, reservoirs_previous[id.x], receiver, &state) {
                atomicAdd(&counters[25], 1u);
            }
        } else { atomicAdd(&counters[23], 1u); }
    }
    gi_reservoir_limit_history(&result, params.restir.z);
    reservoirs_temporal[id.x] = result;
}

@compute @workgroup_size(64)
fn restir_spatial(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= params.image.x * params.image.y { return; }
    var result = reservoirs_temporal[id.x];
    let receiver = primary_now[id.x];
    if (params.restir.y & 2u) != 0u && receiver.normal.w == 1.0 {
        var state = mix(id.x ^ mix(params.image.w + 0xbd31a841u)) | 1u;
        let pixel = vec2<i32>(i32(id.x % params.image.x), i32(id.x / params.image.x));
        // Four same-frame sources from a separate buffer; no read/write feedback in this pass.
        let offsets = array<vec2<i32>, 4>(vec2<i32>(-1, 0), vec2<i32>(1, 0), vec2<i32>(0, -1), vec2<i32>(0, 1));
        for (var neighbor = 0u; neighbor < 4u; neighbor++) {
            let point = pixel + offsets[neighbor];
            if any(point < vec2<i32>(0)) || any(point >= vec2<i32>(params.image.xy)) { continue; }
            let index = u32(point.y) * params.image.x + u32(point.x);
            if restir_compatible(receiver, primary_now[index], false) {
                atomicAdd(&counters[19], 1u);
                if restir_merge(&result, reservoirs_temporal[index], receiver, &state) {
                    atomicAdd(&counters[26], 1u);
                }
            }
        }
    }
    gi_reservoir_limit_history(&result, params.restir.z);
    reservoirs_final[id.x] = result;
}

fn restir_visibility(receiver: GiPrimary, candidate: GiCandidate) -> f32 {
    let origin = receiver.position.xyz + receiver.normal.xyz * params.grid.y * 2.0;
    let delta = candidate.second_position.xyz - origin;
    let distance = length(delta);
    if distance <= params.grid.y * 4.0 { return 0.0; }
    atomicAdd(&counters[20], 1u);
    let hit = trace(origin, delta / distance, distance + params.grid.y * 4.0, true, false);
    let tolerance = max(params.grid.y * 8.0, distance * 1e-4);
    if hit.valid == 1u && hit.primitive == candidate.identity.x && abs(hit.t - distance) <= tolerance {
        atomicAdd(&counters[21], 1u);
        return 1.0;
    }
    atomicAdd(&counters[22], 1u);
    return 0.0;
}

@compute @workgroup_size(64)
fn restir_shade(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= params.image.x * params.image.y { return; }
    let receiver = primary_now[id.x];
    let reservoir = reservoirs_final[id.x];
    atomicMax(&counters[30], reservoir.M);
    var color = receiver.direct.xyz;
    if receiver.normal.w == 1.0 && reservoir.valid == 1u {
        atomicAdd(&counters[27], 1u);
        let visibility = restir_visibility(receiver, reservoir.candidate);
        color += gi_reservoir_estimate(reservoir, receiver.position.xyz, receiver.normal.xyz, receiver.albedo.xyz, visibility);
    }
    image[id.x] += vec4<f32>(color, 1.0);
}
