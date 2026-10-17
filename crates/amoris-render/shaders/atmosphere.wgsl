// The sky: a physically based single-scattering atmosphere (Rayleigh, Mie, ozone; Hillaire 2020's
// constants) rendered into a cube when the sun moves, then drawn behind everything and used as the
// image-based light. The sun disk is drawn only in the visible sky, never into the light cube (its
// specular comes from the analytic sun).

const R_GROUND: f32 = 6360e3;
const R_TOP: f32 = 6460e3;
const RAYLEIGH: vec3f = vec3f(5.802e-6, 13.558e-6, 33.1e-6);
const MIE_SCATTER: f32 = 3.996e-6;
const MIE_EXTINCT: f32 = 4.40e-6 + 3.996e-6;
const OZONE: vec3f = vec3f(0.650e-6, 1.881e-6, 0.085e-6);

fn ray_sphere(o: vec3f, d: vec3f, r: f32) -> vec2f {
    let b = dot(o, d);
    let c = dot(o, o) - r * r;
    let h = b * b - c;
    if (h < 0.0) {
        return vec2f(-1.0, -1.0);
    }
    let s = sqrt(h);
    return vec2f(-b - s, -b + s);
}

fn densities(height: f32) -> vec3f {
    let rayleigh = exp(-height / 8000.0);
    let mie = exp(-height / 1200.0);
    let ozone = max(0.0, 1.0 - abs(height - 25000.0) / 15000.0);
    return vec3f(rayleigh, mie, ozone);
}

fn extinction(d: vec3f) -> vec3f {
    return RAYLEIGH * d.x + vec3f(MIE_EXTINCT) * d.y + OZONE * d.z;
}

fn transmittance_to_sun(p: vec3f, sun: vec3f) -> vec3f {
    let t = ray_sphere(p, sun, R_TOP).y;
    let g = ray_sphere(p, sun, R_GROUND);
    if (g.x > 0.0) {
        return vec3f(0.0);
    }
    let steps = 8;
    let dt = t / f32(steps);
    var optical = vec3f(0.0);
    for (var i = 0; i < steps; i++) {
        let q = p + sun * (f32(i) + 0.5) * dt;
        optical += extinction(densities(length(q) - R_GROUND)) * dt;
    }
    return exp(-optical);
}

// Radiance of the sky toward `dir` seen from `altitude` metres, for a sun of unit illuminance.
fn atmosphere(dir: vec3f, sun: vec3f, altitude: f32) -> vec3f {
    let o = vec3f(0.0, R_GROUND + altitude, 0.0);
    var t_max = ray_sphere(o, dir, R_TOP).y;
    let g = ray_sphere(o, dir, R_GROUND);
    if (g.x > 0.0) {
        t_max = g.x;
    }
    let steps = 24;
    let dt = t_max / f32(steps);
    let mu = dot(dir, sun);
    let phase_r = 3.0 / (16.0 * PI) * (1.0 + mu * mu);
    let gm = 0.8;
    let phase_m = 3.0 / (8.0 * PI) * ((1.0 - gm * gm) * (1.0 + mu * mu))
        / ((2.0 + gm * gm) * pow(1.0 + gm * gm - 2.0 * gm * mu, 1.5));
    var trans = vec3f(1.0);
    var light = vec3f(0.0);
    for (var i = 0; i < steps; i++) {
        let p = o + dir * (f32(i) + 0.5) * dt;
        let d = densities(length(p) - R_GROUND);
        let ext = extinction(d);
        let sun_t = transmittance_to_sun(p, sun);
        let scatter = (RAYLEIGH * d.x * phase_r + vec3f(MIE_SCATTER * d.y * phase_m)) * sun_t;
        // Analytic integration over the step (Hillaire's energy-conserving form).
        let step_t = exp(-ext * dt);
        light += trans * (scatter - scatter * step_t) / max(ext, vec3f(1e-12));
        trans *= step_t;
    }
    // A little multiple scattering, as an isotropic lift of the shadowed side.
    return light * 1.6;
}

