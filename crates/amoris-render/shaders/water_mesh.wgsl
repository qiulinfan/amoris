// Conservative current-field culling. No near/far test: shallow_clip intentionally keeps water
// inside the ordinary near plane. The same masked indices feed the mask and optical pass.
@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var<storage,read_write> mesh_indices: array<u32>;
// Indexed indirect command, candidate/visible/dry/frustum patch counts, then active index count.
@group(0) @binding(2) var<storage,read_write> mesh_args: array<atomic<u32>>;
// Zero is uninitialized, 1 masked, 2 + template id retained. One workgroup owns each entry.
@group(0) @binding(3) var<storage,read_write> previous_mask: array<u32>;
@group(0) @binding(4) var<storage,read> lod_templates:array<u32>;
@group(0) @binding(6) var<storage,read_write> geometry_data:array<vec4u>;
struct MeshSettings { pixels:f32, hysteresis:f32, enabled:u32, generation:u32, dt:f32, duration:f32, unused:vec2u };
@group(0) @binding(5) var<uniform> settings:MeshSettings;
var<workgroup> patch_heights:array<f32,361>;
var<workgroup> lod_errors:array<atomic<u32>,3>;
var<workgroup> world_errors:array<atomic<u32>,3>;
var<workgroup> chosen_template:u32;
var<workgroup> coarsen:u32;
var<workgroup> ledge_patch:atomic<u32>;
var<workgroup> pixel_scale:f32;
var<workgroup> chosen_level:u32;
var<workgroup> patch_bounds: array<vec4f,64>;
var<workgroup> patch_visible:u32;
var<workgroup> patch_update:u32;

fn patch_outside(lo:vec3f,hi:vec3f)->bool {
    var sides=vec4f(-3.402823e38);var max_w=-3.402823e38;
    for(var c=0u;c<8u;c++) {
        let world=vec3f(select(lo.x,hi.x,(c&1u)!=0u),select(lo.y,hi.y,(c&2u)!=0u),select(lo.z,hi.z,(c&4u)!=0u));
        let p=view.view_proj*vec4f(world,1.0);
        if(any(abs(p)>vec4f(3.402823e38))||any(p!=p)){return false;}
        let margin=1e-5*max(1.0,max(max(abs(p.x),abs(p.y)),abs(p.w)));
        sides=max(sides,vec4f(p.x+p.w,p.w-p.x,p.y+p.w,p.w-p.y)+vec4f(margin));
        max_w=max(max_w,p.w+margin);
    }
    return any(sides<vec4f(0.0))||max_w<0.0;
}

// Perspective projection Lipschitz bound for vertical displacement inside this AABB.
fn vertical_pixel_scale(lo:vec3f,hi:vec3f)->f32 {
    var nearest=3.402823e38;var largest=vec2f(0.0);
    for(var c=0u;c<8u;c++) {
        let world=vec3f(select(lo.x,hi.x,(c&1u)!=0u),select(lo.y,hi.y,(c&2u)!=0u),select(lo.z,hi.z,(c&4u)!=0u));
        let p=view.view_proj*vec4f(world,1.0);nearest=min(nearest,p.w);largest=max(largest,abs(p.xy));
        if(any(abs(p)>vec4f(3.402823e38))||any(p!=p)){return 3.402823e38;}
    }
    if(nearest<=0.0001){return 3.402823e38;}
    let output_size=view.viewport.xy/max(view.texture_lod.xy,vec2f(0.0001));
    let change=abs(view.view_proj[1].xy)/nearest+largest*abs(view.view_proj[1].w)/(nearest*nearest);
    return length(change*output_size*0.5)*1.01;
}
// A conservative camera-side test keeps the complete surface whenever the medium can apply.
// Orthographic camera-origin prisms retain full detail until their LOD contract is established.
fn keep_camera_detail()->bool {
    if(abs(view.proj[3].w)>=0.5){return true;}
    let grid=(view.camera_pos.xz-shallow.origin)/shallow.cell-0.5;
    if(any(grid<vec2f(0.0))||any(grid>vec2f(shallow.count.xy-vec2u(1u)))){return false;}
    let a=vec2i(min(vec2u(floor(grid)),shallow.count.xy-vec2u(2u)));
    let surface=max(max(shallow_vertex(a.x,a.y).x,shallow_vertex(a.x+1,a.y).x),max(shallow_vertex(a.x,a.y+1).x,shallow_vertex(a.x+1,a.y+1).x));
    return view.camera_pos.y<=surface+WATER_VIEW_EPSILON;
}
fn patch_height(local:vec2u,begin:vec2u,first:vec2i,extent:vec2u)->f32 {
    let at=begin+local-vec2u(first);return patch_heights[at.x+extent.x*at.y];
}
fn level_error(level:u32)->vec2f {
    if(level==0u){return vec2f(0.0);}
    return vec2f(bitcast<f32>(atomicLoad(&lod_errors[level-1u])),bitcast<f32>(atomicLoad(&world_errors[level-1u])));
}
fn metric(error:vec2f)->f32{return max(error.x/settings.pixels,error.y/max(patch_bounds[0].w*0.5,0.000001));}
fn bridge_template(a:u32,b:u32)->u32 {
    if(a==0u){return 3u+b;}if(a==1u){return 5u+b;}return 9u;
}
@compute @workgroup_size(64)
fn build(@builtin(workgroup_id) tile:vec3u,@builtin(local_invocation_index) lane:u32) {
    let blocky=shallow.surround.w>0.5;
    let per=select(1u,max(shallow.count.w,1u),blocky);
    let units=select(shallow.count.xy-vec2u(1u),shallow.count.xy/per,blocky);
    let begin=tile.xy*16u;let end=min(begin+vec2u(16u),units);let size=end-begin;
    let first=select(max(vec2i(begin)-vec2i(1),vec2i(0)),vec2i(begin*per),blocky);
    let last=min(select(end+vec2u(1u),(end+vec2u(1u))*per-vec2u(1u),blocky),shallow.count.xy-vec2u(1u));
    let extent=last-vec2u(first)+vec2u(1u);
    if(ledge_draw.y!=0u){if(lane==0u){atomicStore(&ledge_patch,0u);}workgroupBarrier();}
    var bound=vec4f(3.402823e38,-3.402823e38,0.0,3.402823e38);
    for(var n=lane;n<extent.x*extent.y;n+=64u) {
        let at=first+vec2i(i32(n%extent.x),i32(n/extent.x));let c=shallow_column(at.x,at.y);
        bound=vec4f(min(bound.x,c.x-0.05),max(bound.y,c.x),max(bound.z,c.y),min(bound.w,c.y));
        if(!blocky){patch_heights[n]=c.x;
            if(ledge_draw.y!=0u && all(at>=vec2i(0)) && all(at<vec2i(shallow.count.xy)-vec2i(1))){if(textureLoad(ledge_mask,at,0).x!=0u){atomicStore(&ledge_patch,1u);}}
        }
    }
    patch_bounds[lane]=bound;workgroupBarrier();
    for(var stride=32u;stride>0u;stride/=2u) {
        if(lane<stride) {
            let a=patch_bounds[lane];let b=patch_bounds[lane+stride];
            patch_bounds[lane]=vec4f(min(a.x,b.x),max(a.y,b.y),max(a.z,b.z),min(a.w,b.w));
        }
        workgroupBarrier();
    }
    if(lane<3u){atomicStore(&lod_errors[lane],0u);atomicStore(&world_errors[lane],0u);}
    if(lane==0u) {
        atomicAdd(&mesh_args[5],1u);
        let b=patch_bounds[0];let half=select(0.5,0.0,blocky);
        let lo=vec3f(shallow.origin.x+(f32(begin.x*per)+half)*shallow.cell,b.x,shallow.origin.y+(f32(begin.y*per)+half)*shallow.cell);
        let hi=vec3f(shallow.origin.x+(f32(end.x*per)+half)*shallow.cell,b.y,shallow.origin.y+(f32(end.y*per)+half)*shallow.cell);
        let dry=b.z<0.002;let outside=patch_outside(lo,hi);patch_visible=select(1u,0u,dry||outside);
        pixel_scale=vertical_pixel_scale(lo,hi);
        coarsen=u32(!blocky&&all(size==vec2u(16u))&&b.w>SHALLOW_WET&&settings.enabled!=0u&&settings.pixels>0.0&&pixel_scale<3.402823e38&&!keep_camera_detail()&&(ledge_draw.y==0u || atomicLoad(&ledge_patch)==0u));
        if(dry){atomicAdd(&mesh_args[7],1u);}else if(outside){atomicAdd(&mesh_args[8],1u);}else{atomicAdd(&mesh_args[6],1u);}
    }
    workgroupBarrier();
    // Both the original grid and a fan interpolate the same affine plane plus residuals.
    // Their difference is bounded by the residual range at all grid vertices in that subpatch.
    for(var candidate=0u;candidate<3u;candidate++) {
        let step=4u<<candidate;let side=16u/step;
        if(coarsen!=0u&&patch_visible!=0u&&lane<side*side) {
            let base=vec2u(lane%side,lane/side)*step;
            let a=patch_height(base,begin,first,extent);let b=patch_height(base+vec2u(step,0u),begin,first,extent);let d=patch_height(base+vec2u(0u,step),begin,first,extent);
            var low=3.402823e38;var high=-3.402823e38;
            for(var z=0u;z<=step;z++){for(var x=0u;x<=step;x++){
                let plane=a+(b-a)*(f32(x)/f32(step))+(d-a)*(f32(z)/f32(step));
                let residual=patch_height(base+vec2u(x,z),begin,first,extent)-plane;
                low=min(low,residual);high=max(high,residual);
            }}
            let rounding=16.0*0.00000011920928955078125*max(1.0,max(abs(patch_bounds[0].x),abs(patch_bounds[0].y)));
            let world_error=high-low+rounding;
            // Keep the approximate surface above its interpolated bed even in thin water.
            atomicMax(&lod_errors[candidate],bitcast<u32>(world_error*pixel_scale));
            atomicMax(&world_errors[candidate],bitcast<u32>(world_error));
        }
    }
    workgroupBarrier();
    if(lane==0u) {
        let patch_id=tile.x+((units.x+15u)/16u)*tile.y;let state_row=geometry_data[0].y+patch_id*2u;
        var state=geometry_data[state_row];let generation=geometry_data[state_row+1u].x;
        let old_level=select(state.x,state.y,state.w==2u);
        var wanted=0u;
        if(coarsen!=0u&&patch_visible!=0u){for(var level=1u;level<=3u;level++){
            let error=level_error(level);let limit=settings.pixels*select(1.0-settings.hysteresis,1.0-settings.hysteresis*0.5,old_level>=level);
            if(error.x<=limit&&error.y<=patch_bounds[0].w*0.5){wanted=level;}
        }}
        var reset=false;
        if(coarsen==0u||patch_visible==0u){
            reset=state.w!=0u&&(state.x!=0u||state.y!=0u);state=vec4u(0u,0u,0u,select(0u,1u,patch_visible!=0u));
        }else if(state.w==0u||generation!=settings.generation){state=vec4u(wanted,wanted,0u,1u);}
        else {
            if(state.w==1u&&wanted!=state.x){
                let old=state.x;state.x=min(old,wanted);state.y=max(old,wanted);state.z=bitcast<u32>(select(1.0,0.0,old==state.x));state.w=select(3u,2u,wanted==state.y);
            }
            if(state.w>=2u){
                if(wanted<=state.x){state.w=3u;}else if(wanted>=state.y){state.w=2u;}
                else{state.w=select(2u,3u,wanted-state.x<state.y-wanted);}
                let ea=level_error(state.x);let eb=level_error(state.y);let ma=metric(ea);let mb=metric(eb);
                var t=clamp(bitcast<f32>(state.z)+select(-1.0,1.0,state.w==2u)*settings.dt/max(settings.duration,0.001),0.0,1.0);
                if(min(ma,mb)>1.0){reset=true;state=vec4u(0u,0u,0u,1u);}
                else {
                    let before=t;
                    if(mb>ma){t=min(t,clamp((1.0-ma)/max(mb-ma,0.0000001),0.0,1.0));}
                    else if(ma>mb){t=max(t,clamp((ma-1.0)/max(ma-mb,0.0000001),0.0,1.0));}
                    reset=abs(t-before)>0.000001;
                    let applied=mix(ea,eb,t);
                    if(applied.x>settings.pixels||applied.y>patch_bounds[0].w*0.5){reset=true;state=vec4u(0u,0u,0u,1u);}
                    else if(t<=0.000001){state=vec4u(state.x,state.x,0u,1u);}
                    else if(t>=0.999999){state=vec4u(state.y,state.y,0u,1u);}
                    else{state.z=bitcast<u32>(t);}
                }
            }
        }
        geometry_data[state_row]=state;geometry_data[state_row+1u]=vec4u(settings.generation,0u,0u,0u);
        chosen_template=state.x;chosen_level=state.x;var selected_error=0.0;
        if(state.w>=2u){chosen_template=bridge_template(state.x,state.y);chosen_level=select(state.x,state.y,state.w==2u);selected_error=mix(level_error(state.x).x,level_error(state.y).x,bitcast<f32>(state.z));atomicAdd(&mesh_args[16],1u);}
        else if(state.w==1u&&coarsen!=0u){selected_error=level_error(state.x).x;}
        if(reset){atomicAdd(&mesh_args[17],1u);}
        let key=select(1u,2u+chosen_template,patch_visible!=0u);patch_update=u32(previous_mask[patch_id]!=key);previous_mask[patch_id]=key;
        if(patch_update!=0u){atomicAdd(&mesh_args[10],1u);}
        atomicStore(&mesh_args[1],1u);
        if(patch_visible!=0u){
            let count=select(size.x*size.y*select(6u,18u,blocky),lod_templates[chosen_template*2u+1u],chosen_template>0u);
            let full=units.x*units.y*select(6u,18u,blocky);var submitted=full;
            if(chosen_template>=4u&&count>1536u){submitted=full+patch_id*geometry_data[0].z+count-1536u;}
            atomicMax(&mesh_args[0],submitted);atomicAdd(&mesh_args[9],count);
            atomicAdd(&mesh_args[11u+chosen_level],1u);atomicMax(&mesh_args[15],bitcast<u32>(selected_error));
        }
    }
}
