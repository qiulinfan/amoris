// The box's liquid as a surface (fluid.rs; screen-space fluid rendering after van der Laan et al.
// 2009): the particles' nearest depth and their summed thickness are drawn into textures of their
// own, the depth is smoothed by a bilateral filter that keeps edges, and a full-screen pass in the
// transparent layer rebuilds the surface's position and normal from it and shades it: the liquid's
// colour absorbed through its thickness over what lies behind, sky reflected by Fresnel, the sun's
// highlight. Depths are distances along the view direction, metres; 0 is no liquid.

struct Particle {
    x: vec3f,
    _a: f32,
    v: vec3f,
    _b: f32,
    c: mat3x3f,
};

struct Look {
    origin: vec4f,        // the grid's corner in the world; w: metres per cell
    color: vec4f,         // absorption per metre (rgb); w: the spheres' radius, metres
    blur: vec4f,          // x: the bilateral filter's step direction (1, 0) or (0, 1) in y too
};

@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var<storage, read> particles: array<Particle>;
@group(0) @binding(2) var<uniform> look: Look;

struct Sphere {
    @builtin(position) clip: vec4f,
    @location(0) uv: vec2f,
    @location(1) center: vec3f,
};

fn sphere_vertex(vi: u32, ii: u32, scale: f32) -> Sphere {
    var o: Sphere;
    let p = particles[ii];
    let world = look.origin.xyz + p.x * look.origin.w;
    let corners = array<vec2f, 6>(vec2f(-1.0, -1.0), vec2f(1.0, -1.0), vec2f(1.0, 1.0),
                                  vec2f(-1.0, -1.0), vec2f(1.0, 1.0), vec2f(-1.0, 1.0));
    let c = corners[vi];
    let center = (view.view * vec4f(world, 1.0)).xyz;
    let r = look.color.w * scale;
    o.center = center;
    o.uv = c;
    o.clip = view.proj * vec4f(center + vec3f(c * r, r), 1.0);
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
    let p = in.center + vec3f(in.uv, sqrt(1.0 - d)) * look.color.w;
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

@fragment
fn thick_fs(in: Sphere) -> @location(0) f32 {
    let d = dot(in.uv, in.uv);
    if (d > 1.0) {
        discard;
    }
    // The chord through a sphere of the radius, softened at the rim.
    return 2.0 * look.color.w * sqrt(1.0 - d) * 0.5;
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
    let pixels = look.color.w * 4.0 * view.proj[1][1] * view.viewport.y * 0.5 / z0;
    let radius = clamp(i32(pixels), 3, 24);
    let sigma = f32(radius) * 0.5;
    let range = look.color.w * 4.0;
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
    let thickness = textureLoad(thick_in, px, 0).r;
    // What lies behind shows through the liquid, absorbed along its thickness.
    let transmit = exp(-look.color.rgb * thickness * 1.5);
    let coverage = 1.0 - (transmit.r + transmit.g + transmit.b) / 3.0;
    let sun = view.sun_dir.w * view.sun_color.rgb / PI;
    let body = vec3f(0.03, 0.16, 0.26) * (max(dot(n, view.sun_dir.xyz), 0.0) * 0.6 + 0.4) * sun;
    let cos_v = clamp(dot(n, to_eye), 0.0, 1.0);
    let fresnel = 0.02 + 0.98 * pow(1.0 - cos_v, 5.0);
    let r = reflect(-to_eye, n);
    let sky = mix(view.fog.rgb, view.sky_color.rgb, clamp(r.y, 0.0, 1.0)) * sun * 0.5;
    let h = normalize(view.sun_dir.xyz + to_eye);
    let spec = pow(max(dot(n, h), 0.0), 400.0) * sun * 3.0;
    let a = clamp(max(coverage, fresnel), 0.0, 1.0);
    var o: SurfaceOut;
    // Premultiplied: the absorbing body over what lies behind, the reflection and highlight added.
    o.color = vec4f(body * coverage * (1.0 - fresnel) + sky * fresnel + spec, a);
    let clip = view.proj * vec4f(p, 1.0);
    o.z = clip.z / clip.w;
    return o;
}
