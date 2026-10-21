// Presentation-only foam, transported by the actual water velocity and decaying in simulated time.
struct Params { count: vec4<u32>, cell: f32, dt: f32, clear: u32, padding: u32 }
@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var<storage, read> water: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> old_foam: array<f32>;
@group(0) @binding(3) var<storage, read_write> next_foam: array<f32>;
@group(0) @binding(4) var foam_field: texture_storage_2d<r32float, write>;
@group(0) @binding(5) var<storage, read> impacts:array<u32>;
fn column(pos: vec2<i32>) -> vec4<f32> {
    let c = vec2<u32>(clamp(pos, vec2<i32>(0), vec2<i32>(p.count.xy) - vec2<i32>(1)));
    if p.count.z != 0u {
        return water[((c.x / 16u) + (p.count.x / 16u) * (c.y / 16u)) * 256u +
            (c.x % 16u) + 16u * (c.y % 16u)];
    }
    return water[c.x + p.count.x * c.y];
}
fn previous(pos: vec2<f32>) -> f32 {
    if p.clear != 0u || any(pos < vec2<f32>(0.0)) || any(pos > vec2<f32>(p.count.xy - vec2<u32>(1u))) {
        return 0.0;
    }
    let a = vec2<u32>(floor(pos));
    let b = min(a + vec2<u32>(1u), p.count.xy - vec2<u32>(1u));
    let t = fract(pos);
    return mix(mix(old_foam[a.x + p.count.x * a.y], old_foam[b.x + p.count.x * a.y], t.x),
        mix(old_foam[a.x + p.count.x * b.y], old_foam[b.x + p.count.x * b.y], t.x), t.y);
}
fn neighbour(pos:vec2i,fallback:vec4f)->vec4f {
    let c=column(pos);return select(fallback,c,c.y>=0.0&&abs(c.x)<1e10);
}
fn surface(c: vec4<f32>, fallback: f32) -> f32 {
    // Unreceived tiles have negative depth (older sentinels used a huge height).
    return select(fallback, c.x, c.y >= 0.0 && abs(c.x) < 1e10);
}
@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id.xy >= p.count.xy) { return; }
    let n = id.x + p.count.x * id.y;
    let at = vec2<i32>(id.xy);
    let c = column(at);
    if c.y < 0.002 { next_foam[n] = 0.0; textureStore(foam_field, at, vec4<f32>(0.0)); return; }
    let l = neighbour(at - vec2<i32>(1, 0),c); let r = neighbour(at + vec2<i32>(1, 0),c);
    let d = neighbour(at - vec2<i32>(0, 1),c); let u = neighbour(at + vec2<i32>(0, 1),c);
    let divergence = (r.z - l.z + u.w - d.w) / (2.0 * p.cell);
    let slope = length(vec2<f32>(surface(r, c.x) - surface(l, c.x),
        surface(u, c.x) - surface(d, c.x))) / (2.0 * p.cell);
    let speed = length(c.zw);
    let froude = speed / sqrt(9.81 * max(c.y, 0.05));
    let compress = smoothstep(0.25, 2.0, -divergence);
    let rapids = smoothstep(0.7, 1.8, froude) * smoothstep(0.5, 2.5, speed);
    let depth_gradient = vec2<f32>(r.y - l.y, u.y - d.y) / (2.0 * p.cell);
    let shore_push = max(-dot(c.zw, depth_gradient), 0.0);
    let shoreline = (1.0 - smoothstep(0.08, 0.7, c.y)) * smoothstep(0.8, 3.0, speed)
        * smoothstep(0.05, 0.5, shore_push);
    let fall = smoothstep(0.3, 1.5, slope) * smoothstep(1.0, 3.0, speed);
    let breaking = rapids * max(compress, smoothstep(0.1, 0.6, slope));
    let source = max(max(compress * 0.6, breaking * 0.5), max(shoreline * 0.5, fall));
    let carried = previous(vec2<f32>(id.xy) - c.zw * p.dt / p.cell);
    // Exact bounded integration of decay and a saturating source; no foam on a motionless shore.
    let remaining = carried * exp(-0.35 * p.dt);
    var density = clamp(1.0 - (1.0 - remaining) * exp(-3.0 * source * p.dt), 0.0, 1.0);
    if(impacts[n]!=0u){density=1.0-(1.0-density)*exp(-0.12*min(f32(impacts[n]),64.0));}
    next_foam[n] = density;
    textureStore(foam_field, at, vec4<f32>(density,0.0,0.0,0.0));
}
