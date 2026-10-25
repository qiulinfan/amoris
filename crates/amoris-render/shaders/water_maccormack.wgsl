// Appended to water_compute.wgsl. Modified MacCormack velocity advection (Selle et al. 2008;
// Chentanez/Mueller 2010). Two semi-Lagrangian predictions use the same old advecting velocity.
// Each phase writes every active-grid element; pass boundaries order all storage dependencies.
@group(0) @binding(7) var<storage,read_write> mac_predicted:array<vec2f>;
@group(0) @binding(8) var<storage,read_write> mac_reversed:array<vec2f>;
// Bodies' displaced volume as metres of head per column (zero unless occupancy_on).
@group(0) @binding(9) var<storage,read> occupancy:array<f32>;

fn mac_value(i:u32,k:u32,axis:u32,predicted:bool)->f32 {
    if(!predicted){if(axis==0u){return u(i,k);}return w(i,k);}
    if (axis==0u && i>=p.size.x) || (axis==1u && k>=p.size.y){return 0.0;}
    if(predicted){return mac_predicted[at(i,k)][axis];}
    return src[at(i,k)][axis+2u];
}
fn mac_corners(ij:vec2u,axis:u32,predicted:bool)->vec4f {
    return vec4f(mac_value(ij.x,ij.y,axis,predicted),mac_value(ij.x+1u,ij.y,axis,predicted),
        mac_value(ij.x,ij.y+1u,axis,predicted),mac_value(ij.x+1u,ij.y+1u,axis,predicted));
}
fn mac_count(axis:u32)->vec2u {
    return p.size+vec2u(select(0u,1u,axis==0u),select(0u,1u,axis==1u));
}
fn mac_index(q:vec2f,axis:u32)->vec2u {
    return min(vec2u(floor(q)),mac_count(axis)-vec2u(2u));
}
fn mac_sample_predicted(pos:vec2f,axis:u32)->f32 {
    let q=clamp(pos,vec2f(0.0),vec2f(mac_count(axis)-vec2u(1u)));
    let ij=mac_index(q,axis);let t=q-vec2f(ij);let c=mac_corners(ij,axis,true);
    let top=c.x+(c.y-c.x)*t.x;let bottom=c.z+(c.w-c.z)*t.x;
    return top+(bottom-top)*t.y;
}
fn mac_trace(i:u32,k:u32,axis:u32,direction:f32)->vec2f {
    var advector:vec2f;
    if(axis==0u){
        advector=vec2f(u(i,k),(w(i-1u,k)+w(i,k)+w(i-1u,k+1u)+w(i,k+1u))*0.25);
    }else{
        advector=vec2f((u(i,k-1u)+u(i+1u,k-1u)+u(i,k)+u(i+1u,k))*0.25,w(i,k));
    }
    return vec2f(f32(i),f32(k))-advector*(direction*p.dt/p.cell);
}
fn mac_face_open(i:u32,k:u32,axis:u32)->bool {
    if(axis==0u){return i>0u && i<p.size.x && k<p.size.y && awake[at(i-1u,k)]!=0u && awake[at(i,k)]!=0u;}
    return k>0u && k<p.size.y && i<p.size.x && awake[at(i,k-1u)]!=0u && awake[at(i,k)]!=0u;
}
fn mac_face_wet(i:u32,k:u32,axis:u32)->bool {
    if(!mac_face_open(i,k,axis)){return false;}
    let n=at(i,k);let d=select(1u,p.size.x,axis==1u);
    return src[n].y>0.0001 && src[n-d].y>0.0001;
}
fn mac_stencil_wet(pos:vec2f,axis:u32)->bool {
    let limit=vec2f(mac_count(axis)-vec2u(1u));
    if(any(pos<vec2f(0.0)) || any(pos>limit)){return false;}
    let ij=mac_index(pos,axis);
    return mac_face_wet(ij.x,ij.y,axis) && mac_face_wet(ij.x+1u,ij.y,axis) &&
        mac_face_wet(ij.x,ij.y+1u,axis) && mac_face_wet(ij.x+1u,ij.y+1u,axis);
}
@compute @workgroup_size(8,8)
fn mac_forward(@builtin(global_invocation_id) id:vec3u){
    if(any(id.xy>=p.size)){return;}
    let i=id.x;let k=id.y;let n=at(i,k);
    var value=vec2f(0.0);
    if(awake[n]==0u){mac_predicted[n]=src[n].zw;return;}
    for(var axis=0u;axis<2u;axis++){
        if(mac_face_open(i,k,axis)){value[axis]=sample_face(mac_trace(i,k,axis,1.0),axis);}
    }
    mac_predicted[n]=value;
}
@compute @workgroup_size(8,8)
fn mac_reverse(@builtin(global_invocation_id) id:vec3u){
    if(any(id.xy>=p.size)){return;}
    let i=id.x;let k=id.y;let n=at(i,k);var value=vec2f(0.0);
    if(awake[n]==0u){mac_reversed[n]=src[n].zw;return;}
    for(var axis=0u;axis<2u;axis++){
        if(mac_face_open(i,k,axis)){value[axis]=mac_sample_predicted(mac_trace(i,k,axis,-1.0),axis);}
    }
    mac_reversed[n]=value;
}
fn mac_corrected(i:u32,k:u32,axis:u32)->f32 {
    let n=at(i,k);let first=mac_predicted[n][axis];
    let departure=mac_trace(i,k,axis,1.0);let arrival=mac_trace(i,k,axis,-1.0);
    if(!mac_stencil_wet(departure,axis) || !mac_stencil_wet(arrival,axis)){return first;}
    let corrected=first+0.5*(src[n][axis+2u]-mac_reversed[n][axis]);
    let donors=mac_corners(mac_index(departure,axis),axis,false);
    let lo=min(min(donors.x,donors.y),min(donors.z,donors.w));
    let hi=max(max(donors.x,donors.y),max(donors.z,donors.w));
    // Revert an overshoot rather than clipping an extremum into the transported field.
    if(corrected>=lo && corrected<=hi){return corrected;}
    return first;
}
fn mac_head(n:u32)->f32 {
    if(p.occupancy_on<=0.0){return 0.0;}
    return occupancy[n];
}
// The surface's rate of rise from the continuity equation at the step's start, m/s.
fn mac_rise(i:u32,k:u32)->f32 {
    let n=at(i,k);let h=src[n].y;
    var q=0.0;
    if(i>0u){q+=u(i,k)*0.5*(h+src[n-1u].y);}
    if(i+1u<p.size.x){q-=u(i+1u,k)*0.5*(h+src[n+1u].y);}
    if(k>0u){q+=w(i,k)*0.5*(h+src[n-p.size.x].y);}
    if(k+1u<p.size.y){q-=w(i,k+1u)*0.5*(h+src[n+p.size.x].y);}
    return q/p.cell;
}
// Breaking as eddy viscosity (Kennedy et al. 2000): it switches on where the surface rises
// faster than half the local long-wave speed and is bounded by explicit stability.
fn mac_breaking(i:u32,k:u32,axis:u32,v:f32)->f32 {
    let n=at(i,k);let o=select(n-1u,n-p.size.x,axis==1u);
    let h=0.5*(src[n].y+src[o].y);
    if(p.breaking<=0.0||h<0.05){return v;}
    let oi=select(vec2u(i-1u,k),vec2u(i,k-1u),axis==1u);
    let rise=max(mac_rise(i,k),mac_rise(oi.x,oi.y));
    let onset=0.5*sqrt(9.81*h);
    let strength=clamp(rise/onset-1.0,0.0,1.0);
    if(strength<=0.0){return v;}
    let nu=min(strength*1.44*h*rise,0.2*p.cell*p.cell/p.dt);
    var lap=-4.0*v;
    if(axis==0u){
        lap+=select(v,u(i-1u,k),i>1u)+select(v,u(i+1u,k),i+1u<p.size.x);
        lap+=select(v,u(i,k-1u),k>0u)+select(v,u(i,k+1u),k+1u<p.size.y);
    }else{
        lap+=select(v,w(i-1u,k),i>0u)+select(v,w(i+1u,k),i+1u<p.size.x);
        lap+=select(v,w(i,k-1u),k>1u)+select(v,w(i,k+1u),k+1u<p.size.y);
    }
    return v+p.dt*nu*lap/(p.cell*p.cell);
}
@compute @workgroup_size(8,8)
fn mac_faces(@builtin(global_invocation_id) id:vec3u){
    if(any(id.xy>=p.size)){return;}
    let i=id.x;let k=id.y;let n=at(i,k);var value=vec2f(0.0);
    // Frozen columns keep their stored words in transport, while their owned faces are walls
    // for active neighbours. Do not leak a frozen nonzero face into conservative fluxes.
    if(awake[n]==0u){velocity[n*2u]=0.0;velocity[n*2u+1u]=0.0;return;}
    if(mac_face_open(i,k,0u)){
        let v=mac_breaking(i,k,0u,mac_corrected(i,k,0u));
        value.x=pressure_heads(v,n-1u,n,mac_head(n-1u),mac_head(n));
    }
    if(mac_face_open(i,k,1u)){
        let v=mac_breaking(i,k,1u,mac_corrected(i,k,1u));
        value.y=pressure_heads(v,n-p.size.x,n,mac_head(n-p.size.x),mac_head(n));
    }
    velocity[n*2u]=value.x;velocity[n*2u+1u]=value.y;
}
