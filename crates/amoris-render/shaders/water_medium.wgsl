// Camera-side transport through the same piecewise-linear field that draws the interface.
// Classification uses the camera/near-ray prism; whole-field bounds trim active rays. Indirect
// zero dispatch avoids image work above water. DDA integrates top/bottom/wet intervals.
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
// min bed, max bed, min surface, max surface. Also certifies an exactly planar prism.
var<workgroup> bounds: array<vec4f,64>;

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
    if(abs(view.proj[3].w)<0.5){enabled=medium_inside(view.camera_pos.xyz);}
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
fn medium_inside(p:vec3f) -> bool {
    let g=(p.xz-medium_min())/medium_step();let count=medium_cells();
    if(any(g<vec2f(0.0)) || any(g>vec2f(count))){return false;}
    let at=min(vec2i(floor(g)),vec2i(count)-1);
    var h=triangle_value(corners(at),g-vec2f(at));
    if(ledge_draw.y!=0u && shallow.surround.w<0.5 && textureLoad(ledge_mask,at,0).x!=0u){let q=ledge_quad(at);if(q.enabled!=0u){h=ledge_value(ledge_containing(q,g-vec2f(at)),g-vec2f(at)).xy;}}
    return h.y>0.002 && p.y<=h.x+WATER_VIEW_EPSILON && p.y>=h.x-h.y;
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
    var interval=positive_interval(h0.x-y0,h1.x-y1,vec2f(0.0,1.0));
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
        var interval=positive_interval(h0.x-y0,h1.x-y1,vec2f(0.0,1.0));interval=positive_interval(y0-h0.x+h0.y,y1-h1.x+h1.y,interval);interval=positive_interval(h0.y-0.002,h1.y-0.002,interval);
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
    let vertical=vec4f(bitcast<f32>(region_bounds[4]),bitcast<f32>(region_bounds[5]),bitcast<f32>(region_bounds[6]),bitcast<f32>(region_bounds[7]));
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
fn medium_light() -> vec3f {
    let irradiance=max(vec3f(0.0),-0.429043*sh[8].xyz+0.886227*sh[0].xyz-0.247708*sh[6].xyz+1.023328*sh[1].xyz);
    return irradiance*0.08+view.sun_dir.w*view.sun_color.rgb*max(view.sun_dir.y,0.0)*0.012;
}
@compute @workgroup_size(8,8)
fn integrate(@builtin(global_invocation_id) id:vec3u){
    let size=textureDimensions(source_color);let pixel=vec2u(id.x,id.y+tile.y);
    if(pixel.x>=size.x || id.y>=tile.rows || pixel.y>=size.y){return;}
    let color=textureLoad(source_color,vec2i(pixel),0);
    let index=id.x+size.x*id.y;
    let uv=(vec2f(pixel)+0.5)/vec2f(size);let near=medium_world(uv,1.0);
    let origin=medium_origin(near);let dir=normalize(near-origin);
    if(owners[index]==tile.stamp || !medium_inside(origin)){textureStore(result,vec2i(pixel),color);return;}
    var z=0.0;for(var s=0u;s<textureNumSamples(depth);s++){z=max(z,textureLoad(depth,vec2i(pixel),i32(s)));}
    var distance=1e6;
    if(z>0.0){distance=max(0.0,dot(medium_world(uv,z)-origin,dir));}
    let haze=max(shallow.fog.w,0.0);let sigma=shallow.absorb.rgb+shallow.scatter.rgb+vec3f(haze);
    let least=min(sigma.x,min(sigma.y,sigma.z));let cutoff=select(1e30,16.0/max(least,1e-30),least>0.0);
    let path=medium_length(origin,dir,distance,cutoff);
    let trans=exp(-sigma*path);
    let source=shallow.scatter.rgb*medium_light()+shallow.fog.rgb*haze*view.sun_color.w;
    let radiance=color.rgb*trans+source/max(sigma,vec3f(1e-6))*(1.0-trans);
    textureStore(result,vec2i(pixel),vec4f(radiance,color.a));owners[index]=tile.stamp;
}
