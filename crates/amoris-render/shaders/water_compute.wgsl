// Staggered shallow water: advection + pressure, donor scaling, conservative transport.
// One invocation owns a column and its -x/-z faces. Dispatch boundaries order all dependencies.
struct SeaWave { phase:vec4f, data:vec4f }
struct Params {
    size: vec2<u32>,
    cell: f32,
    dt: f32,
    friction: f32,
    breaking: f32,      // 1: eddy-viscosity breaking (MacCormack faces)
    fall_min: f32,      // ballistic falls over drops deeper than this, metres; 0: none
    occupancy_on: f32,  // 1: bodies' displaced volume adds a pressure head
    calm: f32,          // calm damping of slow faces, per second (water.md 4)
    advection: f32,     // 0: MacCormack, 1: momentum-conserving, 2: conserving where rapid (61.1)
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
// Per column: fixed-point fallen depth and its x/z momentum, gathered at landing columns.
@group(0) @binding(10) var<storage,read_write> falls:array<atomic<i32>>;

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
    return pressure_heads(v0, a, b, 0.0, 0.0, false);
}
// Displaced body volume (oa, ob: metres of head) loads the water as a floating lid would. With
// `climb` (the resident kernel, water.md 61.1), water flows onto a column whose bed stands above
// the giving surface when its velocity head carries it there, as a front running up a beach does.
fn pressure_heads(v0: f32, a: u32, b: u32, oa: f32, ob: f32, climb: bool) -> f32 {
    let ha = src[a].y; let hb = src[b].y;
    if ha <= 0.0001 && hb <= 0.0001 { return 0.0; }
    let ea = src[a].x + ha + oa; let eb = src[b].x + hb + ob;
    // The higher side pushes with at most its own depth over a step; a bed rising less than half
    // a column over one is a slope, whose fall all the water feels, however thin (`climb`).
    var delta = clamp(eb - ea, -(ha + oa), hb + ob);
    if (climb && abs(src[b].x - src[a].x) <= 0.5 * p.cell) { delta = eb - ea; }
    var v = v0 - 9.81 * p.dt * delta / p.cell;
    let calm = p.calm * min(1.0, 0.5 / max(abs(v), 0.000001));
    v /= 1.0 + p.dt * (p.friction / max(max(ha, hb), 0.05) + calm);
    let head = select(0.0, v * v / (2.0 * 9.81), climb);
    if (v > 0.0 && (ha <= 0.0001 || src[b].x >= ea + head)) ||
       (v < 0.0 && (hb <= 0.0001 || src[a].x >= eb + head)) { return 0.0; }
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
// Ballistic falls. A face whose donor bed stands above the receiver's surface by more than
// fall_min carries its discharge through the air: the donor loses it as usual, the receiver
// does not gain it, and the column where the trajectory meets water or bed receives it. Both
// sides quantize the same discharge identically, so the transfer conserves volume exactly.
// The hand-off from a height field to free flight at a lip is in the spirit of N. Chentanez,
// M. Mueller and T.-Y. Kim, "Coupling 3D Eulerian, Heightfield and Particle Methods for
// Interactive Simulation of Large Scale Liquid Phenomena", IEEE TVCG 21(10), 2015; this
// variant lands the flux analytically within the substep rather than carrying particles.
const FALL_SCALE: f32 = 262144.0;
const FALL_STEPS: u32 = 96u;
fn fall_face(donor:u32,receiver:u32)->bool {
    if(p.fall_min<=0.0||src[donor].y<=1e-4){return false;}
    return src[donor].x-(src[receiver].x+src[receiver].y)>p.fall_min;
}
fn fall_units(depth:f32)->i32 {return i32(round(clamp(depth,0.0,64.0)*FALL_SCALE));}
fn fall_depth(units:i32)->f32 {return f32(units)/FALL_SCALE;}
// The landing column: march the jet's parabola over the columns beyond the lip.
fn fall_landing(donor:u32,receiver:u32,axis:u32,v:f32)->u32 {
    let di=vec2f(f32(donor%p.size.x),f32(donor/p.size.x));
    let ri=vec2f(f32(receiver%p.size.x),f32(receiver/p.size.x));
    // The lip: the shared face's centre, at mid-depth of the leaving water.
    let lip=(di+ri)*0.5+vec2f(0.5);
    let y0=src[donor].x+0.5*src[donor].y;
    let along=ri-di;
    var cross=0.0;
    let d=donor;
    if(axis==0u){
        let k=d/p.size.x;let i=d%p.size.x;
        cross=(w(i,k)+w(i,k+1u))*0.5;
    }else{
        let k=d/p.size.x;let i=d%p.size.x;
        cross=(u(i,k)+u(i+1u,k))*0.5;
    }
    let vel=along*abs(v)+select(vec2f(cross,0.0),vec2f(0.0,cross),axis==0u);
    let speed=max(length(vel),1e-4);
    var landing=receiver;
    let limit=vec2f(p.size)-vec2f(0.001);
    for(var s=0u;s<FALL_STEPS;s++){
        // Half a column of travel at a time, starting inside the receiver.
        let t=(f32(s)*0.5+0.5)*p.cell/speed;
        let at=clamp(lip+vel*(t/p.cell),vec2f(0.0),limit);
        let c=vec2u(floor(at));
        let n=c.x+p.size.x*c.y;
        landing=n;
        let y=y0-0.5*9.81*t*t;
        if(y<=src[n].x+src[n].y){break;}
    }
    return landing;
}
fn fall_deposit(landing:u32,units:i32,velocity:vec2f){
    atomicAdd(&falls[landing*3u],units);
    let m=velocity*fall_depth(units)*FALL_SCALE;
    atomicAdd(&falls[landing*3u+1u],i32(round(clamp(m.x,-1.0e9,1.0e9))));
    atomicAdd(&falls[landing*3u+2u],i32(round(clamp(m.y,-1.0e9,1.0e9))));
}
// Net inflow through one face: positive enters column n. A fall face moves its quantized
// discharge to the landing column instead of the receiving neighbour.
fn face_net(v:f32,a:u32,b:u32,axis:u32,n:u32)->f32 {
    let flow=depth_flow(v,a,b,axis);
    if(flow==0.0||p.fall_min<=0.0){return select(-flow,flow,n==b);}
    let donor=select(b,a,flow>0.0);let receiver=select(a,b,flow>0.0);
    if(!fall_face(donor,receiver)){return select(-flow,flow,n==b);}
    let units=fall_units(abs(flow)*p.dt/p.cell);
    let moved=fall_depth(units)*p.cell/p.dt;
    if(n==donor){return -moved;}
    let direction=select(-1.0,1.0,flow>0.0);
    let landing=fall_landing(donor,receiver,axis,direction*max(abs(v),0.0));
    let along=select(vec2f(0.0,direction*abs(v)),vec2f(direction*abs(v),0.0),axis==0u);
    fall_deposit(landing,units,along);
    return 0.0;
}
@compute @workgroup_size(8,8)
fn transport(@builtin(global_invocation_id) id:vec3u){
    if(any(id.xy>=p.size)){return;}
    let i=id.x;let k=id.y;let n=at(i,k);
    if(awake[n]==0u){dst[n]=src[n];return;}
    var net=0.0;
    if(i>0u){net+=face_net(vx(i,k),n-1u,n,0u,n);}else if(p.edge_kind.x!=0u){net+=edge_flux[k].x;}
    if(i+1u<p.size.x){net+=face_net(vx(i+1u,k),n,n+1u,0u,n);}else if(p.edge_kind.y!=0u){net-=edge_flux[p.size.y+k].x;}
    if(k>0u){net+=face_net(vz(i,k),n-p.size.x,n,1u,n);}else if(p.edge_kind.z!=0u){net+=edge_flux[2u*p.size.y+i].x;}
    if(k+1u<p.size.y){net+=face_net(vz(i,k+1u),n,n+p.size.x,1u,n);}else if(p.edge_kind.w!=0u){net-=edge_flux[2u*p.size.y+p.size.x+i].x;}
    dst[n]=vec4f(src[n].x,max(src[n].y+net*p.dt/p.cell,0.0),vec2f(velocity[n*2u],velocity[n*2u+1u]));
}
// Landed depth first, into the free donor-scale scratch, so the next pass reads stable depths.
@compute @workgroup_size(8,8)
fn fall_land_depth(@builtin(global_invocation_id) id:vec3u){
    if(any(id.xy>=p.size)){return;}
    let n=at(id.x,id.y);
    scale[n]=dst[n].y+fall_depth(atomicLoad(&falls[n*3u]));
}
// Each column writes only its own element: new depth and its -x/-z faces, which take half of
// each neighbour's landed momentum. Whole-vector stores keep the write owned by one invocation.
@compute @workgroup_size(8,8)
fn fall_land_faces(@builtin(global_invocation_id) id:vec3u){
    if(any(id.xy>=p.size)){return;}
    let i=id.x;let k=id.y;let n=at(i,k);
    var c=dst[n];
    c.y=scale[n];
    let limit=0.5*p.cell/p.dt;
    if(i>0u&&awake[n-1u]!=0u&&awake[n]!=0u){c.z=fall_face_velocity(c.z,n-1u,n,1u);}
    if(k>0u&&awake[n-p.size.x]!=0u&&awake[n]!=0u){c.w=fall_face_velocity(c.w,n-p.size.x,n,2u);}
    dst[n]=vec4f(c.x,c.y,clamp(c.z,-limit,limit),clamp(c.w,-limit,limit));
}
fn fall_face_velocity(v:f32,a:u32,b:u32,component:u32)->f32 {
    let volume=0.5*(fall_depth(atomicLoad(&falls[a*3u]))+fall_depth(atomicLoad(&falls[b*3u])));
    if(volume<=0.0){return v;}
    let momentum=0.5*(f32(atomicLoad(&falls[a*3u+component]))+f32(atomicLoad(&falls[b*3u+component])))/FALL_SCALE;
    let depth=0.5*(scale[a]+scale[b]);
    if(depth<=1e-4){return v;}
    return (v*max(depth-volume,0.0)+momentum)/depth;
}
