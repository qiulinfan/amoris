// Staggered shallow water: advection + pressure, donor scaling, conservative transport.
// One invocation owns a column and its -x/-z faces. Dispatch boundaries order all dependencies.
struct SeaWave { phase:vec4f, data:vec4f }
struct Params {
    size: vec2<u32>,
    cell: f32,
    dt: f32,
    friction: f32,
    padding0: f32,
    padding1: f32,
    padding2: f32,
    edge_kind:vec4u,
    edge_rate:vec4f,
    edge_normal:vec4f,
    sea_meta:vec4f,
    waves:array<SeaWave,16>,
}
@group(0) @binding(0) var<uniform> p: Params;
// bed, depth, -x velocity, -z velocity (outer +x/+z faces are walls).
@group(0) @binding(1) var<storage, read> src: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read_write> dst: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read> awake: array<u32>;
@group(0) @binding(4) var<storage, read_write> velocity: array<f32>;
@group(0) @binding(5) var<storage, read_write> scale: array<f32>;
@group(0) @binding(6) var<storage,read_write> edge_flux:array<vec4f>;

fn at(i: u32, k: u32) -> u32 { return i + p.size.x * k; }
fn far_face(n:u32)->f32 {return src[p.size.x*p.size.y+n/4u][n%4u];}
fn u(i: u32, k: u32) -> f32 {
    if i >= p.size.x { return far_face(k); }
    return src[at(i, k)].z;
}
fn w(i: u32, k: u32) -> f32 {
    if k >= p.size.y { return far_face(p.size.y+i); }
    return src[at(i, k)].w;
}
fn sample_face(pos: vec2<f32>, axis: u32) -> f32 {
    let count = p.size + vec2<u32>(select(0u, 1u, axis == 0u), select(0u, 1u, axis == 1u));
    let q = clamp(pos, vec2<f32>(0.0), vec2<f32>(count - vec2<u32>(1u)));
    let ij = min(vec2<u32>(floor(q)), count - vec2<u32>(2u));
    let t = q - vec2<f32>(ij);
    var a: f32; var b: f32; var c: f32; var d: f32;
    if axis == 0u {
        a = u(ij.x, ij.y); b = u(ij.x + 1u, ij.y);
        c = u(ij.x, ij.y + 1u); d = u(ij.x + 1u, ij.y + 1u);
    } else {
        a = w(ij.x, ij.y); b = w(ij.x + 1u, ij.y);
        c = w(ij.x, ij.y + 1u); d = w(ij.x + 1u, ij.y + 1u);
    }
    let top = a + (b - a) * t.x;
    let bottom = c + (d - c) * t.x;
    return top + (bottom - top) * t.y;
}
fn pressure(v0: f32, a: u32, b: u32) -> f32 {
    let ha = src[a].y; let hb = src[b].y;
    if ha <= 0.0001 && hb <= 0.0001 { return 0.0; }
    let ea = src[a].x + ha; let eb = src[b].x + hb;
    let delta = clamp(eb - ea, -ha, hb);
    var v = v0 - 9.81 * p.dt * delta / p.cell;
    let calm = 0.2 * min(1.0, 0.5 / max(abs(v), 0.000001));
    v /= 1.0 + p.dt * (p.friction / max(max(ha, hb), 0.05) + calm);
    if (v > 0.0 && (ha <= 0.0001 || src[b].x >= ea)) ||
       (v < 0.0 && (hb <= 0.0001 || src[a].x >= eb)) { return 0.0; }
    let limit = 0.5 * p.cell / p.dt;
    return clamp(v, -limit, limit);
}
@compute @workgroup_size(8, 8)
fn faces(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x; let k = id.y;
    if i >= p.size.x || k >= p.size.y { return; }
    let n = at(i, k);
    var v = vec2<f32>(0.0);
    if awake[n] == 0u { velocity[n*2u]=src[n].z;velocity[n*2u+1u]=src[n].w; return; }
    let back = p.dt / p.cell;
    if i > 0u && awake[n - 1u] != 0u {
        let cross = (w(i - 1u, k) + w(i, k) + w(i - 1u, k + 1u) + w(i, k + 1u)) * 0.25;
        let pos = vec2<f32>(f32(i) - u(i, k) * back, f32(k) - cross * back);
        v.x = pressure(sample_face(pos, 0u), n - 1u, n);
    }
    if k > 0u && awake[n - p.size.x] != 0u {
        let cross = (u(i, k - 1u) + u(i + 1u, k - 1u) + u(i, k) + u(i + 1u, k)) * 0.25;
        let pos = vec2<f32>(f32(i) - cross * back, f32(k) - w(i, k) * back);
        v.y = pressure(sample_face(pos, 1u), n - p.size.x, n);
    }
    velocity[n*2u]=v.x;velocity[n*2u+1u]=v.y;
}
fn vx(i: u32, k: u32) -> f32 {
    if i >= p.size.x {if(p.edge_kind.y==0u){return 0.0;}return edge_flux[p.size.y+k].w;}
    return velocity[at(i,k)*2u];
}
fn vz(i: u32, k: u32) -> f32 {
    if k >= p.size.y {if(p.edge_kind.w==0u){return 0.0;}return edge_flux[p.size.y*2u+p.size.x+i].w;}
    return velocity[at(i,k)*2u+1u];
}
// Limited linear depth reconstruction; donor budgets bound total outgoing water.
// All interfaces consume the same reconstructed donor state and its column's volume budget.
fn depth_minmod(a:f32,b:f32)->f32 {
    if(a*b<=0.0){return 0.0;}
    return sign(a)*min(abs(a),abs(b));
}
fn depth_states(n:u32)->vec4f {
    let i=n%p.size.x;let k=n/p.size.x;let h=src[n].y;
    var dx=0.0;var dz=0.0;
    if(i>0u && i+1u<p.size.x){dx=depth_minmod(h-src[n-1u].y,src[n+1u].y-h);}
    if(k>0u && k+1u<p.size.y){dz=depth_minmod(h-src[n-p.size.x].y,src[n+p.size.x].y-h);}
    return max(vec4f(h-0.5*dx,h+0.5*dx,h-0.5*dz,h+0.5*dz),vec4f(0.0));
}
@compute @workgroup_size(8,8)
fn donors(@builtin(global_invocation_id) id:vec3u){
    if(any(id.xy>=p.size)){return;}
    let i=id.x;let k=id.y;let n=at(i,k);let h=depth_states(n);
    var rates=vec4f(max(-vx(i,k),0.0),max(vx(i+1u,k),0.0),max(-vz(i,k),0.0),max(vz(i,k+1u),0.0))*h;
    if(i==0u && p.edge_kind.x!=0u){rates.x=max(-edge_flux[k].x,0.0);}
    if(i+1u==p.size.x && p.edge_kind.y!=0u){rates.y=max(edge_flux[p.size.y+k].x,0.0);}
    if(k==0u && p.edge_kind.z!=0u){rates.z=max(-edge_flux[2u*p.size.y+i].x,0.0);}
    if(k+1u==p.size.y && p.edge_kind.w!=0u){rates.w=max(edge_flux[2u*p.size.y+p.size.x+i].x,0.0);}
    let out=(rates.x+rates.y+rates.z+rates.w)*p.dt/p.cell;
    scale[n]=1.0;
    if(out>src[n].y){scale[n]=src[n].y/out;}
}
fn depth_flow(v:f32,a:u32,b:u32,axis:u32)->f32 {
    if(v>0.0){return v*depth_states(a)[axis*2u+1u]*scale[a];}
    return v*depth_states(b)[axis*2u]*scale[b];
}
@compute @workgroup_size(8,8)
fn transport(@builtin(global_invocation_id) id:vec3u){
    if(any(id.xy>=p.size)){return;}
    let i=id.x;let k=id.y;let n=at(i,k);
    if(awake[n]==0u){dst[n]=src[n];return;}
    var net=0.0;
    if(i>0u){net+=depth_flow(vx(i,k),n-1u,n,0u);}else if(p.edge_kind.x!=0u){net+=edge_flux[k].x;}
    if(i+1u<p.size.x){net-=depth_flow(vx(i+1u,k),n,n+1u,0u);}else if(p.edge_kind.y!=0u){net-=edge_flux[p.size.y+k].x;}
    if(k>0u){net+=depth_flow(vz(i,k),n-p.size.x,n,1u);}else if(p.edge_kind.z!=0u){net+=edge_flux[2u*p.size.y+i].x;}
    if(k+1u<p.size.y){net-=depth_flow(vz(i,k+1u),n,n+p.size.x,1u);}else if(p.edge_kind.w!=0u){net-=edge_flux[2u*p.size.y+p.size.x+i].x;}
    dst[n]=vec4f(src[n].x,max(src[n].y+net*p.dt/p.cell,0.0),vec2f(velocity[n*2u],velocity[n*2u+1u]));
}
