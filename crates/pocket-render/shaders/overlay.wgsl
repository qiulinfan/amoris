// Editor overlays, drawn after the display transform on top of the image: world-space lines of a
// pixel width and filled polygons (the gizmo's shapes), the selection's outline (an edge of the
// selection mask), and, inside the HDR pass, the ground grid with the world axes.

@group(0) @binding(0) var<uniform> view: View;

// --- Lines: one instance per segment, a screen-space quad of `width` pixels ---------------------

struct Segment {
    @location(0) a: vec3f,
    @location(1) b: vec3f,
    @location(2) color: vec4f,
    @location(3) width: f32,
};

struct LineOut {
    @builtin(position) clip: vec4f,
    @location(0) color: vec4f,
};

@vertex
fn vs_line(@builtin(vertex_index) vi: u32, s: Segment) -> LineOut {
    var ca = view.view_proj * vec4f(s.a, 1.0);
    var cb = view.view_proj * vec4f(s.b, 1.0);
    // Clip the segment to the near plane (reversed Z: z <= w is in front).
    let near_a = ca.w - ca.z;
    let near_b = cb.w - cb.z;
    if (near_a < 0.0 && near_b < 0.0) {
        var o: LineOut;
        o.clip = vec4f(2.0, 2.0, 0.0, 1.0);
        return o;
    }
    if (near_a < 0.0) {
        ca = mix(ca, cb, near_a / (near_a - near_b) + 1e-4);
    }
    if (near_b < 0.0) {
        cb = mix(cb, ca, near_b / (near_b - near_a) + 1e-4);
    }
    let pa = ca.xy / ca.w;
    let pb = cb.xy / cb.w;
    let px = view.viewport.xy;
    var dir = (pb - pa) * px;
    if (length(dir) < 1e-6) {
        dir = vec2f(1.0, 0.0);
    }
    dir = normalize(dir);
    let n = vec2f(-dir.y, dir.x) * s.width * 0.5 / px * 2.0;
    let corner = array<vec2f, 6>(vec2f(0.0, -1.0), vec2f(1.0, -1.0), vec2f(1.0, 1.0),
                                 vec2f(0.0, -1.0), vec2f(1.0, 1.0), vec2f(0.0, 1.0));
    let c = corner[vi];
    let p = mix(pa, pb, c.x) + n * c.y;
    var o: LineOut;
    o.clip = vec4f(p, 0.5, 1.0);
    o.color = s.color;
    return o;
}

// --- Polygons: triangles in world space, flat colour ---------------------------------------------

struct PolyIn {
    @location(0) p: vec3f,
    @location(1) color: vec4f,
};

@vertex
fn vs_poly(v: PolyIn) -> LineOut {
    var o: LineOut;
    let c = view.view_proj * vec4f(v.p, 1.0);
    o.clip = vec4f(c.xy / max(c.w, 1e-5), 0.5, 1.0);
    o.color = v.color;
    return o;
}

// Overlay colours are sRGB; the output may encode sRGB itself (params.y).
struct OverlayParams {
    params: vec4f,     // x: output is sRGB-encoded by the surface (1/0); y: outline width px
    select: vec4f,     // selection outline colour (sRGB)
    hover: vec4f,      // hover outline colour (sRGB)
};

@group(0) @binding(1) var<uniform> op: OverlayParams;

fn to_output(c: vec4f) -> vec4f {
    if (op.params.x > 0.5) {
        return vec4f(srgb_to_linear(c.rgb), c.a);
    }
    return c;
}

@fragment
fn fs_flat(in: LineOut) -> @location(0) vec4f {
    return to_output(in.color);
}

// --- Selection outline: edges of the mask (1 selected, 2 hovered) -------------------------------

@group(0) @binding(2) var mask: texture_2d<u32>;

struct FullOut {
    @builtin(position) pos: vec4f,
};

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> FullOut {
    let p = vec2f(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
    var o: FullOut;
    o.pos = vec4f(p, 0.0, 1.0);
    return o;
}

@fragment
fn fs_outline(in: FullOut) -> @location(0) vec4f {
    let dim = vec2i(textureDimensions(mask));
    let p = vec2i(in.pos.xy);
    let here = textureLoad(mask, clamp(p, vec2i(0), dim - 1), 0).r;
    let r = i32(op.params.y);
    var near = 0u;
    for (var y = -r; y <= r; y++) {
        for (var x = -r; x <= r; x++) {
            if (x * x + y * y > r * r) {
                continue;
            }
            let q = clamp(p + vec2i(x, y), vec2i(0), dim - 1);
            near = max(near, textureLoad(mask, q, 0).r);
        }
    }
    if (near == 0u || here == near) {
        // Inside a selected silhouette: a faint tint; elsewhere nothing.
        if (here == 1u) {
            return to_output(vec4f(op.select.rgb, 0.06));
        }
        discard;
    }
    let c = select(op.select, op.hover, near == 2u);
    return to_output(c);
}

// --- Selection mask: the selected entities' instances, by slot ----------------------------------

@group(0) @binding(3) var<storage, read> instances: array<Instance>;

struct MaskIn {
    @location(0) position: vec3f,
};

struct MaskOut {
    @builtin(position) clip: vec4f,
    @location(0) @interpolate(flat) value: u32,
};

@vertex
fn vs_mask(v: MaskIn, @builtin(instance_index) slot: u32) -> MaskOut {
    let inst = instances[slot & 0x7fffffffu];
    let pose = instance_pose(inst, view.params.x);
    let world = pose.pos + quat_rotate(pose.rot, v.position * inst.scale);
    var o: MaskOut;
    o.clip = view.view_proj * vec4f(world, 1.0);
    // The top bit marks the hovered entity.
    o.value = select(1u, 2u, (slot & 0x80000000u) != 0u);
    return o;
}

@fragment
fn fs_mask(in: MaskOut) -> @location(0) u32 {
    return in.value;
}

// --- Ground grid and world axes (inside the HDR pass, depth-tested) -----------------------------

struct GridOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
};

@vertex
fn vs_grid(@builtin(vertex_index) i: u32) -> GridOut {
    let c = array<vec2f, 6>(vec2f(-1.0, -1.0), vec2f(1.0, -1.0), vec2f(1.0, 1.0),
                            vec2f(-1.0, -1.0), vec2f(1.0, 1.0), vec2f(-1.0, 1.0));
    let size = 2000.0;
    let centre = floor(view.camera_pos.xz / 10.0) * 10.0;
    let p = vec3f(centre.x + c[i].x * size, 0.0, centre.y + c[i].y * size);
    var o: GridOut;
    o.clip = view.view_proj * vec4f(p, 1.0);
    o.world = p;
    return o;
}

fn grid_lines(p: vec2f, cell: f32) -> f32 {
    let g = p / cell;
    let w = max(fwidth(g), vec2f(1e-5));
    let d = abs(fract(g - 0.5) - 0.5) / w;
    // Fade cells smaller than a few pixels instead of letting the lines fill them.
    let keep = clamp(1.0 - max(w.x, w.y) * 4.0, 0.0, 1.0);
    return (1.0 - min(min(d.x, d.y), 1.0)) * keep;
}

@fragment
fn fs_grid(in: GridOut) -> @location(0) vec4f {
    let p = in.world.xz;
    let dist = distance(view.camera_pos.xyz, in.world);
    let fine = grid_lines(p, 1.0) * clamp(1.0 - dist / 60.0, 0.0, 1.0);
    let coarse = grid_lines(p, 10.0) * clamp(1.0 - dist / 600.0, 0.0, 1.0);
    var a = max(fine * 0.35, coarse * 0.6);
    var col = vec3f(0.55);
    let axes = op.params.z > 0.5;
    if (axes) {
        let w = max(fwidth(p), vec2f(1e-5));
        // A line about 1.5 pixels wide, faded out where a pixel spans more than half a metre
        // (grazing angles), so the axis never smears into a band.
        let ax = (1.0 - min(abs(p.y) / (w.y * 1.5), 1.0)) * clamp(1.0 - w.y / 0.5, 0.0, 1.0);
        let az = (1.0 - min(abs(p.x) / (w.x * 1.5), 1.0)) * clamp(1.0 - w.x / 0.5, 0.0, 1.0);
        if (ax > 0.0) {
            col = mix(col, vec3f(0.9, 0.2, 0.2), ax);
            a = max(a, ax);
        }
        if (az > 0.0) {
            col = mix(col, vec3f(0.2, 0.4, 0.95), az);
            a = max(a, az);
        }
    }
    if (a <= 0.003) {
        discard;
    }
    // In the HDR target before exposure: scale so the lines read on a sunlit scene.
    return vec4f(col * 1.4 / max(view.params.y, 1e-3), a);
}
