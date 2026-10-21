// One owner per MAC face; no float atomics. Point support and face writes are separate
// dispatches. Faces write only z/w scalar locations while reading immutable depth y locations.
struct Params { shape:vec4u, metric:vec4f }
struct Report { momentum:vec4f, rounding:vec4f }
@group(0) @binding(0) var<uniform> p:Params;
// Scalar addressing prevents backends from lowering a dynamic vec4 component store into a
// read/modify/write of the whole column: independent u/w owners must not overwrite each other.
@group(0) @binding(1) var<storage,read_write> field:array<f32>;
@group(0) @binding(2) var<storage,read> points:array<vec4f>;
@group(0) @binding(3) var<storage,read_write> missing:array<vec2f>;
@group(0) @binding(4) var<storage,read> runs:array<vec4u>;
@group(0) @binding(5) var<storage,read> links:array<u32>;
@group(0) @binding(6) var<storage,read_write> stats:array<vec4f>;
@group(0) @binding(7) var<storage,read_write> result:Report;
var<workgroup> sums:array<vec4f,64>;
var<workgroup> errors:array<vec4f,64>;

fn mass(face:u32,axis:u32)->f32 {
    let ij=vec2u(face%p.shape.x,face/p.shape.x);
    if(ij[axis]==0u){return 0.0;}
    let delta=select(1u,p.shape.x,axis==1u);
    return (field[face*4u+1u]+field[(face-delta)*4u+1u])*0.5*1000.0*p.metric.x*p.metric.x;
}
fn centre(point:vec2f,corner:u32)->vec3f {
    return water_point_corner(point,corner,p.shape.xy);
}
fn face_weight(point:vec2f,face:u32,axis:u32)->f32 {
    var weight=0.0;
    for(var c=0u;c<4u;c++){
        let v=centre(point,c);let ij=vec2u(v.xy);let here=ij.x+p.shape.x*ij.y;
        if(here==face){weight+=v.z;}
        if(ij[axis]+1u<p.shape[axis]){
            if(here+select(1u,p.shape.x,axis==1u)==face){weight+=v.z;}
        }
    }
    return weight;
}
@compute @workgroup_size(64)
fn prepare(@builtin(global_invocation_id) id:vec3u){
    if(id.x>=p.shape.z){return;}
    let point=points[id.x].xy;var total=vec2f(0.0);
    for(var c=0u;c<4u;c++){
        let v=centre(point,c);let ij=vec2u(v.xy);let face=ij.x+p.shape.x*ij.y;
        for(var axis=0u;axis<2u;axis++){
            if(mass(face,axis)<=0.0){total[axis]+=v.z;}
            if(ij[axis]+1u<p.shape[axis]){
                if(mass(face+select(1u,p.shape.x,axis==1u),axis)<=0.0){total[axis]+=v.z;}
            }else{total[axis]+=v.z;}
        }
    }
    missing[id.x]=total;
}
@compute @workgroup_size(64)
fn apply(@builtin(global_invocation_id) id:vec3u){
    if(id.x>=p.shape.w){return;}
    let run=runs[id.x];let m=mass(run.x,run.y);var requested=0.0;
    if(m>0.0){
        for(var i=0u;i<run.w;i++){
            let n=links[run.z+i];
            requested+=points[n][run.y+2u]*face_weight(points[n].xy,run.x,run.y);
        }
    }
    let old=field[run.x*4u+run.y+2u];var invalid=0.0;
    if(m>0.0 && requested!=0.0){
        let index=run.x*4u+run.y+2u;
        let value=old+requested/m;
        if(abs(value)<=3.402823e38){
            field[index]=value;
        }else{invalid=1.0;}
    }
    stats[id.x]=vec4f(old,requested,m,invalid);
}
@compute @workgroup_size(64)
fn summarize(@builtin(local_invocation_index) lane:u32){
    var total=vec4f(0.0);var residual=vec4f(0.0);
    for(var n=lane;n<p.shape.z;n+=64u){
        let unsupported=points[n].zw*missing[n];total.z+=unsupported.x;total.w+=unsupported.y;
    }
    for(var n=lane;n<p.shape.w;n+=64u){
        let s=stats[n];let run=runs[n];let axis=run.y;
        // Reload the committed scalar in a later dispatch. Reusing `old+J/m` can let a
        // compiler cancel the subtraction before storage rounding and hide the actual error.
        let actual=(field[run.x*4u+axis+2u]-s.x)*s.z;
        total[axis]+=actual;residual[axis]+=s.y-actual;residual.z+=s.w;
    }
    sums[lane]=total;errors[lane]=residual;workgroupBarrier();
    for(var stride=32u;stride>0u;stride/=2u){
        if(lane<stride){sums[lane]+=sums[lane+stride];errors[lane]+=errors[lane+stride];}
        workgroupBarrier();
    }
    if(lane==0u){result.momentum=sums[0];result.rounding=errors[0];}
}
