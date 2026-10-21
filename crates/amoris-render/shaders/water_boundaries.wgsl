// Appended to water_compute.wgsl. Flux vectors contain signed discharge per width, accumulated
// entered/left volume, and normal velocity. Each entry has one owner, including at corners.
@group(0) @binding(7) var<storage,read_write> boundary_report:array<vec4f>;
var<workgroup> boundary_sums:array<vec4f,64>;
fn edge_count()->u32 {return 2u*(p.size.x+p.size.y);}
// side, column x, column z, axis
fn edge_address(face:u32)->vec4u {
    if(face<p.size.y){return vec4u(0u,0u,face,0u);}
    if(face<2u*p.size.y){return vec4u(1u,p.size.x-1u,face-p.size.y,0u);}
    if(face<2u*p.size.y+p.size.x){return vec4u(2u,face-2u*p.size.y,0u,1u);}
    return vec4u(3u,face-2u*p.size.y-p.size.x,p.size.y-1u,1u);
}
fn outward(side:u32)->f32 {return select(-1.0,1.0,(side%2u)==1u);}
fn outlet_velocity(a:vec4u)->f32 {
    let side=a.x;let i=a.y;let k=a.z;let axis=a.w;let n=at(i,k);let h=src[n].y;
    if(h<=0.0001){return 0.0;}
    let sign=outward(side);var near=vec2u(i,k);
    if(sign>0.0){near[axis]-=1u;}else{near[axis]+=1u;}
    if(src[n].x-src[at(near.x,near.y)].x>=h){return 0.0;}
    // Transmissive exit: use the updated adjacent normal face, without extra boundary damping.
    var inner=vec2u(i,k);if(sign<0.0){inner[axis]+=1u;}
    let value=velocity[at(inner.x,inner.y)*2u+axis];
    return sign*max(sign*value,0.0);
}
fn stage_velocity(a:vec4u,level:f32)->f32 {
    let side=a.x;let i=a.y;let k=a.z;let axis=a.w;let n=at(i,k);let h=src[n].y;
    if(h<=0.0001){return 0.0;}
    let sign=outward(side);var face=vec2u(i,k);if(sign>0.0){face[axis]+=1u;}
    var advector:vec2f;
    if(axis==0u){advector=vec2f(u(face.x,k),(w(i,k)+w(i,k+1u))*0.5);}
    else{advector=vec2f((u(i,k)+u(i+1u,k))*0.5,w(i,face.y));}
    let advected=sample_face(vec2f(face)-advector*(p.dt/p.cell),axis);
    var near=vec2u(i,k);if(sign>0.0){near[axis]-=1u;}else{near[axis]+=1u;}
    let bed=src[n].x;let ghost_bed=2.0*bed-src[at(near.x,near.y)].x;
    if(ghost_bed>=bed+h){return 0.0;}
    let outside=max(level-ghost_bed,0.0);
    let delta=clamp(max(level,ghost_bed)-(bed+h),-h,outside);
    var value=advected-9.81*p.dt*sign*delta/p.cell;
    let calm=0.2*min(1.0,0.5/max(abs(value),0.000001));
    value/=1.0+p.dt*(p.friction/max(max(h,outside),0.05)+calm);
    return sign*clamp(sign*value,0.0,0.5*p.cell/p.dt);
}
// Local normal characteristic matching. This is a smooth-wave reservoir boundary, not
// an exact shock Riemann solver. Transcritical rarefactions use their sonic interface state.
// Returns outward velocity and outward discharge per unit width, before donor limiting.
fn radiation(a:vec4u,level:f32,external_velocity:f32)->vec2f {
    let n=at(a.y,a.z);let h=src[n].y;let he=max(level-src[n].x,0.0);
    if(h<=0.0001&&he<=0.0001){return vec2f(0.0);}
    // An empty exterior column carries no velocity characteristic or momentum.
    let ue=select(0.0,external_velocity,he>0.0001);
    let sign=outward(a.x);var inner=a.yz;if(sign<0.0){inner[a.w]+=1u;}
    let ui=sign*velocity[at(inner.x,inner.y)*2u+a.w];
    let ci=sqrt(9.81*h);let ce=sqrt(9.81*he);var ub=0.0;var hb=0.0;
    if(h>0.0001&&ui>=ci){ub=ui;hb=h;}
    else if(he>0.0001&&ue<=-ce){ub=ue;hb=he;}
    else {
        let plus=ui+2.0*ci;let minus=ue-2.0*ce;
        var cb=max((plus-minus)*0.25,0.0);ub=(plus+minus)*0.5;
        if(cb==0.0){ub=0.0;}
        else if(ub>cb){cb=max(plus/3.0,0.0);ub=cb;}
        else if(ub < -cb){cb=max(-minus/3.0,0.0);ub=-cb;}
        hb=cb*cb/9.81;
    }
    var q=ub*hb;if(q>0.0&&h<=0.0001){q=0.0;}
    return vec2f(sign*ub,sign*q);
}
// Wave phases are local to the region origin at the start of this recorded step.
// Each reduction advances a disposable, reset-per-submission midpoint clock.
fn sea_state(a:vec4u)->vec2f {
    let bed=src[at(a.y,a.z)].x;let h=max(p.sea_meta.x-bed,0.0);var level=p.sea_meta.x;var un=0.0;
    if(h<=0.0001){return vec2f(level,un);}
    let c=sqrt(9.81*h);let sign=outward(a.x);var pos=(vec2f(a.yz)+vec2f(0.5))*p.cell;
    pos[a.w]=select(0.0,f32(p.size[a.w])*p.cell,sign>0.0);
    let time=(boundary_report[1].x+0.5)*p.dt;
    for(var i=0u;i<u32(p.sea_meta.y);i++){
        let w=p.waves[i];if(w.phase[a.w]*sign>=0.0){continue;}
        let wavelength=6.283185307179586*c/w.phase.z;
        if(wavelength<4.0*p.cell||wavelength<20.0*h){continue;}
        let tangent=w.phase[1u-a.w]*c/w.phase.z;let normal=sqrt(max(1.0-tangent*tangent,0.0));
        let eta=w.data.x*sin(dot(w.phase.xy,pos)-w.phase.z*time+w.phase.w);
        level+=eta;un-=eta*sqrt(9.81/h)*normal;
    }
    return vec2f(level,un);
}
@compute @workgroup_size(64)
fn boundary_predict(@builtin(global_invocation_id) id:vec3u){
    let face=id.x;if(face>=edge_count()){return;}
    let a=edge_address(face);let n=at(a.y,a.z);let sign=outward(a.x);
    var value=0.0;var discharge=0.0;
    if(awake[n]!=0u){
        if(p.edge_kind[a.x]==1u){
            let q=p.edge_rate[a.x];
            if(q>0.0){
                let critical=pow(q/sqrt(9.81),2.0/3.0);
                value=-sign*q/max(src[n].y,critical);discharge=-sign*q;
            }
        }else if(p.edge_kind[a.x]==2u){value=outlet_velocity(a);discharge=value*src[n].y;}
        else if(p.edge_kind[a.x]==3u){value=stage_velocity(a,p.edge_rate[a.x]);discharge=value*src[n].y;}
        else if(p.edge_kind[a.x]==4u){let r=radiation(a,p.edge_rate[a.x],p.edge_normal[a.x]);value=r.x;discharge=r.y;}
        else if(p.edge_kind[a.x]==5u){let state=sea_state(a);let r=radiation(a,state.x,state.y);value=r.x;discharge=r.y;}
    }
    // Keep a quiescent exact checkpoint unchanged on negative-facing sides too.
    if((bitcast<u32>(value)&0x7fffffffu)==0u){value=bitcast<f32>(0u);}
    if((bitcast<u32>(discharge)&0x7fffffffu)==0u){discharge=bitcast<f32>(0u);}
    let old=edge_flux[face];edge_flux[face]=vec4f(discharge,old.yz,value);
    // Scalar addresses keep independent west/south owners from racing at their common corner.
    if(a.x==0u||a.x==2u){velocity[n*2u+a.w]=value;}
}
@compute @workgroup_size(64)
fn boundary_limit(@builtin(global_invocation_id) id:vec3u){
    let first=id.x*4u;if(first>=edge_count()){return;}
    var velocities=vec4f(0.0);
    for(var j=0u;j<4u;j++){
        let face=first+j;let a=edge_address(face);let sign=outward(a.x);var f=edge_flux[face];
        if(f.x*sign>0.0){f.x*=scale[at(a.y,a.z)];}
        let amount=f.x*sign*p.dt*p.cell;
        f.y+=max(-amount,0.0);f.z+=max(amount,0.0);edge_flux[face]=f;velocities[j]=f.w;
    }
    let a=edge_address(first);
    // Dimensions divisible by four keep each packet on one side and one packed vec4 owner.
    if(a.x==1u){dst[p.size.x*p.size.y+(first-p.size.y)/4u]=velocities;}
    if(a.x==3u){dst[p.size.x*p.size.y+(first-p.size.y-p.size.x)/4u]=velocities;}
}
@compute @workgroup_size(64)
fn boundary_reduce(@builtin(local_invocation_index) lane:u32){
    var sum=vec4f(0.0);let limit=0.5*p.cell/p.dt;
    for(var i=lane;i<edge_count();i+=64u){
        let f=edge_flux[i];sum.x+=f.y;sum.y+=f.z;
        if(any(abs(f)>vec4f(3.402823e38))||any(f!=f)||abs(f.w)>limit*1.00001){sum.z+=1.0;}
    }
    boundary_sums[lane]=sum;workgroupBarrier();
    for(var stride=32u;stride>0u;stride/=2u){
        if(lane<stride){boundary_sums[lane]+=boundary_sums[lane+stride];}workgroupBarrier();
    }
    if(lane==0u){boundary_sums[0].z+=boundary_report[0].z;boundary_report[0]=boundary_sums[0];boundary_report[1].x+=1.0;}
}
