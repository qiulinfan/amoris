// Bodies' volume under the free surface, gathered per column as metres of head. Each point
// spreads over a smooth disk of its footprint radius (at least one column): weights
// (1 - d^2/R^2)^2 at the covered column centres, normalized so the units it adds sum to its
// volume. Integer sums keep the result independent of scatter order, so a replay on one device
// reproduces it bit for bit.
// Displacement coupling of rigid bodies to a height field follows the idea in M. Mueller,
// "Fast Water Simulation for Games Using Height Fields", GDC 2008: a body's submerged volume
// pushes the water column aside. Here it enters as a pressure head on the faces instead of an
// edit of the depth, so water volume itself is never created or destroyed. The footprint must
// cover the body's waterplane: a head piled on fewer columns would rise faster than the body
// sinks and pump its bobbing.
struct Params { size:vec2u, count:u32, area:f32 }
@group(0) @binding(0) var<uniform> p:Params;
@group(0) @binding(1) var<storage,read> points:array<vec4f>;
@group(0) @binding(2) var<storage,read_write> accum:array<atomic<i32>>;
@group(0) @binding(3) var<storage,read_write> occupancy:array<f32>;
// 2^14 units a cubic metre: 61 cm^3 resolution, 131,000 m^3 a column before overflow.
const OCC_SCALE:f32=16384.0;

@compute @workgroup_size(64)
fn occupancy_clear(@builtin(global_invocation_id) id:vec3u){
    if(id.x>=p.size.x*p.size.y){return;}
    atomicStore(&accum[id.x],0);
}
fn footprint(at:vec2f,r:f32,c:vec2i)->f32{
    let d=(vec2f(c)+0.5-at)/r;
    let s=max(1.0-dot(d,d),0.0);
    return s*s;
}
@compute @workgroup_size(64)
fn occupancy_scatter(@builtin(global_invocation_id) id:vec3u){
    if(id.x>=p.count){return;}
    let point=points[id.x];
    let r=clamp(point.w,1.0,8.0);
    let last=vec2i(p.size)-vec2i(1);
    let lo=max(vec2i(floor(point.xy-0.5-r)),vec2i(0));
    let hi=min(vec2i(ceil(point.xy-0.5+r)),last);
    var total=0.0;
    for(var j=lo.y;j<=hi.y;j++){
        for(var i=lo.x;i<=hi.x;i++){total+=footprint(point.xy,r,vec2i(i,j));}
    }
    if(total<=0.0){
        let c=clamp(vec2i(floor(point.xy)),vec2i(0),last);
        atomicAdd(&accum[u32(c.x)+p.size.x*u32(c.y)],i32(round(point.z*OCC_SCALE)));
        return;
    }
    for(var j=lo.y;j<=hi.y;j++){
        for(var i=lo.x;i<=hi.x;i++){
            let units=i32(round(point.z*footprint(point.xy,r,vec2i(i,j))/total*OCC_SCALE));
            if(units!=0){atomicAdd(&accum[u32(i)+p.size.x*u32(j)],units);}
        }
    }
}
@compute @workgroup_size(64)
fn occupancy_resolve(@builtin(global_invocation_id) id:vec3u){
    if(id.x>=p.size.x*p.size.y){return;}
    let head=f32(atomicLoad(&accum[id.x]))/OCC_SCALE/p.area;
    occupancy[id.x]=head;
}
