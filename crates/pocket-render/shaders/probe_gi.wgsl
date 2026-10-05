// SH radiance with Lambertian convolution, divided by PI. Distance moments reduce light leaks.
struct GiParams {
    origin: vec4f,
    spacing: vec4f,
    dimensions: vec4u,
    settings: vec4f, // intensity, normal bias, max ray distance, enabled
};
@group(1) @binding(8) var<uniform> gi_params: GiParams;
@group(1) @binding(9) var<storage, read> gi_sh: array<vec4f>;
@group(1) @binding(10) var<storage, read> gi_distances: array<vec2f>;

fn gi_sh_diffuse(index: u32, n: vec3f) -> vec3f {
    let j = index * 9u;
    let c1 = 0.488602512 * (2.0 / 3.0);
    let c2 = 1.092548431 * 0.25;
    return max(vec3f(0.0),
        gi_sh[j].xyz * 0.282094792
        + c1 * (gi_sh[j+1u].xyz*n.y + gi_sh[j+2u].xyz*n.z + gi_sh[j+3u].xyz*n.x)
        + c2 * (gi_sh[j+4u].xyz*n.x*n.y + gi_sh[j+5u].xyz*n.y*n.z + gi_sh[j+7u].xyz*n.x*n.z)
        + gi_sh[j+6u].xyz * (0.315391565 * 0.25 * (3.0*n.z*n.z-1.0))
        + gi_sh[j+8u].xyz * (0.546274215 * 0.25 * (n.x*n.x-n.y*n.y)));
}

fn gi_oct_uv(v: vec3f) -> vec2f {
    let d = v / max(abs(v.x)+abs(v.y)+abs(v.z), 1e-6);
    var p = d.xy;
    if d.z < 0.0 {
        p = (1.0-abs(p.yx)) * select(vec2f(-1.0),vec2f(1.0),p>=vec2f(0.0));
    }
    return p*0.5+0.5;
}

fn gi_moments(index: u32, uv: vec2f) -> vec2f {
    let res=gi_params.dimensions.w;
    let texel=clamp(uv*f32(res)-0.5,vec2f(0.0),vec2f(f32(res-1u)));
    let lo=vec2u(floor(texel));
    let hi=min(lo+vec2u(1u),vec2u(res-1u));
    let f=fract(texel);
    let base=index*res*res;
    let a=gi_distances[base+lo.x+lo.y*res];
    let b=gi_distances[base+hi.x+lo.y*res];
    let c=gi_distances[base+lo.x+hi.y*res];
    let d=gi_distances[base+hi.x+hi.y*res];
    return mix(mix(a,b,f.x),mix(c,d,f.x),f.y);
}

fn gi_weight(i: u32) -> f32 {
    return gi_sh[i/4u][i%4u];
}

// Offline-trained 6 -> 32 -> 32 -> 3 MLP. Uses the same diffuse field as the probe teacher.
fn neural_diffuse(world: vec3f, normal: vec3f) -> vec4f {
    let low=gi_params.origin.xyz;
    let size=gi_params.spacing.xyz;
    if any(world<low) || any(world>low+size) { return vec4f(0.0); }
    let p=2.0*(world-low)/size-1.0;
    let input=array<f32,6>(p.x,p.y,p.z,normal.x,normal.y,normal.z);
    var a: array<f32,32>;
    var b: array<f32,32>;
    for (var i=0u;i<32u;i++) {
        var value=gi_weight(192u+i);
        for (var j=0u;j<6u;j++) { value+=gi_weight(i*6u+j)*input[j]; }
        a[i]=max(value,0.0);
    }
    for (var i=0u;i<32u;i++) {
        var value=gi_weight(1248u+i);
        for (var j=0u;j<32u;j++) { value+=gi_weight(224u+i*32u+j)*a[j]; }
        b[i]=max(value,0.0);
    }
    var color: vec3f;
    for (var i=0u;i<3u;i++) {
        var value=gi_weight(1376u+i);
        for (var j=0u;j<32u;j++) { value+=gi_weight(1280u+i*32u+j)*b[j]; }
        color[i]=max(value,0.0);
    }
    return vec4f(color*gi_params.settings.x,1.0);
}

// w=1 means the point lies in the baked field, including fully occluded/dark regions.
fn baked_diffuse(world: vec3f, normal: vec3f) -> vec4f {
    if gi_params.settings.w == 0.0 || gi_params.settings.x == 0.0 {
        return vec4f(0.0);
    }
    if gi_params.settings.w == 2.0 { return neural_diffuse(world,normal); }
    let query_grid = (world-gi_params.origin.xyz)/gi_params.spacing.xyz;
    let dims = gi_params.dimensions.xyz;
    if any(query_grid<vec3f(-0.5)) || any(query_grid>vec3f(dims)-vec3f(0.5)) { return vec4f(0.0); }
    let grid = clamp(query_grid,vec3f(0.0),vec3f(dims-vec3u(1u)));
    let lo = vec3u(floor(grid));
    let hi = min(lo+vec3u(1u),dims-vec3u(1u));
    let f = fract(grid);
    let p = world+normal*gi_params.settings.y;
    var color = vec3f(0.0);
    var sum = 0.0;
    let res = gi_params.dimensions.w;
    for (var z=0u;z<2u;z++) {
        for (var y=0u;y<2u;y++) {
            for (var x=0u;x<2u;x++) {
                let c=select(lo,hi,vec3u(x,y,z)>vec3u(0u));
                let wv=select(vec3f(1.0)-f,f,vec3u(x,y,z)>vec3u(0u));
                var weight=wv.x*wv.y*wv.z;
                if weight<=0.0 { continue; }
                let index=c.x+dims.x*(c.y+dims.y*c.z);
                let at=gi_params.origin.xyz+vec3f(c)*gi_params.spacing.xyz;
                let to=p-at;
                let distance=length(to);
                let direction=to/max(distance,1e-6);
                let uv=gi_oct_uv(direction);
                let moments=gi_moments(index,uv);
                // Zero moments mark a probe rejected inside geometry by the baker.
                if moments.y == 0.0 { continue; }
                if distance>moments.x {
                    let variance=max(moments.y-moments.x*moments.x,1e-5);
                    let delta=distance-moments.x;
                    let visibility=variance/(variance+delta*delta);
                    weight*=visibility*visibility*visibility;
                }
                let facing=max(0.05,0.5+0.5*dot(normal,-direction));
                weight*=facing*facing;
                color+=weight*gi_sh_diffuse(index,normal);
                sum+=weight;
            }
        }
    }
    return vec4f(color/max(sum,1e-6)*gi_params.settings.x,1.0);
}
