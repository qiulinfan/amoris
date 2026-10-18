// The box's liquid drawn as its particles (fluid.rs): each a sphere impostor, a camera-facing quad
// whose fragments take the sphere's normal and depth, so the particles occlude each other and the
// scene correctly. Lit by the sun with a little ambient; faster particles are paler (foam).

struct Particle {
    x: vec3f,
    _a: f32,
    v: vec3f,
    _b: f32,
    c: mat3x3f,
};

struct Look {
    origin: vec4f,        // the grid's corner in the world; w: metres per cell
    color: vec4f,         // the liquid's colour; w: the spheres' radius, metres
    foam: vec4f,          // the foam's colour; w: the speed (cells per substep) at full foam
};

@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var<storage, read> particles: array<Particle>;
@group(0) @binding(2) var<uniform> look: Look;

struct Out {
    @builtin(position) clip: vec4f,
    @location(0) uv: vec2f,
    @location(1) center: vec3f,   // view space
    @location(2) foam: f32,
};

@vertex
fn vs(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> Out {
    var o: Out;
    let p = particles[ii];
    let world = look.origin.xyz + p.x * look.origin.w;
    let corners = array<vec2f, 6>(vec2f(-1.0, -1.0), vec2f(1.0, -1.0), vec2f(1.0, 1.0),
                                  vec2f(-1.0, -1.0), vec2f(1.0, 1.0), vec2f(-1.0, 1.0));
    let c = corners[vi];
    let center = (view.view * vec4f(world, 1.0)).xyz;
    let r = look.color.w;
    o.center = center;
    o.uv = c;
    // The quad faces the camera, just in front of the sphere's centre.
    o.clip = view.proj * vec4f(center + vec3f(c * r, r), 1.0);
    o.foam = clamp(length(p.v) / look.foam.w, 0.0, 1.0);
    return o;
}

struct Frag {
    @location(0) color: vec4f,
    @builtin(frag_depth) depth: f32,
};

@fragment
fn fs(in: Out) -> Frag {
    let d = dot(in.uv, in.uv);
    if (d > 1.0) {
        discard;
    }
    let r = look.color.w;
    let n_view = vec3f(in.uv, sqrt(1.0 - d));
    let p_view = in.center + n_view * r;
    let clip = view.proj * vec4f(p_view, 1.0);
    // The view's rotation inverted: the normal in the world.
    let rot = mat3x3f(view.view[0].xyz, view.view[1].xyz, view.view[2].xyz);
    let n = normalize(transpose(rot) * n_view);
    let albedo = mix(look.color.rgb, look.foam.rgb, in.foam * in.foam);
    let sun = max(dot(n, view.sun_dir.xyz), 0.0) * view.sun_dir.w / PI * view.sun_color.rgb;
    let sky = view.sky_color.rgb * view.sun_color.w * (0.5 + 0.5 * n.y) * view.sun_dir.w / PI * 0.35;
    var o: Frag;
    o.color = vec4f(albedo * (sun + sky), 1.0);
    o.depth = clip.z / clip.w;
    return o;
}
