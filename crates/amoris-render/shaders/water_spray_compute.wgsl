// Presentation spray. Water columns are current displayed surface/depth/centred velocity.
// Kinds: 0 a falling sheet's water, 1 spray thrown up where it lands, 2 rapids, 3 a splash where
// the surface rises fast (a fall's foot, a bore), 4 a breaking crest's lip thrown ahead of its
// face, 5 mist from a fall's foot, 6 the crown a body throws up as it enters. None of it carries
// water or force.
struct SprayParams { count:vec4u, origin_cell_dt:vec4f, flags:vec4u, rates:vec4f, absorb:vec4f, entry:vec4f }
struct Droplet { position:vec4f, velocity:vec4f, info:vec4u }
struct SprayState { head:atomic<u32>, born:atomic<u32>, alive:atomic<u32>, sources:atomic<u32>, vertex_count:u32, draw_instances:atomic<u32>, first_vertex:u32, first_instance:u32 }
@group(0) @binding(0) var<uniform> view:View;
@group(0) @binding(1) var<uniform> p:SprayParams;
@group(0) @binding(2) var<storage,read> water:array<vec4f>;
@group(0) @binding(3) var<storage,read_write> droplets:array<Droplet>;
@group(0) @binding(4) var<storage,read_write> state:SprayState;
// Births this frame, shared by every region: [0] all of them, [1] splashes and breaking lips.
@group(0) @binding(5) var<storage,read_write> budget:array<atomic<u32>,4>;
@group(0) @binding(6) var<storage,read_write> impacts:array<atomic<u32>>;
// The field's occupancy head and the head bodies added since the last advance (resident fields).
@group(0) @binding(7) var<storage,read> occupancy:array<f32>;
@group(0) @binding(8) var<storage,read_write> rise:array<f32>;
const MIST:u32=5u;
const CROWN:u32=6u;
// Splashes and breaking lips, which a big surf or a gorge's white water throws from thousands of
// columns at once, take at most this many births a frame (water_spray.rs `SPLASH_PER_FRAME`): the
// ring then still holds about two seconds of them beside a waterfall's sheet and the crowns,
// instead of overwriting the sheet's drops before they land.
const SPLASH_PER_FRAME:u32=64u;
fn spray_random(seed:u32)->f32 {return f32(pcg_hash(seed)>>8u)/16777216.0;}
fn column(ij:vec2u)->vec4f {
    if(p.count.z!=0u){return water[((ij.x/16u)+(p.count.x/16u)*(ij.y/16u))*256u+ij.x%16u+16u*(ij.y%16u)];}
    return water[ij.x+p.count.x*ij.y];
}
fn inside(xz:vec2f)->bool {let q=(xz-p.origin_cell_dt.xy)/p.origin_cell_dt.z;return all(q>=vec2f(0.0))&&all(q<vec2f(p.count.xy));}
fn valid(c:vec4f)->bool {return c.y>=0.0&&abs(c.x)<1e10;}
fn lifetime(kind:u32)->f32 {
    return array<f32,7>(4.0,1.2,1.2,1.6,2.0,3.5,1.5)[min(kind,6u)];
}
@compute @workgroup_size(64)
fn simulate(@builtin(global_invocation_id) id:vec3u){
    if(id.x>=p.count.w){return;}
    if(id.x==0u){state.vertex_count=6u;}
    var d=droplets[id.x];let dt=p.origin_cell_dt.w;
    if(d.velocity.w<=0.0||d.position.w>=d.velocity.w){return;}
    let old=d.position.xyz;let v=d.velocity.xyz;
    if(d.info.x==MIST){
        // Mist drifts on the air it entrains: little gravity and strong drag toward a slow rise.
        let drift=mix(v,vec3f(v.x*0.6,0.35,v.z*0.6),min(dt*1.5,1.0));
        d.position=vec4f(old+drift*dt,d.position.w+dt);d.velocity=vec4f(drift,d.velocity.w);
        if(!inside(d.position.xz)){d.velocity.w=0.0;}
        if(d.position.w<d.velocity.w){atomicAdd(&state.alive,1u);atomicStore(&state.draw_instances,p.count.w);}
        droplets[id.x]=d;return;
    }
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
                if(spray_random(seed+5u)<0.3){
                    // A share of a fall's foot rises as mist.
                    d.position=vec4f(at.x,c.x+0.2,at.z,0.0);
                    d.velocity=vec4f((spray_random(seed+11u)*2.0-1.0)*spread,0.6+spray_random(seed+13u)*0.8,(spray_random(seed+17u)*2.0-1.0)*spread,lifetime(MIST));
                    d.info.x=MIST;
                }else{
                    d.position=vec4f(at.x,c.x+0.025,at.z,0.0);
                    d.velocity=vec4f(v.x*0.2+(spray_random(seed+11u)*2.0-1.0)*spread,min(5.0,-vy*0.35),v.z*0.2+(spray_random(seed+17u)*2.0-1.0)*spread,lifetime(1u));
                    d.info.x=1u;
                }
            }else{d.velocity.w=0.0;}
            hit=true;break;
        }
    }
    if(!hit){d.position=vec4f(old+v*dt-vec3f(0.0,4.905*dt*dt,0.0),d.position.w+dt);d.velocity=vec4f(v-vec3f(0.0,9.81*dt,0.0),d.velocity.w);}
    if(d.position.w<d.velocity.w){atomicAdd(&state.alive,1u);atomicStore(&state.draw_instances,p.count.w);}
    droplets[id.x]=d;
}
fn release_particle(pos:vec3f,velocity:vec3f,kind:u32,seed:u32){
    let ticket=atomicAdd(&budget[0],1u);if(ticket>=p.flags.w){return;}
    let slot=atomicAdd(&state.head,1u)%p.count.w;
    var d:Droplet;d.position=vec4f(pos,0.0);d.velocity=vec4f(velocity,lifetime(kind));d.info=vec4u(kind,seed,0u,0u);
    droplets[slot]=d;atomicAdd(&state.born,1u);atomicStore(&state.draw_instances,p.count.w);
}
fn face(ij:vec2u,axis:u32){
    let offset=select(vec2u(1u,0u),vec2u(0u,1u),axis==1u);if(any(ij+offset>=p.count.xy)){return;}
    let a=column(ij);let b=column(ij+offset);if(!valid(a)||!valid(b)){return;}
    let speed=(a[axis+2u]+b[axis+2u])*0.5;let positive=speed>0.0;
    let donor=select(b,a,positive);let receiver=select(a,b,positive);
    if(donor.y<=0.01||abs(speed)<0.05){return;}
    let rise=(receiver.x-receiver.y)-(donor.x-donor.y);
    // A fall leaves a bed standing over the water below it, as the step's own falls do. A bore's
    // front is as steep but runs over one bed: its spray is the splash's, never a falling sheet.
    let fall=(donor.x-donor.y)-receiver.x>0.5;let rapid=length(donor.zw)>4.0&&rise>0.25&&receiver.x-receiver.y<donor.x;
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
        if(atomicLoad(&budget[0])>=p.flags.w){break;}
        let s=pcg_hash(seed+j*277803737u);let tangent=select(vec2f(0.0,1.0),vec2f(1.0,0.0),axis==1u);
        let pos=center+tangent*((spray_random(s)*2.0-1.0)*p.origin_cell_dt.z*0.48)+vec2f(offset)*select(-0.01,0.01,positive);
        var velocity=vec3f(donor.z,0.0,donor.w);velocity[axis*2u]=speed;
        if(rapid&&!fall){velocity.y=min(3.0,sqrt(19.62*rise)*0.6);velocity=vec3f(velocity.x*0.6,velocity.y,velocity.z*0.6);}
        release_particle(vec3f(pos.x,donor.x+0.025,pos.y),velocity,select(2u,0u,fall),s);
    }
}
// The displayed surface's rate of rise at a column, from its neighbours' centred fluxes.
fn neighbour(ij:vec2i)->vec4f {
    let c=column(vec2u(clamp(ij,vec2i(0),vec2i(p.count.xy)-1)));
    return select(vec4f(c.x,0.0,0.0,0.0),c,valid(c));
}
fn splash(ij:vec2u){
    let c=column(ij);if(!valid(c)||c.y<=0.05){return;}
    let at=vec2i(ij);
    let l=neighbour(at-vec2i(1,0));let r=neighbour(at+vec2i(1,0));
    let d=neighbour(at-vec2i(0,1));let u=neighbour(at+vec2i(0,1));
    let cell=p.origin_cell_dt.z;
    let qx=0.25*((r.y+c.y)*(r.z+c.z)-(l.y+c.y)*(l.z+c.z));
    let qz=0.25*((u.y+c.y)*(u.w+c.w)-(d.y+c.y)*(d.w+c.w));
    let rise=-(qx+qz)/cell;
    let slope=vec2f(select(r.x,c.x,r.y<=0.01)-select(l.x,c.x,l.y<=0.01),select(u.x,c.x,u.y<=0.01)-select(d.x,c.x,d.y<=0.01))/(2.0*cell);
    let celerity=sqrt(9.81*c.y);
    let steep=length(slope);
    let lip=rise>0.45*celerity&&steep>0.3;
    let burst=rise>p.rates.z;
    if(!lip&&!burst){return;}
    if(p.flags.y!=0u){atomicAdd(&state.sources,1u);}
    if(p.origin_cell_dt.w<=0.0){return;}
    let center=(vec2f(ij)+0.5)*cell+p.origin_cell_dt.xy;
    let away=distance(center,view.camera_pos.xz);
    if(away>p.rates.w){return;}
    let key=ij.x+p.count.x*ij.y+2u*p.count.x*p.count.y;
    let seed=pcg_hash(key^(p.flags.x*2891336453u));
    var wanted=0.0;
    if(lip){wanted=min(clamp(rise/(0.45*celerity)-1.0,0.2,2.0)*40.0*cell*p.origin_cell_dt.w,12.0);}
    else{wanted=min((rise-p.rates.z)*cell*cell*90.0*p.origin_cell_dt.w,16.0);}
    // Beyond 40 m a column throws fewer drops, as fewer can be seen: a long breaking front then
    // leaves the frame's splash births (SPLASH_PER_FRAME) to the white water near the camera.
    wanted*=min(40.0/max(away,1.0),1.0);
    let count=u32(floor(wanted))+u32(spray_random(seed+3u)<fract(wanted));
    // Downhill along the free surface: away from a splash's mound, ahead of a breaking face.
    let ahead=select(vec2f(0.0),-slope/max(steep,1e-6),steep>1e-4);
    for(var j=0u;j<count;j++){
        if(atomicLoad(&budget[0])>=p.flags.w||atomicAdd(&budget[1],1u)>=SPLASH_PER_FRAME){break;}
        let s=pcg_hash(seed+j*2654435761u);
        let offset=(vec2f(spray_random(s),spray_random(s+1u))-0.5)*cell;
        let jitter=(vec2f(spray_random(s+2u),spray_random(s+3u))*2.0-1.0);
        var velocity:vec3f;var kind:u32;
        if(lip){
            let throw_=celerity*(0.75+0.35*spray_random(s+4u));
            velocity=vec3f(c.z+ahead.x*throw_+jitter.x*0.4,0.4+spray_random(s+5u)*1.2,c.w+ahead.y*throw_+jitter.y*0.4);
            kind=4u;
        }else{
            let up=min(0.9*rise,8.0)*(0.5+spray_random(s+4u));
            let out=0.35*min(rise,8.0);
            velocity=vec3f(c.z+(ahead.x*out+jitter.x*0.6*out),up,c.w+(ahead.y*out+jitter.y*0.6*out));
            kind=3u;
        }
        release_particle(vec3f(center.x+offset.x,c.x+0.03,center.y+offset.y),velocity,kind,s);
    }
}
fn head_at(ij:vec2i)->f32 {
    let c=vec2u(clamp(ij,vec2i(0),vec2i(p.count.xy)-1));return occupancy[c.x+p.count.x*c.y];
}
// A body entering the water throws a crown: the head its volume adds to a column in the time
// the steps took gives how fast it went in (footprints spread a body's volume over about four
// times its waterplane, forces.rs `footprint`), and the crown leaves upward and away from the
// footprint's middle in proportion to that speed and the volume it pushed aside.
fn entry(ij:vec2u){
    if(p.entry.x==0.0){return;}
    let i=ij.x+p.count.x*ij.y;
    let added=rise[i];
    if(added==0.0){return;}
    rise[i]=0.0;
    let c=column(ij);let cell=p.origin_cell_dt.z;
    if(p.origin_cell_dt.w<=0.0||!valid(c)||c.y<=0.05||added<0.003){return;}
    let speed=min(4.0*added*p.entry.y,14.0);
    if(speed<1.2){return;}
    let center=(vec2f(ij)+0.5)*cell+p.origin_cell_dt.xy;
    if(distance(center,view.camera_pos.xz)>p.rates.w){return;}
    let at=vec2i(ij);
    let g=vec2f(head_at(at+vec2i(1,0))-head_at(at-vec2i(1,0)),head_at(at+vec2i(0,1))-head_at(at-vec2i(0,1)));
    let out=select(vec2f(0.0),-g/max(length(g),1e-6),length(g)>1e-5);
    let seed=pcg_hash((i+3u*p.count.x*p.count.y)^(p.flags.x*1597334677u));
    let wanted=min(added*cell*cell*8000.0*min(speed*0.25,2.0),48.0);
    let count=u32(floor(wanted))+u32(spray_random(seed+3u)<fract(wanted));
    for(var j=0u;j<count;j++){
        if(atomicLoad(&budget[0])>=p.flags.w){break;}
        let s=pcg_hash(seed+j*2246822519u);
        let offset=(vec2f(spray_random(s),spray_random(s+1u))-0.5)*cell;
        let jitter=vec2f(spray_random(s+2u),spray_random(s+3u))*2.0-1.0;
        let way=out+jitter*0.6;
        let side=speed*(0.15+0.3*spray_random(s+5u));
        let up=speed*(0.35+0.55*spray_random(s+4u));
        let velocity=vec3f(c.z+way.x*side,up,c.w+way.y*side);
        release_particle(vec3f(center.x+offset.x,c.x+0.04,center.y+offset.y),velocity,CROWN,s);
    }
}
@compute @workgroup_size(8,8)
fn emit(@builtin(global_invocation_id) id:vec3u){
    if(any(id.xy>=p.count.xy)){return;}
    entry(id.xy);
    if(p.flags.y==0u&&atomicLoad(&state.sources)==0u){return;}
    face(id.xy,0u);face(id.xy,1u);splash(id.xy);
}
