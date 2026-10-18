// The box's liquid as a surface (fluid.rs; screen-space fluid rendering after van der Laan et al.
// 2009): the particles' nearest depth and their summed thickness are drawn into textures of their
// own, the depth is smoothed by a bilateral filter that keeps edges, and a full-screen pass in the
// transparent layer rebuilds the surface's position and normal from it and shades it: the liquid's
// colour absorbed through its thickness over what lies behind, sky reflected by Fresnel, the sun's
// highlight; the frame's fog over it. Depths are distances along the view direction, metres; 0 is
// no liquid. Each particle takes its region's place, cell size and absorption (fluid_mpm.wgsl).

struct Particle {
    x: vec3f,
    age: f32,
    v: vec3f,
    tag: u32,
    c: mat3x3f,
};

struct Region {
    origin: vec4f,        // the region's corner in the world; w: metres per cell (0: closed)
    absorb: vec4f,        // the liquid's absorption per metre (rgb)
    generation: vec4u,    // x: the generation open in this slot
};

struct Look {
    size: vec4f,          // x: a sphere's radius in cells; y: lifetime (s, 0: for ever);
                          // z: the largest radius in metres (the filter's reach)
    blur: vec4f,          // xy: the bilateral filter's step direction, (1, 0) or (0, 1)
};

const DEAD: u32 = 0xffffffffu;

@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var<storage, read> particles: array<Particle>;
@group(0) @binding(2) var<uniform> look: Look;
@group(0) @binding(3) var<uniform> regions: array<Region, 4>;

struct Sphere {
    @builtin(position) clip: vec4f,
    @location(0) uv: vec2f,
    @location(1) center: vec3f,
    @location(2) radius: f32,
    @location(3) absorb: vec3f,
};

fn sphere_vertex(vi: u32, ii: u32, scale: f32) -> Sphere {
    var o: Sphere;
    let p = particles[ii];
    let slot = p.tag & 7u;
    let r = regions[min(slot, 3u)];
    if (p.tag == DEAD || r.origin.w <= 0.0 || r.generation.x != (p.tag >> 3u)) {
        // Outside the clip volume: nothing drawn.
        o.clip = vec4f(0.0, 0.0, 2.0, 1.0);
        return o;
    }
    let world = r.origin.xyz + p.x * r.origin.w;
    let corners = array<vec2f, 6>(vec2f(-1.0, -1.0), vec2f(1.0, -1.0), vec2f(1.0, 1.0),
                                  vec2f(-1.0, -1.0), vec2f(1.0, 1.0), vec2f(-1.0, 1.0));
    let c = corners[vi];
    let center = (view.view * vec4f(world, 1.0)).xyz;
    // In its last half second a particle shrinks away.
    var fade = 1.0;
    if (look.size.y > 0.0) {
        fade = clamp((look.size.y - p.age) * 2.0, 0.0, 1.0);
    }
    let radius = look.size.x * r.origin.w * fade;
    let rs = radius * scale;
    o.center = center;
    o.uv = c;
    o.radius = radius;
    o.absorb = r.absorb.rgb;
    o.clip = view.proj * vec4f(center + vec3f(c * rs, rs), 1.0);
    return o;
}

// --- The nearest depth --------------------------------------------------------------------------

@vertex
fn depth_vs(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> Sphere {
    return sphere_vertex(vi, ii, 1.0);
}

struct DepthOut {
    @location(0) depth: f32,
    @builtin(frag_depth) z: f32,
};

@fragment
fn depth_fs(in: Sphere) -> DepthOut {
    let d = dot(in.uv, in.uv);
    if (d > 1.0) {
        discard;
    }
    let p = in.center + vec3f(in.uv, sqrt(1.0 - d)) * in.radius;
    let clip = view.proj * vec4f(p, 1.0);
    var o: DepthOut;
    o.depth = -p.z;
    o.z = clip.z / clip.w;
    return o;
}

// --- The thickness -------------------------------------------------------------------------------

@vertex
fn thick_vs(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> Sphere {
    return sphere_vertex(vi, ii, 1.5);
}

// rgb: optical depth (absorption times length), a: length.
@fragment
fn thick_fs(in: Sphere) -> @location(0) vec4f {
    let d = dot(in.uv, in.uv);
    if (d > 1.0) {
        discard;
    }
    // The chord through a sphere of the radius, softened at the rim.
    let chord = 2.0 * in.radius * sqrt(1.0 - d) * 0.5;
    return vec4f(in.absorb * chord, chord);
}

// --- Full-screen passes -------------------------------------------------------------------------

struct Screen {
    @builtin(position) clip: vec4f,
};

@vertex
fn screen_vs(@builtin(vertex_index) vi: u32) -> Screen {
    let p = vec2f(f32((vi << 1u) & 2u), f32(vi & 2u)) * 2.0 - 1.0;
    var o: Screen;
    o.clip = vec4f(p, 0.0, 1.0);
    return o;
}

@group(1) @binding(0) var depth_in: texture_2d<f32>;
@group(1) @binding(1) var thick_in: texture_2d<f32>;

// A bilateral filter along `look.blur.xy`: neighbours weighted by distance and by how close their
// depth is, so the surface smooths without melting into what lies behind it.
@fragment
fn blur_fs(in: Screen) -> @location(0) f32 {
    let px = vec2i(in.clip.xy);
    let size = vec2i(textureDimensions(depth_in));
    let z0 = textureLoad(depth_in, px, 0).r;
    if (z0 <= 0.0) {
        return 0.0;
    }
    let dir = vec2i(look.blur.xy);
    // The filter's reach: about four particle radii on the screen.
    let pixels = look.size.z * 4.0 * view.proj[1][1] * view.viewport.y * 0.5 / z0;
    let radius = clamp(i32(pixels), 3, 24);
    let sigma = f32(radius) * 0.5;
    let range = look.size.z * 4.0;
    var sum = 0.0;
    var weights = 0.0;
    for (var i = -radius; i <= radius; i++) {
        let q = clamp(px + dir * i, vec2i(0), size - 1);
        let z = textureLoad(depth_in, q, 0).r;
        if (z <= 0.0) {
            continue;
        }
        let r = f32(i) / sigma;
        let dz = (z - z0) / range;
        let w = exp(-0.5 * r * r) * exp(-dz * dz);
        sum += z * w;
        weights += w;
    }
    return sum / max(weights, 1e-6);
}

// The view-space point under pixel `px` at distance `z`.
fn at(px: vec2f, z: f32) -> vec3f {
    let ndc = vec2f(px.x * view.viewport.z * 2.0 - 1.0, 1.0 - px.y * view.viewport.w * 2.0);
    return vec3f(ndc.x * z / view.proj[0][0], ndc.y * z / view.proj[1][1], -z);
}

struct SurfaceOut {
    @location(0) color: vec4f,
    @builtin(frag_depth) z: f32,
};

@fragment
fn surface_fs(in: Screen) -> SurfaceOut {
    let px = vec2i(in.clip.xy);
    let size = vec2i(textureDimensions(depth_in));
    let z = textureLoad(depth_in, px, 0).r;
    if (z <= 0.0) {
        discard;
    }
    let p = at(in.clip.xy, z);
    // The normal from the nearer of each pair of neighbours (edges do not bleed).
    let zr = textureLoad(depth_in, min(px + vec2i(1, 0), size - 1), 0).r;
    let zl = textureLoad(depth_in, max(px - vec2i(1, 0), vec2i(0)), 0).r;
    let zu = textureLoad(depth_in, max(px - vec2i(0, 1), vec2i(0)), 0).r;
    let zd = textureLoad(depth_in, min(px + vec2i(0, 1), size - 1), 0).r;
    let pr = at(in.clip.xy + vec2f(1.0, 0.0), select(z, zr, zr > 0.0));
    let pl = at(in.clip.xy - vec2f(1.0, 0.0), select(z, zl, zl > 0.0));
    let pu = at(in.clip.xy - vec2f(0.0, 1.0), select(z, zu, zu > 0.0));
    let pd = at(in.clip.xy + vec2f(0.0, 1.0), select(z, zd, zd > 0.0));
    let ddx = select(p - pl, pr - p, abs(pr.z - p.z) < abs(p.z - pl.z));
    let ddy = select(pu - p, p - pd, abs(pd.z - p.z) < abs(p.z - pu.z));
    let n_view = normalize(cross(ddx, ddy));
    let rot = mat3x3f(view.view[0].xyz, view.view[1].xyz, view.view[2].xyz);
    var n = normalize(transpose(rot) * n_view);
    let to_eye = normalize(transpose(rot) * normalize(-p));
    if (dot(n, to_eye) < 0.0) {
        n = -n;
    }
    let thick = textureLoad(thick_in, px, 0);
    // What lies behind shows through the liquid, absorbed along its thickness.
    let transmit = exp(-thick.rgb * 1.5);
    let coverage = 1.0 - (transmit.r + transmit.g + transmit.b) / 3.0;
    let sun = view.sun_dir.w * view.sun_color.rgb / PI;
    // The light it scatters back, tinted by its absorption over a short path.
    let absorb = thick.rgb / max(thick.a, 1e-4);
    let tint = 0.45 * exp(-0.6 * absorb);
    let body = tint * (max(dot(n, view.sun_dir.xyz), 0.0) * 0.6 + 0.4) * sun;
    let cos_v = clamp(dot(n, to_eye), 0.0, 1.0);
    let fresnel = 0.02 + 0.98 * pow(1.0 - cos_v, 5.0);
    let r = reflect(-to_eye, n);
    let sky = mix(view.fog.rgb, view.sky_color.rgb, clamp(r.y, 0.0, 1.0)) * sun * 0.5;
    let h = normalize(view.sun_dir.xyz + to_eye);
    let spec = pow(max(dot(n, h), 0.0), 400.0) * sun * 3.0;
    // Spray: where the liquid is thin (droplets, sheets), air in it scatters white.
    let foam = (1.0 - smoothstep(0.02, 0.25, thick.a)) * 0.75;
    let a = clamp(max(max(coverage, fresnel), foam * 0.85), 0.0, 1.0);
    var o: SurfaceOut;
    // Premultiplied: the absorbing body over what lies behind, the reflection and highlight added,
    // then the fog (as forward.wgsl's: even under a liquid, else thinning with height).
    let world = view.camera_pos.xyz + transpose(rot) * p;
    let white = vec3f(0.8, 0.86, 0.9) * (max(dot(n, view.sun_dir.xyz), 0.0) * 0.5 + 0.5) * sun;
    let water = (body * coverage * (1.0 - fresnel) + sky * fresnel + spec) / max(a, 1e-4);
    let lit = mix(water, white, foam);
    let fogged = mix(lit, view.fog.rgb * view.sun_color.w, fog_amount(world));
    o.color = vec4f(fogged * a, a);
    let clip = view.proj * vec4f(p, 1.0);
    o.z = clip.z / clip.w;
    return o;
}

// How much of the colour at `world` the fog replaces (forward.wgsl's `fog_amount`).
fn fog_amount(world: vec3f) -> f32 {
    let density = view.fog.w;
    let d = distance(world, view.camera_pos.xyz);
    if (density < 0.0) {
        return clamp(1.0 - exp(density * d), 0.0, 1.0);
    }
    if (density == 0.0) {
        return 0.0;
    }
    let h = max(world.y, 0.0);
    return clamp(1.0 - exp(-density * d * exp(-h * 0.02)), 0.0, 1.0);
}
