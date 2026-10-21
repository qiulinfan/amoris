// Dilute presentation spray. Water columns are current displayed surface/depth/centred velocity.
struct SprayParams { count:vec4u, origin_cell_dt:vec4f, flags:vec4u, rates:vec4f, absorb:vec4f }
struct Droplet { position:vec4f, velocity:vec4f, info:vec4u }
struct SprayState { head:atomic<u32>, born:atomic<u32>, alive:atomic<u32>, sources:atomic<u32>, vertex_count:u32, draw_instances:atomic<u32>, first_vertex:u32, first_instance:u32 }
@group(0) @binding(0) var<uniform> view:View;
@group(0) @binding(1) var<uniform> p:SprayParams;
@group(0) @binding(2) var<storage,read> water:array<vec4f>;
@group(0) @binding(3) var<storage,read_write> droplets:array<Droplet>;
@group(0) @binding(4) var<storage,read_write> state:SprayState;
@group(0) @binding(5) var<storage,read_write> budget:atomic<u32>;
@group(0) @binding(6) var<storage,read_write> impacts:array<atomic<u32>>;
fn spray_random(seed:u32)->f32 {return f32(pcg_hash(seed)>>8u)/16777216.0;}
fn column(ij:vec2u)->vec4f {
    if(p.count.z!=0u){return water[((ij.x/16u)+(p.count.x/16u)*(ij.y/16u))*256u+ij.x%16u+16u*(ij.y%16u)];}
    return water[ij.x+p.count.x*ij.y];
}
fn inside(xz:vec2f)->bool {let q=(xz-p.origin_cell_dt.xy)/p.origin_cell_dt.z;return all(q>=vec2f(0.0))&&all(q<vec2f(p.count.xy));}
fn valid(c:vec4f)->bool {return c.y>=0.0&&abs(c.x)<1e10;}
@compute @workgroup_size(64)
fn simulate(@builtin(global_invocation_id) id:vec3u){
    if(id.x>=p.count.w){return;}
    if(id.x==0u){state.vertex_count=6u;}
    var d=droplets[id.x];let dt=p.origin_cell_dt.w;
    if(d.velocity.w<=0.0||d.position.w>=d.velocity.w){return;}
    let old=d.position.xyz;let v=d.velocity.xyz;
    let steps=u32(clamp(ceil(length(v*dt)/p.origin_cell_dt.z),1.0,8.0));
    var hit=false;
    for(var i=1u;i<=steps;i++){
        let t=dt*f32(i)/f32(steps);let at=old+v*t-vec3f(0.0,4.905*t*t,0.0);
        if(!inside(at.xz)){d.velocity.w=0.0;hit=true;break;}
        let ij=vec2u((at.xz-p.origin_cell_dt.xy)/p.origin_cell_dt.z);let c=column(ij);
        if(valid(c)&&at.y<=c.x){
            if(c.y>0.01){atomicAdd(&impacts[ij.x+p.count.x*ij.y],1u);}
            let vy=v.y-9.81*t;
            if(d.info.x==0u&&vy < -3.0&&c.y>0.01){
                let seed=d.info.y;let spread=min(3.0,-vy*0.18);
                d.position=vec4f(at.x,c.x+0.025,at.z,0.0);
                d.velocity=vec4f(v.x*0.2+(spray_random(seed+11u)*2.0-1.0)*spread,min(5.0,-vy*0.35),v.z*0.2+(spray_random(seed+17u)*2.0-1.0)*spread,1.2);
                d.info.x=1u;
            }else{d.velocity.w=0.0;}
            hit=true;break;
        }
    }
    if(!hit){d.position=vec4f(old+v*dt-vec3f(0.0,4.905*dt*dt,0.0),d.position.w+dt);d.velocity=vec4f(v-vec3f(0.0,9.81*dt,0.0),d.velocity.w);}
    if(d.position.w<d.velocity.w){atomicAdd(&state.alive,1u);atomicStore(&state.draw_instances,p.count.w);}
    droplets[id.x]=d;
}
fn release_particle(pos:vec3f,velocity:vec3f,kind:u32,seed:u32){
    let ticket=atomicAdd(&budget,1u);if(ticket>=p.flags.w){return;}
    let slot=atomicAdd(&state.head,1u)%p.count.w;
    var d:Droplet;d.position=vec4f(pos,0.0);d.velocity=vec4f(velocity,select(4.0,1.2,kind!=0u));d.info=vec4u(kind,seed,0u,0u);
    droplets[slot]=d;atomicAdd(&state.born,1u);atomicStore(&state.draw_instances,p.count.w);
}
fn face(ij:vec2u,axis:u32){
    let offset=select(vec2u(1u,0u),vec2u(0u,1u),axis==1u);if(any(ij+offset>=p.count.xy)){return;}
    let a=column(ij);let b=column(ij+offset);if(!valid(a)||!valid(b)){return;}
    let speed=(a[axis+2u]+b[axis+2u])*0.5;let positive=speed>0.0;
    let donor=select(b,a,positive);let receiver=select(a,b,positive);
    if(donor.y<=0.01||abs(speed)<0.05){return;}
    let drop=donor.x-receiver.x;let rise=(receiver.x-receiver.y)-(donor.x-donor.y);
    let fall=drop>0.5;let rapid=length(donor.zw)>4.0&&rise>0.25&&receiver.x-receiver.y<donor.x;
    if(!fall&&!rapid){return;}
    if(p.flags.y!=0u){atomicAdd(&state.sources,1u);}
    if(p.origin_cell_dt.w<=0.0){return;}
    let center=(vec2f(ij)+vec2f(0.5)+vec2f(offset)*0.5)*p.origin_cell_dt.z+p.origin_cell_dt.xy;
    if(distance(center,view.camera_pos.xz)>p.rates.y){return;}
    let discharge=abs(speed)*donor.y*p.origin_cell_dt.z;
    var rate=discharge/p.rates.x;
    if(!fall){rate=min(30.0,rate*0.1);}
    let wanted=min(rate*p.origin_cell_dt.w,32.0);
    let key=ij.x+p.count.x*ij.y+axis*p.count.x*p.count.y;
    let seed=pcg_hash(key^(p.flags.x*747796405u));
    let count=u32(floor(wanted))+u32(spray_random(seed+3u)<fract(wanted));
    for(var j=0u;j<count;j++){
        if(atomicLoad(&budget)>=p.flags.w){break;}
        let s=pcg_hash(seed+j*277803737u);let tangent=select(vec2f(0.0,1.0),vec2f(1.0,0.0),axis==1u);
        let pos=center+tangent*((spray_random(s)*2.0-1.0)*p.origin_cell_dt.z*0.48)+vec2f(offset)*select(-0.01,0.01,positive);
        var velocity=vec3f(donor.z,0.0,donor.w);velocity[axis*2u]=speed;
        if(rapid&&!fall){velocity.y=min(3.0,sqrt(19.62*rise)*0.6);velocity=vec3f(velocity.x*0.6,velocity.y,velocity.z*0.6);}
        release_particle(vec3f(pos.x,donor.x+0.025,pos.y),velocity,select(2u,0u,fall),s);
    }
}
@compute @workgroup_size(8,8)
fn emit(@builtin(global_invocation_id) id:vec3u){
    if(any(id.xy>=p.count.xy)){return;}
    if(p.flags.y==0u&&atomicLoad(&state.sources)==0u){return;}
    face(id.xy,0u);face(id.xy,1u);
}
