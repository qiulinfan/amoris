// GPU particles: a ring-buffer pool; a spawn pass initializes the particles the CPU allotted this
// frame (per emitter), a simulate pass ages and moves every particle, and the draw pass expands
// each live one into a camera-facing quad blended into the HDR image (premultiplied alpha: alpha 0
// with bright colour adds light), depth-tested against the scene.

struct Particle {
    pos: vec3f,
    age: f32,
    vel: vec3f,
    life: f32,
    emitter: u32,
    seed: u32,
    _p0: u32,
    _p1: u32,
};

struct Emitter {
    pos: vec4f,           // xyz; w: radius
    up: vec4f,            // xyz; w: cos(spread)
    accel: vec4f,         // xyz; w: drag
    size: vec4f,          // start, end, speed, lifetime
    color_start: vec4f,
    color_end: vec4f,
};

struct Spawn {
    first: u32,           // first pool slot
    count: u32,
    emitter: u32,
    seed: u32,
};

struct Sim {
    dt: f32,
    pool: u32,
    spawns: u32,
    _p: u32,
};

@group(0) @binding(0) var<uniform> sim: Sim;
@group(0) @binding(1) var<storage, read_write> particles: array<Particle>;
@group(0) @binding(2) var<storage, read> emitters: array<Emitter>;
@group(0) @binding(3) var<storage, read> spawns: array<Spawn>;

fn hash(x: u32) -> u32 {
    var h = x * 747796405u + 2891336453u;
    h = ((h >> ((h >> 28u) + 4u)) ^ h) * 277803737u;
    return (h >> 22u) ^ h;
}

fn rand(s: ptr<function, u32>) -> f32 {
    *s = hash(*s);
    return f32(*s) / 4294967295.0;
}

@compute @workgroup_size(64)
fn spawn(@builtin(global_invocation_id) id: vec3u) {
    // One thread per new particle across every spawn record (records are few: a linear scan).
    var k = id.x;
    var rec = 0u;
    loop {
        if (rec >= sim.spawns) {
            return;
        }
        if (k < spawns[rec].count) {
            break;
        }
        k -= spawns[rec].count;
        rec++;
    }
    let sp = spawns[rec];
    let e = emitters[sp.emitter];
    var s = sp.seed ^ (k * 2654435761u);
    // A direction within the cone around `up`.
    let up = normalize(e.up.xyz);
    let helper = select(vec3f(0.0, 1.0, 0.0), vec3f(1.0, 0.0, 0.0), abs(up.y) > 0.9);
    let t1 = normalize(cross(up, helper));
    let t2 = cross(up, t1);
    let ct = mix(1.0, e.up.w, rand(&s));
    let st = sqrt(max(0.0, 1.0 - ct * ct));
    let phi = rand(&s) * 6.2831853;
    let dir = up * ct + (t1 * cos(phi) + t2 * sin(phi)) * st;
    let offset = (vec3f(rand(&s), rand(&s), rand(&s)) * 2.0 - 1.0) * e.pos.w;
    var p: Particle;
    p.pos = e.pos.xyz + offset;
    p.vel = dir * e.size.z * mix(0.7, 1.3, rand(&s));
    p.age = 0.0;
    p.life = e.size.w * mix(0.75, 1.25, rand(&s));
    p.emitter = sp.emitter;
    p.seed = s;
    particles[(sp.first + k) % sim.pool] = p;
}

@compute @workgroup_size(64)
fn simulate(@builtin(global_invocation_id) id: vec3u) {
    let i = id.x;
    if (i >= sim.pool) {
        return;
    }
    var p = particles[i];
    if (p.age >= p.life) {
        return;
    }
    let e = emitters[p.emitter];
    p.vel += e.accel.xyz * sim.dt;
    p.vel *= max(0.0, 1.0 - e.accel.w * sim.dt);
    p.pos += p.vel * sim.dt;
    p.age += sim.dt;
    particles[i] = p;
}

// --- Drawing -------------------------------------------------------------------------------------

@group(0) @binding(4) var<uniform> view: View;
@group(0) @binding(5) var<storage, read> live: array<Particle>;
@group(0) @binding(6) var<storage, read> looks: array<Emitter>;

struct Out {
    @builtin(position) clip: vec4f,
    @location(0) uv: vec2f,
    @location(1) color: vec4f,
};

@vertex
fn vs(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> Out {
    var o: Out;
    let p = live[ii];
    if (p.age >= p.life || p.life <= 0.0) {
        o.clip = vec4f(2.0, 2.0, 2.0, 1.0);
        return o;
    }
    let e = looks[p.emitter];
    let t = clamp(p.age / p.life, 0.0, 1.0);
    let size = mix(e.size.x, e.size.y, t);
    let corners = array<vec2f, 6>(vec2f(-1.0, -1.0), vec2f(1.0, -1.0), vec2f(1.0, 1.0),
                                  vec2f(-1.0, -1.0), vec2f(1.0, 1.0), vec2f(-1.0, 1.0));
    let c = corners[vi];
    let right = vec3f(view.view[0].x, view.view[1].x, view.view[2].x);
    let upv = vec3f(view.view[0].y, view.view[1].y, view.view[2].y);
    let world = p.pos + (right * c.x + upv * c.y) * size * 0.5;
    o.clip = view.view_proj * vec4f(world, 1.0);
    o.uv = c;
    o.color = mix(e.color_start, e.color_end, t);
    return o;
}

@fragment
fn fs(in: Out) -> @location(0) vec4f {
    let d = dot(in.uv, in.uv);
    if (d > 1.0) {
        discard;
    }
    let falloff = (1.0 - d) * (1.0 - d);
    let a = in.color.a * falloff;
    // Premultiplied: the colour scaled by the falloff; alpha only occludes when the emitter asks.
    return vec4f(in.color.rgb * falloff * max(in.color.a, 0.25), a);
}
