// Shared surface geometry and material layout for raster water and GPU medium integration.
struct Shallow {
    origin: vec2f,        // the grid's corner, world x and z
    cell: f32,            // metres per column
    time: f32,
    count: vec4u,         // x, y: columns along x and z; z: 1 when the columns are in tiles of
                          // 16 x 16 (else rows); w: columns to a block (Blocky)
    absorb: vec4f,        // absorption per metre, rgb
    scatter: vec4f,       // scattering per metre, rgb
    surround: vec4f,      // the water drawn around: its level, its reach, its inset (metres)
    fog: vec4f,          // extra isotropic scattering color/radiance and density per metre
    optics: vec4f,        // refraction, local reflection, subgrid detail, unused
    ripple: vec4f,        // the ripples' window (water.md 57): its corner (world x, z), side, 1 on
    sea: vec4f,           // per side (-x, +x, -z, +z) open on the sea: the band (m) where the
                          // water turns to the sea's (water.md 60), else 0
    joined: vec4f,        // per side, 1 when joined to another region (water.md 64.3)
    rain: vec4f,          // the rain (water.md 67): drops a square metre gets a second, their
                          // diameter (m); zero without rain
};

@group(3) @binding(1) var<uniform> shallow: Shallow;
@group(3) @binding(2) var<storage, read> columns: array<vec4f>;   // surface (world y), depth, vx, vz
const SHALLOW_WET: f32 = 0.01;
// Reject roundoff-sized camera-plane interfaces; the medium includes this filtered boundary.
const WATER_VIEW_EPSILON: f32 = 1e-5;

fn shallow_column(i: i32, k: i32) -> vec4f {
    let n = vec2i(shallow.count.xy);
    let c = vec2u(clamp(vec2i(i, k), vec2i(0), n - 1));
    if (shallow.count.z == 1u) {
        let tile = c / 16u;
        let local = c % 16u;
        return columns[(tile.x + (shallow.count.x / 16u) * tile.y) * 256u + local.x + 16u * local.y];
    }
    return columns[c.x + shallow.count.x * c.y];
}

fn shallow_vertex(i: i32, k: i32) -> vec4f {
    let c = shallow_column(i, k);
    if (c.y > SHALLOW_WET) {
        return c;
    }
    var sum = 0.0;
    var n = 0.0;
    for (var dk = -1; dk <= 1; dk++) {
        for (var di = -1; di <= 1; di++) {
            let q = shallow_column(i + di, k + dk);
            if (q.y > SHALLOW_WET) {
                sum += q.x;
                n += 1.0;
            }
        }
    }
    if (n > 0.0) {
        return vec4f(sum / n, 0.0, 0.0, 0.0);
    }
    return vec4f(c.x - 0.05, 0.0, 0.0, 0.0);
}


fn shallow_block(bi: i32, bk: i32) -> vec4f {
    let per = i32(max(shallow.count.w, 1u));
    let nb = vec2i(shallow.count.xy) / per;
    if (bi < 0 || bk < 0 || bi >= nb.x || bk >= nb.y) {
        return vec4f(-1e9, 0.0, 0.0, 0.0);
    }
    var wet = 0.0;
    var n_wet = 0.0;
    var beds = 0.0;
    var depth = 0.0;
    var vel = vec2f(0.0);
    for (var dk = 0; dk < per; dk++) {
        for (var di = 0; di < per; di++) {
            let c = shallow_column(bi * per + di, bk * per + dk);
            beds += c.x;
            depth += c.y;
            vel += c.zw;
            if (c.y > SHALLOW_WET) {
                wet += c.x;
                n_wet += 1.0;
            }
        }
    }
    let n = f32(per * per);
    let level = select(beds / n, wet / max(n_wet, 1.0), n_wet > 0.0);
    return vec4f(level, depth / n, vel / n);
}


// Water interfaces remain visible within the ordinary near plane, even in triangles that
// cross the eye. Side/eye clipping still applies; fragment depth restores reversed-Z ordering.
// Moving only positive-w vertices to the near plane incorrectly clips crossing triangles.
fn shallow_clip(world: vec3f) -> vec4f {
    var p = view.view_proj * vec4f(world,1.0);
    p.z = 0.0;
    return p;
}

// Vertex clipping changes interpolated raster depth near the camera. Restore the interface's
// actual projected depth per fragment, clamped to the near plane, so the floor cannot win there.
fn shallow_depth(world: vec3f) -> f32 {
    let p = view.view_proj * vec4f(world,1.0);
    return clamp(p.z/p.w,0.0,1.0-1e-6);
}

// A perspective camera's near plane is its port (water.md 58): a surface between the eye and that
// plane is behind the port, not an interface it sees. The medium pass classifies each pixel by
// where its point on the near plane lies, so a camera at the surface splits along the waterline
// on its port, as a half-submerged camera does.
fn water_behind_port(world: vec3f) -> bool {
    if (abs(view.proj[3].w) >= 0.5) { return false; }
    let p = view.view_proj * vec4f(world,1.0);
    return p.w > 0.0 && p.z >= p.w;
}

// Bed/surface discontinuities share one cap partition for raster and medium transport.
@group(3) @binding(10) var<uniform> ledge_draw:vec4u;
@group(3) @binding(11) var ledge_mask:texture_2d<u32>;
struct LedgeQuad { values:mat4x4f, centres:mat4x4f, cuts:u32, enabled:u32 }
struct LedgePoint { grid:vec2f, value:vec4f }
struct LedgeTriangle { a:LedgePoint, b:LedgePoint, c:LedgePoint }
fn ledge_corner(n:u32)->vec2f {return array<vec2f,4>(vec2f(0.0,0.0),vec2f(1.0,0.0),vec2f(1.0,1.0),vec2f(0.0,1.0))[n];}
fn ledge_valid(c:vec4f)->bool {return c.y>=0.0&&abs(c.x)<1e10;}
fn ledge_bed(at:vec2i)->f32 {let c=shallow_column(at.x,at.y);return c.x-c.y;}
fn ledge_edge(ai:vec2i,bi:vec2i)->bool {
    let count=vec2i(shallow.count.xy);if(any(ai<vec2i(0))||any(bi<vec2i(0))||any(ai>=count)||any(bi>=count)){return false;}
    let a=shallow_column(ai.x,ai.y);let b=shallow_column(bi.x,bi.y);if(!ledge_valid(a)||!ledge_valid(b)){return false;}
    let bed=(b.x-b.y)-(a.x-a.y);let surface=b.x-a.x;
    if(abs(bed)<=0.5||abs(surface)<=0.5||bed*surface<=0.0){return false;}
    let direction=bi-ai;let before=ai-direction;let after=bi+direction;
    let has_before=all(before>=vec2i(0))&&all(before<count)&&ledge_valid(shallow_column(before.x,before.y));
    let has_after=all(after>=vec2i(0))&&all(after<count)&&ledge_valid(shallow_column(after.x,after.y));
    let left=(a.x-a.y)-ledge_bed(before);let right=ledge_bed(after)-(b.x-b.y);var slope=0.0;
    if(has_before&&has_after){if(left*right>0.0){slope=sign(left)*min(abs(left),abs(right));}}
    else if(has_before){slope=left;}else if(has_after){slope=right;}
    // Remove the resolvable local ramp: a steep continuous plane is not a terrain break.
    return abs(bed-slope)>0.5;
}
fn ledge_cuts(at:vec2i)->u32 {
    if(any(at<vec2i(0))||any(at>=vec2i(shallow.count.xy)-vec2i(1))){return 0u;}
    return textureLoad(ledge_mask,at,0).x;
}
fn shallow_normal(at:vec2i)->vec3f {
    let c=shallow_column(at.x,at.y);
    var hl=shallow_vertex(at.x-1,at.y).x;var hr=shallow_vertex(at.x+1,at.y).x;var hd=shallow_vertex(at.x,at.y-1).x;var hu=shallow_vertex(at.x,at.y+1).x;
    var x=hr-hl;var z=hu-hd;
    if(ledge_draw.y!=0u){
        let a=ledge_cuts(at);let b=ledge_cuts(at-vec2i(1,0));let d=ledge_cuts(at-vec2i(0,1));let e=ledge_cuts(at-vec2i(1,1));
        let cl=((b & 1u) | (e & 4u))!=0u;let cr=((a & 1u) | (d & 4u))!=0u;
        let cd=((d & 8u) | (e & 2u))!=0u;let cu=((a & 8u) | (b & 2u))!=0u;
        if(cl||cr){hl=select(hl,c.x,cl);hr=select(hr,c.x,cr);x=select(0.0,(hr-hl)*2.0,!(cl&&cr));}
        if(cd||cu){hd=select(hd,c.x,cd);hu=select(hu,c.x,cu);z=select(0.0,(hu-hd)*2.0,!(cd&&cu));}
    }
    return vec3f(-x,2.0*shallow.cell,-z);
}
fn ledge_quad(at:vec2i)->LedgeQuad {
    let values=mat4x4f(shallow_column(at.x,at.y),shallow_column(at.x+1,at.y),shallow_column(at.x+1,at.y+1),shallow_column(at.x,at.y+1));
    var q:LedgeQuad;q.values=values;q.cuts=0u;q.enabled=0u;var connected=vec4u(1u,2u,4u,8u);
    for(var n=0u;n<4u;n++){
        let a=values[n];let b=values[(n+1u)%4u];
        if(a.y<0.0||b.y<0.0||abs(a.x)>1e10||abs(b.x)>1e10){return q;}
        if(ledge_edge(at+vec2i(ledge_corner(n)),at+vec2i(ledge_corner((n+1u)%4u)))){q.cuts=q.cuts | (1u<<n);}
    }
    if(q.cuts==0u){return q;}
    for(var iteration=0u;iteration<3u;iteration++){for(var n=0u;n<4u;n++){if((q.cuts & (1u<<n))==0u){let next=(n+1u)%4u;let both=connected[n] | connected[next];connected[n]=both;connected[next]=both;}}}
    for(var n=0u;n<4u;n++){
        var value=vec4f(0.0);var count=0.0;
        for(var k=0u;k<4u;k++){if((connected[n] & (1u<<k))!=0u){value+=values[k];count+=1.0;}}
        q.centres[n]=value/count;
        q.enabled=max(q.enabled,u32(values[n].y>0.002));
    }
    return q;
}
fn ledge_point(q:LedgeQuad,corner:u32,point:u32)->LedgePoint {
    let me=q.values[corner];let next=(corner+1u)%4u;let prev=(corner+3u)%4u;
    if(point==0u){return LedgePoint(ledge_corner(corner),me);}
    if(point==2u){return LedgePoint(vec2f(0.5),q.centres[corner]);}
    let other=select(prev,next,point==1u);let edge=select(prev,corner,point==1u);
    return LedgePoint((ledge_corner(corner)+ledge_corner(other))*0.5,select((me+q.values[other])*0.5,me,(q.cuts & (1u<<edge))!=0u));
}
fn ledge_triangle(q:LedgeQuad,corner:u32,triangle:u32)->LedgeTriangle {
    return LedgeTriangle(ledge_point(q,corner,0u),ledge_point(q,corner,select(2u,1u,triangle==0u)),ledge_point(q,corner,select(3u,2u,triangle==0u)));
}
fn ledge_weights(t:LedgeTriangle,at:vec2f)->vec3f {
    let a=t.b.grid-t.a.grid;let b=t.c.grid-t.a.grid;let p=at-t.a.grid;let det=a.x*b.y-a.y*b.x;
    let v=(p.x*b.y-p.y*b.x)/det;let w=(a.x*p.y-a.y*p.x)/det;return vec3f(1.0-v-w,v,w);
}
fn ledge_value(t:LedgeTriangle,at:vec2f)->vec4f {let w=ledge_weights(t,at);return t.a.value*w.x+t.b.value*w.y+t.c.value*w.z;}
fn ledge_containing(q:LedgeQuad,at:vec2f)->LedgeTriangle {
    var corner=0u;if(at.x>=0.5){corner=1u;}if(at.y>=0.5){corner=select(3u,2u,at.x>=0.5);}
    let first=ledge_triangle(q,corner,0u);if(all(ledge_weights(first,at)>=vec3f(-1e-6))){return first;}return ledge_triangle(q,corner,1u);
}
fn ledge_at(world:vec2f)->bool {
    if(ledge_draw.y==0u||shallow.surround.w>0.5){return false;}
    let grid=(world-shallow.origin)/shallow.cell-0.5;
    if(any(grid<vec2f(0.0))||any(grid>=vec2f(shallow.count.xy-vec2u(1u)))){return false;}
    return textureLoad(ledge_mask,vec2i(floor(grid)),0).x!=0u;
}
fn ledge_depth(world:vec3f)->f32 {let z=shallow_depth(world);return min(z*(1.0+2e-5)+1e-8,1.0-1e-6);}
