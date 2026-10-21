// Sparse queries on the resident field. Positions are local grid coordinates: the first column's
// centre is (0.5, 0.5). Outside positions clamp to the region's nearest column centre.
struct Params { size: vec2u, count: u32, body_start: u32 }
@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var<storage, read> state: array<vec4f>;
@group(0) @binding(2) var<storage, read> points: array<vec2f>;
@group(0) @binding(3) var<storage, read_write> result: array<vec4f>;

fn column(ij: vec2u) -> vec4f {
    let n = ij.x + p.size.x * ij.y;
    let c = state[n];
    let east=ij.y;let north=p.size.y+ij.x;let base=p.size.x*p.size.y;
    var right = state[base+east/4u][east%4u]; var far = state[base+north/4u][north%4u];
    if (ij.x + 1u < p.size.x) { right = state[n+1u].z; }
    if (ij.y + 1u < p.size.y) { far = state[n+p.size.x].w; }
    return vec4f(c.x+c.y, c.y, (c.z+right)*0.5, (c.w+far)*0.5);
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
    if(id.x>=p.body_start){let velocity=coupled_velocity(points[id.x]);value.z=velocity.x;value.w=velocity.y;}
    result[id.x]=value;
}
