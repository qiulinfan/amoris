// Camera-side transport through the same piecewise-linear field that draws the interface.
// Classification uses the camera/near-ray prism; whole-field bounds trim active rays. Indirect
// zero dispatch avoids image work above water. DDA integrates top/bottom/wet intervals. A
// perspective pixel's water starts at its point on the near plane, the camera's port, where the
// rippled surface draws the waterline; the sun scatters toward the eye along the path (58).
struct MediumTile { y: u32, rows: u32, stamp: u32, padding: u32 };
@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var<uniform> sh: array<vec4f,9>;
@group(0) @binding(2) var source_color: texture_2d<f32>;
@group(0) @binding(3) var depth: texture_depth_multisampled_2d;
@group(0) @binding(4) var result: texture_storage_2d<rgba16float,write>;
@group(0) @binding(5) var<storage,read_write> owners: array<u32>;
@group(0) @binding(6) var<storage,read_write> dispatch: array<u32>;
@group(0) @binding(7) var<uniform> tile: MediumTile;
@group(0) @binding(8) var<storage,read> region_bounds: array<u32>;
// The lighting the shafts need, numbered as the forward pass's group (shadow cascades, the ripples'
// rise; the caustics map at 15 and 16 comes with water_caustics_light.wgsl).
@group(1) @binding(0) var shadow_map: texture_depth_2d_array;
@group(1) @binding(1) var shadow_sampler: sampler_comparison;
@group(1) @binding(12) var medium_ripples: texture_2d<f32>;
@group(1) @binding(13) var medium_ripple_sampler: sampler;
// min bed, max bed, min surface, max surface. Also certifies an exactly planar prism.
var<workgroup> bounds: array<vec4f,64>;
// The ripples' rise at the pixel's port point: the transport takes the surface that much higher
// along its ray, so the waterline and the water behind it agree.
var<private> medium_lift: f32;

// How far from the surface a corner of the near plane may lie for its port to meet the water:
// more than the ripples' rise and the surface's curvature across a near plane.
const PORT_REACH_M: f32 = 0.1;
// Strata of the sun's scattering along a path, shared by a 2 x 2 block of pixels with
// SHAFT_TAPS each, interleaved (A. Keller and W. Heidrich, "Interleaved sampling", Eurographics
// Workshop on Rendering 2001), and the longest path they cover.
const SHAFT_SAMPLES: u32 = 16u;
const SHAFT_TAPS: u32 = 4u;
const SHAFT_REACH_M: f32 = 64.0;
// Single scattering of the sun's beam: the phase function over the sphere's 4 pi steradians, as
// the sky's light already is (`medium_sky`'s 0.08). The isotropic stand-in before the shafts took
// 0.012 of the sun's irradiance on level water, a sixth of that.
const MEDIUM_SUN: f32 = 0.0795775;
// Henyey and Greenstein's asymmetry for the water's forward-peaked scattering.
const SHAFT_G: f32 = 0.8;

fn medium_world(uv: vec2f,z: f32) -> vec3f {
    let p = view.inv_view_proj * vec4f(uv*vec2f(2.0,-2.0)+vec2f(-1.0,1.0),z,1.0);
    return p.xyz / p.w;
}
fn medium_origin(near: vec3f) -> vec3f {
    if (abs(view.proj[3].w)<0.5) { return view.camera_pos.xyz; }
    let direction = normalize(-vec3f(view.view[0].z,view.view[1].z,view.view[2].z));
    return near-direction*dot(near-view.camera_pos.xyz,direction);
}
fn medium_min() -> vec2f { return shallow.origin + select(vec2f(0.5*shallow.cell),vec2f(0.0),shallow.surround.w>0.5); }
fn medium_cells() -> vec2u { return select(shallow.count.xy-vec2u(1u),shallow.count.xy/max(shallow.count.w,1u),shallow.surround.w>0.5); }
fn medium_step() -> f32 { return shallow.cell*select(1.0,f32(max(shallow.count.w,1u)),shallow.surround.w>0.5); }

@compute @workgroup_size(64)
fn classify(@builtin(local_invocation_index) lane: u32) {
    var lo=vec3f(1e30);var hi=vec3f(-1e30);
    for (var i=0u;i<4u;i++) {
        let near=medium_world(vec2f(f32(i%2u),f32(i/2u)),1.0);
        let origin=medium_origin(near);
        lo=min(lo,min(near,origin));hi=max(hi,max(near,origin));
    }
    let edge0=medium_min();let edge1=edge0+vec2f(medium_cells())*medium_step();
    let overlaps=all(hi.xz>=edge0)&&all(lo.xz<=edge1);
    var enabled=overlaps;
    if(abs(view.proj[3].w)<0.5){
        // The eye in the water, or a corner of the near plane close enough to the surface that
        // the port may straddle it (58).
        enabled=medium_inside(view.camera_pos.xyz);
        for(var i=0u;i<4u;i++){enabled=enabled||medium_near_surface(medium_world(vec2f(f32(i%2u),f32(i/2u)),1.0));}
    }
    // Every lane initializes the workgroup memory it reads. Above-water perspective views
    // skip the field scan as well as the image dispatches. Bounds include the complete field:
    // a ray can cross a distant higher/lower surface, outside the camera's near prism.
    var out=vec4f(1e30,-1e30,1e30,-1e30);
    if (enabled) {
        for(var n=lane;n<shallow.count.x*shallow.count.y;n+=64u) {
            let c=shallow_column(i32(n%shallow.count.x),i32(n/shallow.count.x));
            if(c.y<0.0||abs(c.x)>1e10){continue;}
            let bed=c.x-c.y;
            out=vec4f(min(out.x,bed),max(out.y,bed),min(out.z,c.x),max(out.w,c.x));
        }
    }
    bounds[lane]=out;workgroupBarrier();
    for(var stride=32u;stride>0u;stride/=2u) {
        if(lane<stride){
            let a=bounds[lane];let b=bounds[lane+stride];
            bounds[lane]=vec4f(min(a.x,b.x),max(a.y,b.y),min(a.z,b.z),max(a.w,b.w));
        }
        workgroupBarrier();
    }
    if(lane==0u){
        if(abs(view.proj[3].w)>=0.5){enabled=enabled && hi.y>=bounds[0].x && lo.y<=bounds[0].w;}
        dispatch[0]=select(0u,(u32(view.viewport.x)+7u)/8u,enabled);
        dispatch[1]=select(0u,(tile.rows+7u)/8u,enabled);
        dispatch[2]=1u;dispatch[3]=0u;
        dispatch[4]=bitcast<u32>(bounds[0].x);dispatch[5]=bitcast<u32>(bounds[0].y);
        dispatch[6]=bitcast<u32>(bounds[0].z);dispatch[7]=bitcast<u32>(bounds[0].w);
    }
}

struct Corners { a: vec2f, b: vec2f, c: vec2f, d: vec2f };
fn corners(at: vec2i) -> Corners {
    if(shallow.surround.w>0.5){let c=shallow_block(at.x,at.y).xy;return Corners(c,c,c,c);}
    return Corners(shallow_vertex(at.x,at.y).xy,shallow_vertex(at.x,at.y+1).xy,
                   shallow_vertex(at.x+1,at.y+1).xy,shallow_vertex(at.x+1,at.y).xy);
}
fn triangle_value(c: Corners,p: vec2f) -> vec2f {
    let q=clamp(p,vec2f(0.0),vec2f(1.0));
    if(q.y>=q.x){return c.a*(1.0-q.y)+c.b*(q.y-q.x)+c.c*q.x;}
    return c.a*(1.0-q.x)+c.c*q.y+c.d*(q.x-q.y);
}
// The drawn surface over a world point: its height and the water's depth there (the raster's
// triangles, or a ledge's caps where it has them); depth 0 outside the field.
fn medium_column(p:vec2f) -> vec2f {
    let g=(p-medium_min())/medium_step();let count=medium_cells();
    if(any(g<vec2f(0.0)) || any(g>vec2f(count))){return vec2f(0.0);}
    let at=min(vec2i(floor(g)),vec2i(count)-1);
    var h=triangle_value(corners(at),g-vec2f(at));
    if(ledge_draw.y!=0u && shallow.surround.w<0.5 && textureLoad(ledge_mask,at,0).x!=0u){let q=ledge_quad(at);if(q.enabled!=0u){h=ledge_value(ledge_containing(q,g-vec2f(at)),g-vec2f(at)).xy;}}
    return h;
}
fn medium_inside(p:vec3f) -> bool {
    let h=medium_column(p.xz);
    return h.y>0.002 && p.y<=h.x+WATER_VIEW_EPSILON && p.y>=h.x-h.y;
}
fn medium_near_surface(p:vec3f) -> bool {
    let h=medium_column(p.xz);
    return h.y>0.002 && p.y<=h.x+PORT_REACH_M && p.y>=h.x-h.y-PORT_REACH_M;
}
// The ripples' rise at a world point (water.md 57), faded toward their window's edge as the
// surface shading fades them; 0 where there are none.
fn medium_rise(p:vec2f) -> f32 {
    if(shallow.ripple.w<0.5){return 0.0;}
    let local=(p-shallow.ripple.xy)/shallow.ripple.z;
    if(any(local<vec2f(0.0)) || any(local>=vec2f(1.0))){return 0.0;}
    let edge=min(min(local.x,local.y),min(1.0-local.x,1.0-local.y));
    return textureSampleLevel(medium_ripples,medium_ripple_sampler,p/shallow.ripple.z,0.0).x*smoothstep(0.0,0.0625,edge);
}
fn positive_interval(a:f32,b:f32,interval:vec2f) -> vec2f {
    if(a>=0.0 && b>=0.0){return interval;}
    if(a<0.0 && b<0.0){return vec2f(1.0,0.0);}
    let crossing=clamp(a/(a-b),0.0,1.0);
    return select(vec2f(interval.x,min(interval.y,crossing)),vec2f(max(interval.x,crossing),interval.y),a<0.0);
}
fn segment_length(c:Corners,cell:vec2i,go:vec2f,gd:vec2f,origin:vec3f,dir:vec3f,a:f32,b:f32) -> f32 {
    if(b<=a){return 0.0;}
    let h0=triangle_value(c,go+gd*a-vec2f(cell));let h1=triangle_value(c,go+gd*b-vec2f(cell));
    let y0=origin.y+dir.y*a;let y1=origin.y+dir.y*b;
    var interval=positive_interval(h0.x+medium_lift-y0,h1.x+medium_lift-y1,vec2f(0.0,1.0));
    interval=positive_interval(y0-h0.x+h0.y,y1-h1.x+h1.y,interval);
    interval=positive_interval(h0.y-0.002,h1.y-0.002,interval);
    return max(0.0,interval.y-interval.x)*(b-a);
}
fn ledge_segment(q:LedgeQuad,cell:vec2i,go:vec2f,gd:vec2f,origin:vec3f,dir:vec3f,begin:f32,end:f32)->f32 {
    // Four partition lines split the regular half-cell caps and both possible diagonals.
    let base=go-vec2f(cell);var cursor=begin;var total=0.0;
    for(var piece=0u;piece<5u;piece++){
        var next_t=end;let offsets=vec4f(base.x-0.5,base.y-0.5,base.x-base.y,base.x+base.y-1.0);let slopes=vec4f(gd.x,gd.y,gd.x-gd.y,gd.x+gd.y);
        for(var line=0u;line<4u;line++){if(abs(slopes[line])>1e-9){let crossing=-offsets[line]/slopes[line];if(crossing>cursor+1e-6){next_t=min(next_t,crossing);}}}
        let triangle=ledge_containing(q,base+gd*((cursor+next_t)*0.5));
        let h0=ledge_value(triangle,base+gd*cursor).xy;let h1=ledge_value(triangle,base+gd*next_t).xy;
        let y0=origin.y+dir.y*cursor;let y1=origin.y+dir.y*next_t;
        var interval=positive_interval(h0.x+medium_lift-y0,h1.x+medium_lift-y1,vec2f(0.0,1.0));interval=positive_interval(y0-h0.x+h0.y,y1-h1.x+h1.y,interval);interval=positive_interval(h0.y-0.002,h1.y-0.002,interval);
        total+=max(0.0,interval.y-interval.x)*(next_t-cursor);if(next_t>=end){break;}cursor=next_t;
    }
    return total;
}
fn medium_length(origin:vec3f,dir:vec3f,limit:f32,cutoff:f32) -> f32 {
    let step=medium_step();let count=medium_cells();
    let go=(origin.xz-medium_min())/step;let gd=dir.xz/step;
    var enter=0.0;var exit=limit;
    for(var axis=0u;axis<2u;axis++){
        if(abs(gd[axis])<1e-9){if(go[axis]<0.0 || go[axis]>f32(count[axis])){return 0.0;}}
        else{
            let t0=-go[axis]/gd[axis];let t1=(f32(count[axis])-go[axis])/gd[axis];
            enter=max(enter,min(t0,t1));exit=min(exit,max(t0,t1));
        }
    }
    let vertical=vec4f(bitcast<f32>(region_bounds[4]),bitcast<f32>(region_bounds[5]),
        bitcast<f32>(region_bounds[6])+min(medium_lift,0.0),bitcast<f32>(region_bounds[7])+max(medium_lift,0.0));
    if(abs(dir.y)>=1e-9){
        let t0=(vertical.x-origin.y)/dir.y;let t1=(vertical.w-origin.y)/dir.y;
        enter=max(enter,min(t0,t1));exit=min(exit,max(t0,t1));
    }else if(origin.y<vertical.x || origin.y>vertical.w){
        return 0.0;
    }
    if(exit<=enter){return 0.0;}
    // Exact constant bed/surface, not a tolerance-based approximation. All columns are wet
    // when this prism has nonzero depth; both smooth triangles and block geometry coincide.
    if(vertical.x==vertical.y && vertical.z==vertical.w){return min(cutoff,exit-enter);}
    var cell=vec2i(floor(clamp(go+gd*enter+sign(gd)*1e-5,vec2f(0.0),vec2f(count)-1e-5)));
    var total=0.0;var a=enter;
    for(var n=0u;n<count.x+count.y+1u;n++){
        let border=vec2f(cell)+select(vec2f(0.0),vec2f(1.0),gd>=vec2f(0.0));
        var next=vec2f(1e30);
        if(abs(gd.x)>=1e-9){next.x=(border.x-go.x)/gd.x;}
        if(abs(gd.y)>=1e-9){next.y=(border.y-go.y)/gd.y;}
        let b=min(exit,min(next.x,next.y));let c=corners(cell);
        let da=(go.x-go.y)+(gd.x-gd.y)*a-f32(cell.x-cell.y);
        let db=(go.x-go.y)+(gd.x-gd.y)*b-f32(cell.x-cell.y);
        var ledge=false;
        if(ledge_draw.y!=0u && shallow.surround.w<0.5 && textureLoad(ledge_mask,cell,0).x!=0u){let q=ledge_quad(cell);if(q.enabled!=0u){total+=ledge_segment(q,cell,go,gd,origin,dir,a,b);ledge=true;}}
        if(ledge){}else if(da*db<0.0 && shallow.surround.w<0.5){
            let middle=mix(a,b,da/(da-db));
            total+=segment_length(c,cell,go,gd,origin,dir,a,middle)+segment_length(c,cell,go,gd,origin,dir,middle,b);
        }else{total+=segment_length(c,cell,go,gd,origin,dir,a,b);}
        if(total>=cutoff){return cutoff;}
        if(b>=exit){break;}
        let crossing=min(next.x,next.y);
        if(next.x<=crossing+1e-6){cell.x+=select(-1,1,gd.x>=0.0);}
        if(next.y<=crossing+1e-6){cell.y+=select(-1,1,gd.y>=0.0);}
        if(any(cell<vec2i(0)) || any(cell>=vec2i(count))){break;}
        a=max(a,b);
    }
    return total;
}
// The sky's light in the water, an isotropic source: its irradiance from above, scaled as before.
fn medium_sky() -> vec3f {
    let irradiance=max(vec3f(0.0),-0.429043*sh[8].xyz+0.886227*sh[0].xyz-0.247708*sh[6].xyz+1.023328*sh[1].xyz);
    return irradiance*0.08;
}

// Henyey and Greenstein's phase function, times 4 pi so that it averages 1 over the sphere.
fn medium_phase(cosine: f32) -> f32 {
    let d=1.0+SHAFT_G*SHAFT_G-2.0*SHAFT_G*cosine;
    return (1.0-SHAFT_G*SHAFT_G)/(d*sqrt(d));
}
// Unpolarized dielectric reflectance of light entering water from air at `cosine` to the normal.
fn medium_fresnel(cosine: f32) -> f32 {
    let eta=0.75;let ct=sqrt(max(1.0-eta*eta*(1.0-cosine*cosine),0.0));
    let rs=(eta*cosine-ct)/(eta*cosine+ct);let rp=(cosine-eta*ct)/(cosine+eta*ct);
    return 0.5*(rs*rs+rp*rp);
}
// The sun's shadow for a point under water, asked where its refracted path leaves the water (or
// SUN_REACH_M up it), as the forward pass asks it (53.1): one comparison tap.
fn medium_shadow(world: vec3f) -> f32 {
    let view_depth=-(view.view*vec4f(world,1.0)).z;
    if(view.sky_color.w<0.5 || view_depth<0.0){return 1.0;}
    var c=0u;
    for(var i=0u;i<4u;i++){if(view_depth>view.cascade_splits[i]){c=i+1u;}}
    if(c>=4u){return 1.0;}
    let p=view.cascades[c]*vec4f(world,1.0);
    let uv=p.xy*vec2f(0.5,-0.5)+0.5;
    if(any(uv<vec2f(0.0)) || any(uv>vec2f(1.0)) || p.z>1.0){return 1.0;}
    return textureSampleCompareLevel(shadow_map,shadow_sampler,uv,c,p.z-0.0005);
}
// A map texel over a world point (53: x its focus on the bed, y the water's depth there, w the
// surface's height), or nothing (w far below anything) off the map or with caustics off.
fn medium_texel(p: vec2f) -> vec4f {
    let q=(p-caustics.origin)/caustics.texel-0.5;
    if(caustics.on==0.0 || any(q<vec2f(-0.5)) || any(q>vec2f(caustics.size)-0.5)){return vec4f(1.0,0.0,0.0,-1e30);}
    return caustic_texel(vec2i(round(q)));
}
// The waves' focus on the sun reaching a point `depth` metres under the surface, where the water
// is `column` deep, along the refracted path `up` (toward the sun). The caustics map holds each
// bed point's focus (53); the light through this point lands where its path meets the bed, and at
// this depth carries the share of that focus the depth gives, as the bed's lighting grows it. The
// focus therefore runs along the refracted sun: the waves' lenses draw shafts, not the columns
// over the bed. The nearest texel serves: the samples along a path blur it.
fn medium_focus(world: vec3f, depth: f32, column: f32, up: vec3f) -> f32 {
    let m=medium_texel(world.xz-up.xz*(max(column-depth,0.0)/up.y));
    if(m.w<-1e3){return 1.0;}
    return max(1.0+(m.x-1.0)*clamp(depth/max(m.y,0.05),0.0,1.0),0.0);
}
// The sun scattered toward the eye along a pixel's path through the water (58): the path is cut
// into SHAFT_SAMPLES strata, of which the pixel takes the SHAFT_TAPS numbered `block` modulo 4,
// the other three pixels of its 2 x 2 block the rest; the block pools its sums (`integrate`).
// Each sample is lit by the refracted sun as the forward pass lights the bed (its widening beam,
// the waves' focus, absorption on the way down and the shadow where its path leaves the water),
// weighted by the phase function and attenuated back to the eye along the water from the port.
// The light that comes through the moving surface and between the shadows draws the shafts.
fn medium_shafts(origin:vec3f,dir:vec3f,limit:f32,sigma:vec3f,pixel:vec2u,block:u32) -> vec3f {
    let l=view.sun_dir.xyz;
    if(view.sun_dir.w<=0.0 || l.y<=0.0 || all(shallow.scatter.rgb<=vec3f(0.0))){return vec3f(0.0);}
    let least=min(sigma.x,min(sigma.y,sigma.z));
    var span=min(limit,select(SHAFT_REACH_M,min(4.0/least,SHAFT_REACH_M),least>0.0));
    // The samples go where the path can be in water: up to the field's highest surface or down
    // to its lowest bed, whichever the ray meets (its region bounds, 21).
    if(abs(dir.y)>1e-6){
        let top=bitcast<f32>(region_bounds[7])+max(medium_lift,0.0);let bed=bitcast<f32>(region_bounds[4]);
        span=min(span,max((select(bed,top,dir.y>0.0)-origin.y)/dir.y,0.0));
    }
    if(span<=0.0){return vec3f(0.0);}
    // Toward the sun under flat water (Snell's law, air over water 1/1.333), and the beam's
    // irradiance over the sun's: bent toward the vertical, it widens (53.1).
    let up=vec3f(0.75*l.x,sqrt(1.0-0.5625*(1.0-l.y*l.y)),0.75*l.z);
    let beam=l.y/up.y;
    let step=span/f32(SHAFT_SAMPLES);
    let jitter=unit_from_hash(pcg_hash(pixel.x+pixel.y*8192u));
    var sum=vec3f(0.0);
    for(var i=0u;i<SHAFT_TAPS;i++){
        let t=(f32(i*4u+block)+jitter)*step;
        let x=origin+dir*t;
        // The surface and the water's depth over the sample: the caustics map's texel, which
        // holds both, or off the map the field's own triangles.
        var column=medium_texel(x.xz).wy;
        if(column.x<-1e3){column=medium_column(x.xz);}
        let surface=column.x+medium_lift;
        if(column.y<=0.002 || x.y>surface || x.y<column.x-column.y){continue;}
        let depth=surface-x.y;
        let exit=x+up*(min(depth,SUN_REACH_M)/up.y);
        sum+=exp(-sigma*(t+depth/up.y))*medium_focus(x,depth,column.y+medium_lift,up)*medium_shadow(exit);
    }
    return shallow.scatter.rgb*view.sun_color.rgb*view.sun_dir.w*MEDIUM_SUN*beam*(1.0-medium_fresnel(l.y))
        *medium_phase(dot(up,dir))*sum*(span/f32(SHAFT_TAPS));
}
// The waterline on the port (58): within a few pixels of where the rippled surface crosses the
// near plane, the film of water on the lens turns light away from it, a dark line with a faint
// bright edge on its water side. `d` is the port point's height over the surface.
fn medium_meniscus(radiance:vec3f,uv:vec2f,size:vec2u,near:vec3f,d:f32) -> vec3f {
    let width=max(1.5,0.004*f32(size.y));
    let right=medium_world(uv+vec2f(1.0/f32(size.x),0.0),1.0);
    let below=medium_world(uv+vec2f(0.0,1.0/f32(size.y)),1.0);
    // Far from the line in any orientation: no work.
    if(abs(d)>16.0*width*distance(right,near)){return radiance;}
    let cr=medium_column(right.xz);let cb=medium_column(below.xz);
    if(cr.y<=0.002 || cb.y<=0.002){return radiance;}
    let dr=right.y-cr.x-medium_rise(right.xz)-d;let db=below.y-cb.x-medium_rise(below.xz)-d;
    let s=d/max(length(vec2f(dr,db)),1e-9);
    let core=1.0-smoothstep(0.0,width,abs(s));
    let rim=select(0.0,smoothstep(0.5*width,width,abs(s))*(1.0-smoothstep(width,1.8*width,abs(s))),s<0.0);
    return radiance*(1.0-0.85*core)+medium_sky()*rim*0.5;
}
// A 2 x 2 block's shafts, pooled (`medium_shafts`): each pixel's sum, and its path's length to
// the scene (negative where the pixel draws none).
var<workgroup> shaft_pool: array<vec4f,64>;
@compute @workgroup_size(8,8)
fn integrate(@builtin(global_invocation_id) id:vec3u,@builtin(local_invocation_id) local:vec3u){
    let size=textureDimensions(source_color);let pixel=vec2u(id.x,id.y+tile.y);
    // Every invocation reaches the block's barrier; those past the image or already drawn by an
    // earlier region pool nothing.
    let present=pixel.x<size.x && id.y<tile.rows && pixel.y<size.y;
    let index=id.x+size.x*id.y;
    var color=vec4f(0.0);var radiance=vec3f(0.0);var shafts=vec4f(0.0,0.0,0.0,-1.0);
    var claimed=false;var meniscus=false;var over=0.0;
    let uv=(vec2f(pixel)+0.5)/vec2f(size);let near=medium_world(uv,1.0);
    let perspective=abs(view.proj[3].w)<0.5;
    if(present){
        color=textureLoad(source_color,vec2i(pixel),0);
        radiance=color.rgb;
        claimed=owners[index]==tile.stamp;
    }
    if(present && !claimed){
        let eye=medium_origin(near);let dir=normalize(near-eye);
        // A perspective pixel's water starts at its point on the near plane: the camera's port,
        // classified against the surface with the ripples' rise (58). Orthographic views start
        // at the camera plane.
        let origin=select(eye,near,perspective);
        medium_lift=select(0.0,medium_rise(near.xz),perspective);
        let column=medium_column(origin.xz);
        over=origin.y-column.x-medium_lift;
        meniscus=perspective && column.y>0.002;
        if(column.y>0.002 && over<=WATER_VIEW_EPSILON && origin.y>=column.x-column.y){
            var z=0.0;for(var s=0u;s<textureNumSamples(depth);s++){z=max(z,textureLoad(depth,vec2i(pixel),i32(s)));}
            var distance=1e6;
            if(z>0.0){distance=max(0.0,dot(medium_world(uv,z)-origin,dir));}
            let haze=max(shallow.fog.w,0.0);let sigma=shallow.absorb.rgb+shallow.scatter.rgb+vec3f(haze);
            let least=min(sigma.x,min(sigma.y,sigma.z));let cutoff=select(1e30,16.0/max(least,1e-30),least>0.0);
            let path=medium_length(origin,dir,distance,cutoff);
            let trans=exp(-sigma*path);
            let source=shallow.scatter.rgb*medium_sky()+shallow.fog.rgb*haze*view.sun_color.w;
            radiance=color.rgb*trans+source/max(sigma,vec3f(1e-6))*(1.0-trans);
            shafts=vec4f(medium_shafts(origin,dir,distance,sigma,pixel,(local.x%2u)+2u*(local.y%2u)),min(distance,1e5));
            owners[index]=tile.stamp;
        }
    }
    let lane=local.x+8u*local.y;
    shaft_pool[lane]=shafts;
    workgroupBarrier();
    if(!present){return;}
    if(claimed){textureStore(result,vec2i(pixel),color);return;}
    if(shafts.w>=0.0){
        // The block's pixels whose paths reach about as far share their strata; the others, across
        // an object's edge or the waterline, keep their own.
        let corner=(local.x/2u)*2u+8u*((local.y/2u)*2u);
        var total=vec3f(0.0);var n=0.0;
        for(var j=0u;j<4u;j++){
            let other=shaft_pool[corner+(j%2u)+8u*(j/2u)];
            if(other.w>=0.0 && abs(other.w-shafts.w)<=0.1*shafts.w+0.05){total+=other.rgb;n+=1.0;}
        }
        radiance+=total/max(n,1.0);
    }
    if(meniscus){radiance=medium_meniscus(radiance,uv,size,near,over);}
    textureStore(result,vec2i(pixel),vec4f(radiance,color.a));
}
