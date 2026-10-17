enable wgpu_ray_query;
// Independent surface transport equations; no reference implementation code is copied.
// GGX visible-normal sampling: Heitz, JCGT 7(4), 2018, https://jcgt.org/published/0007/04/01/.
// Dielectric transport/Jacobians: PBRT 4e §9.5 (equations), https://pbr-book.org/4ed/Reflection_Models/Dielectric_BSDF.
// Directions point away from the surface. wo.z > 0, eta = transmitted IOR / incident IOR.
const PT_PI: f32 = 3.141592653589793;
struct PtBsdf {
    base: vec3<f32>, metallic: f32, roughness: f32, transmission: f32, eta: f32,
};
struct PtBsdfEval { value: vec3<f32>, pdf: f32 }; // value is f * abs(wi.z)
struct PtBsdfSample { direction: vec3<f32>, weight: vec3<f32>, pdf: f32, delta: u32, eta_scale: f32 };
fn pt_fresnel(cosine: f32, eta: f32) -> f32 {
    let c = clamp(abs(cosine), 0.0, 1.0);
    let s2 = max(0.0, 1.0 - c*c) / (eta*eta);
    if s2 >= 1.0 { return 1.0; }
    let ct = sqrt(1.0 - s2);
    let rs = (c - eta*ct) / max(1e-20, c + eta*ct);
    let rp = (eta*c - ct) / max(1e-20, eta*c + ct);
    return 0.5 * (rs*rs + rp*rp);
}
fn pt_schlick(f0: vec3<f32>, cosine: f32) -> vec3<f32> {
    let m = 1.0 - clamp(abs(cosine), 0.0, 1.0);
    return f0 + (vec3<f32>(1.0) - f0) * (m*m*m*m*m);
}
fn pt_f0(b: PtBsdf) -> vec3<f32> {
    let r = (b.eta - 1.0) / (b.eta + 1.0);
    return mix(vec3<f32>(r*r), b.base, b.metallic);
}
fn pt_luminance(c: vec3<f32>) -> f32 { return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722)); }
fn pt_glass_weight(b: PtBsdf) -> f32 { return (1.0 - b.metallic) * b.transmission; }
fn pt_spec_probability(b: PtBsdf) -> f32 {
    if b.metallic >= 0.999999 { return 1.0; }
    let s = pt_luminance(pt_f0(b));
    let d = pt_luminance(b.base) * (1.0 - b.metallic);
    return clamp(s / max(1e-8, s + d), 0.1, 0.9);
}
fn pt_ggx_d(h: vec3<f32>, alpha: f32) -> f32 {
    let a2 = alpha * alpha;
    // Algebraically equivalent for a unit h, without catastrophic cancellation at alpha=.002.
    let denom = dot(h.xy, h.xy) + a2*h.z*h.z;
    return a2 / max(1e-30, PT_PI * denom*denom);
}
fn pt_lambda(w: vec3<f32>, alpha: f32) -> f32 {
    let cos2 = w.z*w.z;
    if cos2 < 1e-20 { return 1e20; }
    return 0.5 * (sqrt(1.0 + alpha*alpha * max(0.0, 1.0 - cos2)/cos2) - 1.0);
}
fn pt_ggx_h_pdf(wo: vec3<f32>, h: vec3<f32>, alpha: f32) -> f32 {
    return pt_ggx_d(h, alpha) * abs(dot(wo, h)) / max(1e-20, wo.z * (1.0 + pt_lambda(wo, alpha)));
}
fn pt_visible_normal(wo: vec3<f32>, alpha: f32, u: vec2<f32>) -> vec3<f32> {
    let v = normalize(vec3<f32>(alpha*wo.xy, wo.z));
    let lensq = dot(v.xy, v.xy);
    var t1 = vec3<f32>(1.0, 0.0, 0.0);
    if lensq > 1e-20 { t1 = vec3<f32>(-v.y, v.x, 0.0) / sqrt(lensq); }
    let t2 = cross(v, t1);
    let radius = sqrt(u.x);
    let angle = 2.0*PT_PI*u.y;
    let diskx = radius*cos(angle);
    var disky = radius*sin(angle);
    let s = 0.5*(1.0 + v.z);
    disky = (1.0 - s)*sqrt(max(0.0, 1.0 - diskx*diskx)) + s*disky;
    let nh = diskx*t1 + disky*t2 + sqrt(max(0.0, 1.0 - diskx*diskx - disky*disky))*v;
    return normalize(vec3<f32>(alpha*nh.xy, max(0.0, nh.z)));
}
fn pt_bsdf_eval(b: PtBsdf, wo: vec3<f32>, wi: vec3<f32>) -> PtBsdfEval {
    if wo.z <= 0.0 || abs(wi.z) < 1e-8 { return PtBsdfEval(vec3<f32>(0.0), 0.0); }
    let glass = pt_glass_weight(b);
    let opaque = 1.0 - glass;
    let spec = pt_spec_probability(b);
    let delta_surface = b.roughness <= 0.0001;
    let alpha = max(0.002, b.roughness*b.roughness);
    if wi.z > 0.0 {
        let h = normalize(wo + wi);
        let fresnel = pt_schlick(pt_f0(b), select(dot(wo, h), wo.z, delta_surface));
        var value = opaque * b.base * (1.0 - b.metallic) * (vec3<f32>(1.0) - fresnel) * wi.z/PT_PI;
        var pdf = opaque * (1.0 - spec) * wi.z/PT_PI;
        if !delta_surface {
            let hp = pt_ggx_h_pdf(wo, h, alpha);
            let rp = hp / max(1e-20, 4.0*abs(dot(wo, h)));
            let f = pt_fresnel(dot(wo, h), b.eta);
            let g = 1.0 / (1.0 + pt_lambda(wo, alpha) + pt_lambda(wi, alpha));
            value += (opaque*fresnel + glass*vec3<f32>(f)) * (pt_ggx_d(h, alpha)*g / (4.0*wo.z));
            pdf += (opaque*spec + glass*f)*rp;
        }
        return PtBsdfEval(value, pdf);
    }
    if delta_surface || glass <= 0.0 || abs(b.eta - 1.0) < 1e-6 { return PtBsdfEval(vec3<f32>(0.0), 0.0); }
    var h = normalize(wo + b.eta*wi);
    if h.z < 0.0 { h = -h; }
    let oh = dot(wo, h);
    let ih = dot(wi, h);
    if oh <= 0.0 || ih >= 0.0 { return PtBsdfEval(vec3<f32>(0.0), 0.0); }
    let f = pt_fresnel(oh, b.eta);
    let denominator = ih + oh/b.eta;
    let denom2 = denominator*denominator;
    if denom2 < 1e-20 { return PtBsdfEval(vec3<f32>(0.0), 0.0); }
    let g = 1.0 / (1.0 + pt_lambda(wo, alpha) + pt_lambda(wi, alpha));
    let value = glass*b.base*(1.0 - f)*pt_ggx_d(h, alpha)*g*abs(ih*oh) / (wo.z*denom2*b.eta*b.eta);
    let pdf = glass*(1.0 - f)*pt_ggx_h_pdf(wo, h, alpha)*abs(ih)/denom2;
    return PtBsdfEval(value, pdf);
}
fn pt_bsdf_sample(b: PtBsdf, wo: vec3<f32>, u: vec4<f32>) -> PtBsdfSample {
    let empty = PtBsdfSample(vec3<f32>(0.0), vec3<f32>(0.0), 0.0, 0u, 1.0);
    if wo.z <= 0.0 { return empty; }
    let glass = pt_glass_weight(b);
    let opaque = 1.0 - glass;
    let spec = pt_spec_probability(b);
    let choose_glass = u.x < glass;
    let delta_surface = b.roughness <= 0.0001;
    var wi = vec3<f32>(0.0);
    var transmitted = false;
    if choose_glass || u.y < spec {
        var h = vec3<f32>(0.0, 0.0, 1.0);
        if !delta_surface { h = pt_visible_normal(wo, max(0.002, b.roughness*b.roughness), u.zw); }
        let oh = dot(wo, h);
        let f = pt_fresnel(oh, b.eta);
        if choose_glass && u.y >= f {
            let discriminant = 1.0 - max(0.0, 1.0 - oh*oh)/(b.eta*b.eta);
            if discriminant <= 0.0 { return empty; }
            wi = -wo/b.eta + h*(oh/b.eta - sqrt(discriminant));
            if wi.z >= 0.0 { return empty; }
            transmitted = true;
        } else {
            wi = -wo + 2.0*oh*h;
            if wi.z <= 0.0 { return empty; }
        }
        // Exact index matching has a delta transmission even for a rough interface.
        if delta_surface || (choose_glass && abs(b.eta - 1.0) < 1e-6) {
            if transmitted {
                return PtBsdfSample(wi, b.base/(b.eta*b.eta), glass*(1.0 - f), 1u, b.eta*b.eta);
            }
            let pdf = opaque*spec + glass*f;
            let value = opaque*pt_schlick(pt_f0(b), wo.z) + glass*vec3<f32>(f);
            return PtBsdfSample(wi, value/max(1e-20, pdf), pdf, 1u, 1.0);
        }
    } else {
        let radius = sqrt(u.z);
        let angle = 2.0*PT_PI*u.w;
        wi = vec3<f32>(radius*cos(angle), radius*sin(angle), sqrt(max(0.0, 1.0 - u.z)));
    }
    let result = pt_bsdf_eval(b, wo, wi);
    if result.pdf <= 1e-20 { return empty; }
    return PtBsdfSample(wi, result.value/result.pdf, result.pdf, 0u, select(1.0, b.eta*b.eta, transmitted));
}
