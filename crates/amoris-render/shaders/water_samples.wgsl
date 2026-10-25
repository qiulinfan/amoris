// Sparse queries on the resident field. Positions are local grid coordinates: the first column's
// centre is (0.5, 0.5). Outside positions clamp to the region's nearest column centre.
// Each point returns two vec4: [free surface, depth, centred vx, vz] and [surface rise per
// column along x, z, net inflow m^2/s, 0]. The host divides by cell_m for slope and rate of
// rise. The free surface includes bodies' displaced head.
struct Params { size: vec2u, count: u32, body_start: u32 }
@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var<storage, read> state: array<vec4f>;
@group(0) @binding(2) var<storage, read> points: array<vec2f>;
@group(0) @binding(3) var<storage, read_write> result: array<vec4f>;
@group(0) @binding(4) var<storage, read> occupancy: array<f32>;

fn far_u(ij:vec2u)->f32 {
    let base=p.size.x*p.size.y;
    if (ij.x + 1u < p.size.x) { return state[ij.x+1u+p.size.x*ij.y].z; }
    return state[base+ij.y/4u][ij.y%4u];
}
fn far_w(ij:vec2u)->f32 {
    let base=p.size.x*p.size.y;let north=p.size.y+ij.x;
    if (ij.y + 1u < p.size.y) { return state[ij.x+p.size.x*(ij.y+1u)].w; }
    return state[base+north/4u][north%4u];
}
fn head(n:u32)->f32 { return select(0.0, occupancy[n], state[n].y > 0.0); }
// Depth reaches from the bed to the free surface, so surface minus depth is the true bed even
// where a body's displaced head stands on the water.
fn column(ij: vec2u) -> vec4f {
    let n = ij.x + p.size.x * ij.y;
    let c = state[n];
    let h = head(n);
    return vec4f(c.x+c.y+h, c.y+h, (c.z+far_u(ij))*0.5, (c.w+far_w(ij))*0.5);
}
// Continuity at the column: net inflow of centred-depth fluxes, m^2/s.
fn rise(ij: vec2u) -> f32 {
    let n = ij.x + p.size.x * ij.y; let h = state[n].y;
    var q = 0.0;
    if (ij.x > 0u) { q += state[n].z * 0.5 * (h + state[n-1u].y); }
    if (ij.x + 1u < p.size.x) { q -= far_u(ij) * 0.5 * (h + state[n+1u].y); }
    if (ij.y > 0u) { q += state[n].w * 0.5 * (h + state[n-p.size.x].y); }
    if (ij.y + 1u < p.size.y) { q -= far_w(ij) * 0.5 * (h + state[n+p.size.x].y); }
    return q;
}
fn detail(ij: vec2u) -> vec4f {
    let lo = vec2u(select(ij.x, ij.x-1u, ij.x>0u), select(ij.y, ij.y-1u, ij.y>0u));
    let hi = min(ij+vec2u(1u), p.size-vec2u(1u));
    let here = column(ij);
    var sx = 0.0; var sz = 0.0;
    // One-sided across a dry neighbour, so a shoreline does not read the dry bed as a slope.
    let l = column(vec2u(lo.x, ij.y)); let r = column(vec2u(hi.x, ij.y));
    let d = column(vec2u(ij.x, lo.y)); let u = column(vec2u(ij.x, hi.y));
    let wl = select(here.x, l.x, l.y > 0.0); let wr = select(here.x, r.x, r.y > 0.0);
    let wd = select(here.x, d.x, d.y > 0.0); let wu = select(here.x, u.x, u.y > 0.0);
    let span_x = max(f32(hi.x - lo.x), 1.0); let span_z = max(f32(hi.y - lo.y), 1.0);
    if (here.y > 0.0) { sx = (wr - wl) / span_x; sz = (wu - wd) / span_z; }
    return vec4f(sx, sz, rise(ij), 0.0);
}
fn face_wet(face:u32,axis:u32)->bool {
    let ij=vec2u(face%p.size.x,face/p.size.x);
    if(ij[axis]==0u){return false;}
    return (state[face].y+state[face-select(1u,p.size.x,axis==1u)].y)*0.5>0.0;
}
fn coupled_velocity(point:vec2f)->vec2f {
    var value=vec2f(0.0);
    for(var c=0u;c<4u;c++){
        let v=water_point_corner(point,c,p.size);let ij=vec2u(v.xy);let here=ij.x+p.size.x*ij.y;
        for(var axis=0u;axis<2u;axis++){
            if(face_wet(here,axis)){value[axis]+=state[here][axis+2u]*v.z;}
            else if(ij[axis]==0u && state[here].y>0.0){value[axis]+=state[here][axis+2u]*v.z;}
            if(ij[axis]+1u<p.size[axis]){
                let next=here+select(1u,p.size.x,axis==1u);
                if(face_wet(next,axis)){value[axis]+=state[next][axis+2u]*v.z;}
            }else if(state[here].y>0.0){
                let face=select(ij.y,p.size.y+ij.x,axis==1u);
                value[axis]+=state[p.size.x*p.size.y+face/4u][face%4u]*v.z;
            }
        }
    }
    return value;
}
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3u) {
    if (id.x >= p.count) { return; }
    let q = clamp(points[id.x] - 0.5, vec2f(0.0), vec2f(p.size-1u));
    let lo = vec2u(floor(q));
    let hi = min(lo+1u,p.size-1u);
    let t = fract(q);
    var value = mix(mix(column(lo), column(vec2u(hi.x,lo.y)), t.x),
                    mix(column(vec2u(lo.x,hi.y)), column(hi), t.x), t.y);
    let more = mix(mix(detail(lo), detail(vec2u(hi.x,lo.y)), t.x),
                   mix(detail(vec2u(lo.x,hi.y)), detail(hi), t.x), t.y);
    if(id.x>=p.body_start){let velocity=coupled_velocity(points[id.x]);value.z=velocity.x;value.w=velocity.y;}
    result[id.x*2u]=value;
    result[id.x*2u+1u]=more;
}
