// Presentation spray. Water columns are current displayed surface/depth/centred velocity.
// Kinds: 0 a falling sheet's water, 1 spray thrown up where it lands, 2 rapids, 3 a splash where
// the surface rises fast (a fall's foot, a bore), 4 a breaking crest's lip thrown ahead of its
// face, 5 mist from a fall's foot, 6 the crown a body throws up as it enters. None of it carries
// water or force.
// The ring's counters, the indirect draw's arguments (from `vertex_count`), the slots of the
// parcels alive this frame in slot order, which the draw reads (its instances, `draw_instances` of
// them), and how many are alive in each block of 64 slots.
const LIST_BLOCKS:u32=SPRAY_CAPACITY/64u;
struct SprayState { head:atomic<u32>, born:atomic<u32>, alive:atomic<u32>, sources:atomic<u32>, vertex_count:u32, draw_instances:atomic<u32>, first_vertex:u32, first_instance:u32, drawn:array<u32,SPRAY_CAPACITY>, blocks:array<u32,LIST_BLOCKS> }
@group(0) @binding(0) var<uniform> view:View;
@group(0) @binding(1) var<uniform> p:SprayParams;
@group(0) @binding(2) var<storage,read> water:array<vec4f>;
@group(0) @binding(3) var<storage,read_write> droplets:array<Droplet>;
@group(0) @binding(4) var<storage,read_write> state:SprayState;
// Births this frame, shared by every region: [0] all of them, [1] splashes and breaking lips,
// [2] entry crowns.
@group(0) @binding(5) var<storage,read_write> budget:array<atomic<u32>,4>;
@group(0) @binding(6) var<storage,read_write> impacts:array<atomic<u32>>;
// What the water sounds like from the camera (water_acoustics.rs, water.md 59), shared by every
// region: per kind of source its events this frame, then the mechanical energy they dissipate
// (`joules` each) over the square of their distance from 1 m, split between the left and the
// right ear; in 1/256ths, rounded at random so that many faint sources add up.
@group(0) @binding(7) var<storage,read_write> acoustics:array<atomic<u32>,32>;
// The feed's stirring points that went down fast since its last list (water.md 52; water_spray.rs
// `Entering`), a body's together: `count.x` points, `count.y` bodies, and per body its first
// point, its number of points and its key. A point: its centre before (and its height), now (and
// its volume), and its velocity.
struct Entering { was:vec4f, now:vec4f, velocity:vec4f }
struct Crowns { count:vec4u, points:array<Entering,64>, bodies:array<vec4u,16> }
@group(0) @binding(8) var<uniform> crowns:Crowns;
fn hear(kind:u32,at:vec3f,events:f32,joules:f32,seed:u32){
    if(p.sound.x==0.0||events<=0.0){return;}
    let to=at-view.camera_pos.xyz;let r=length(to);
    let right=vec3f(view.view[0].x,view.view[1].x,view.view[2].x);
    let pan=select(0.0,clamp(dot(to,right)/r,-1.0,1.0),r>1e-3);
    let energy=events*max(joules,0.0)/max(r*r,1.0);
    let k=kind*3u;
    atomicAdd(&acoustics[k],u32(events*256.0+spray_random(seed+101u)));
    atomicAdd(&acoustics[k+1u],u32(energy*(1.0-pan)*128.0+spray_random(seed+103u)));
    atomicAdd(&acoustics[k+2u],u32(energy*(1.0+pan)*128.0+spray_random(seed+107u)));
}
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
    if(id.x==0u){state.vertex_count=6u*SPRAY_SPRITES;}
    var d=droplets[id.x];let dt=p.origin_cell_dt.w;
    if(d.velocity.w<=0.0||d.position.w>=d.velocity.w){return;}
    let old=d.position.xyz;let v=d.velocity.xyz;
    if(d.info.x==MIST){
        // Mist drifts on the air it entrains: little gravity and strong drag toward a slow rise.
        let drift=mix(v,vec3f(v.x*0.6,0.35,v.z*0.6),min(dt*1.5,1.0));
        d.position=vec4f(old+drift*dt,d.position.w+dt);d.velocity=vec4f(drift,d.velocity.w);
        if(!inside(d.position.xz)){d.velocity.w=0.0;}
        if(d.position.w<d.velocity.w){atomicAdd(&state.alive,1u);}
        droplets[id.x]=d;return;
    }
    let steps=u32(clamp(ceil(length(v*dt)/p.origin_cell_dt.z),1.0,8.0));
    var hit=false;
    for(var i=1u;i<=steps;i++){
        let t=dt*f32(i)/f32(steps);let at=old+v*t-vec3f(0.0,4.905*t*t,0.0);
        if(!inside(at.xz)){d.velocity.w=0.0;hit=true;break;}
        let ij=vec2u((at.xz-p.origin_cell_dt.xy)/p.origin_cell_dt.z);let c=column(ij);
        if(valid(c)&&at.y<=c.x){
            // A crown's parcel stands for its share of a parcel's water.
            let weight=select(1.0,crown_unpack(d.info.w).weight,d.info.x==CROWN);
            if(c.y>0.01&&spray_random(d.info.y+23u)<weight){atomicAdd(&impacts[ij.x+p.count.x*ij.y],1u);}
            let vy=v.y-9.81*t;
            // A drop landing makes the small bubbles of rain, as about 10 g of spray would; a
            // falling sheet's water is heard where the fall leaves its lip (`face`).
            if(c.y>0.01&&d.info.x!=0u){let u=vec3f(v.x,vy,v.z);hear(5u,at,weight,0.005*dot(u,u),d.info.y);}
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
    if(d.position.w<d.velocity.w){atomicAdd(&state.alive,1u);}
    droplets[id.x]=d;
}
fn release_particle(pos:vec3f,velocity:vec3f,kind:u32,seed:u32){
    let ticket=atomicAdd(&budget[0],1u);if(ticket>=p.flags.w){return;}
    let slot=atomicAdd(&state.head,1u)%p.count.w;
    var d:Droplet;d.position=vec4f(pos,0.0);d.velocity=vec4f(velocity,lifetime(kind));d.info=vec4u(kind,seed,0u,0u);
    droplets[slot]=d;atomicAdd(&state.born,1u);
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
    let discharge=abs(speed)*donor.y*p.origin_cell_dt.z;
    let key=ij.x+p.count.x*ij.y+axis*p.count.x*p.count.y;
    let seed=pcg_hash(key^(p.flags.x*747796405u));
    // Heard from any distance: a fall's water a drop's volume at a time, each dissipating the
    // height it falls, and a rapid's ten drops' at a time, each losing its speed against the step.
    let parcel=1000.0*p.rates.x*select(10.0,1.0,fall);
    let joules=select(0.5*parcel*speed*speed,parcel*9.81*max(donor.x-receiver.x,0.0),fall);
    hear(select(1u,0u,fall),vec3f(center.x,donor.x,center.y),discharge/p.rates.x*select(0.1,1.0,fall)*p.origin_cell_dt.w,joules,seed);
    if(distance(center,view.camera_pos.xz)>p.rates.y){return;}
    var rate=discharge/p.rates.x;
    if(!fall){rate=min(30.0,rate*0.1);}
    let wanted=min(rate*p.origin_cell_dt.w,32.0);
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
    let key=ij.x+p.count.x*ij.y+2u*p.count.x*p.count.y;
    let seed=pcg_hash(key^(p.flags.x*2891336453u));
    var events=0.0;var joules=0.0;
    if(lip){
        events=clamp(rise/(0.45*celerity)-1.0,0.2,2.0)*40.0*cell*p.origin_cell_dt.w;
        // A breaking crest loses what a bore of height H = 0.78 h does, rho g c H^3 / (4 (h + H))
        // a metre of crest, which 40 events a metre a second share.
        joules=0.0667*1000.0*9.81*celerity*c.y*c.y/40.0;
    }else{
        // A ninetieth of a cubic metre rising faster than the threshold, with its kinetic energy.
        events=(rise-p.rates.z)*cell*cell*90.0*p.origin_cell_dt.w;
        joules=0.5*1000.0/90.0*rise*rise;
    }
    // Heard from any distance and in full: the drops below are thinned for what can be seen.
    hear(select(2u,3u,lip),vec3f(center.x,c.x,center.y),events,joules,seed);
    if(away>p.rates.w){return;}
    var wanted=min(events,select(16.0,12.0,lip));
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
// The drawn surface at `xz`, bilinear over the wet columns about it: height, depth and the
// water's velocity; depth 0 where none is wet.
fn surface_at(xz:vec2f)->vec4f {
    let q=(xz-p.origin_cell_dt.xy)/p.origin_cell_dt.z-0.5;
    let base=vec2i(floor(q));let f=q-floor(q);
    var sum=vec4f(0.0);var weights=0.0;
    for(var n=0;n<4;n++){
        let o=vec2i(n&1,n>>1u);let ij=base+o;
        if(any(ij<vec2i(0))||any(ij>=vec2i(p.count.xy))){continue;}
        let c=column(vec2u(ij));
        if(!valid(c)||c.y<=0.0){continue;}
        let w=select(1.0-f.x,f.x,o.x==1)*select(1.0-f.y,f.y,o.y==1);
        sum+=c*w;weights+=w;
    }
    if(weights<=1e-6){return vec4f(0.0);}
    return sum/weights;
}
// A crown's lattice: rows at these times the footprint's ellipse (the cavity's rim, a little
// wider than the body), each leaving upward and outward at these shares of the launch speed,
// within section 52's ranges: the sheet opens from about 20 degrees off the vertical at its inner
// row to 50 at its outer.
const CROWN_PER_FRAME:u32=2048u;
const CROWN_ROWS:u32=3u;
fn crown_ring(row:u32)->f32 {return array<f32,3>(1.1,1.3,1.5)[min(row,2u)];}
fn crown_up(row:u32)->f32 {return array<f32,3>(0.85,0.62,0.38)[min(row,2u)];}
fn crown_out(row:u32)->f32 {return array<f32,3>(0.3,0.4,0.45)[min(row,2u)];}
// The fastest a crown's water leaves the water's own motion: its rise then lasts at most 1.5 s,
// its life (g x 1.5 / 2).
const CROWN_LAUNCH:f32=7.36;
// Where a parcel of a crown stands apart from its neighbours along a row, at most, m.
const CROWN_ARC:f32=0.05;
// The crown of one body this advance, which invocation 0 plans and every invocation emits.
struct CrownPlan {
    centre:vec2f, s1:f32, s2:f32, phi:f32, speed:f32,
    flow:vec2f, around:u32, sub:u32, base:u32, granted:u32, weight:f32, key:u32,
}
var<workgroup> went:array<vec4f,64>;
var<workgroup> waterplane:array<f32,64>;
var<workgroup> plan:CrownPlan;
// The volume body `b`'s points pushed aside this advance gives its crown: where (the volume's
// centre), how wide (the ellipse with the second moments of their waterplanes, each a disc), how
// fast (the volume's mean speed) and how much (the volume, heard as before). Then its parcels: a
// row of `around` about each ring, `sub` rows of each born through the advance, at most
// CROWN_PER_FRAME a frame among every region's.
fn plan_crown(b:u32){
    plan.granted=0u;
    let body=crowns.bodies[b];
    var volume=0.0;var centre=vec2f(0.0);var speed=0.0;
    for(var i=body.x;i<body.x+body.y;i++){
        let w=went[i];volume+=w.x;centre+=w.x*w.yz;speed+=w.x*w.w;
    }
    if(volume<=0.0){return;}
    centre/=volume;speed=min(speed/volume,14.0);
    var m=vec3f(0.0);
    for(var i=body.x;i<body.x+body.y;i++){
        let w=went[i];if(w.x<=0.0){continue;}
        let d=w.yz-centre;let disc=waterplane[i]/(4.0*PI);
        m+=w.x*vec3f(d.x*d.x+disc,d.x*d.y,d.y*d.y+disc);
    }
    m/=volume;
    let half=sqrt(max(0.25*(m.x-m.z)*(m.x-m.z)+m.y*m.y,0.0));
    let l1=0.5*(m.x+m.z)+half;let l2=max(0.5*(m.x+m.z)-half,1e-6);
    let key=pcg_hash(body.z^(p.flags.x*2654435769u));
    let water=surface_at(centre);
    // As section 52 had it: the kinetic energy of the water pushed aside at its speed, shared by
    // events in proportion to the volume and the speed.
    let events=volume*8000.0*min(speed*0.25,2.0);
    hear(4u,vec3f(centre.x,water.x,centre.y),events,0.5*1000.0*speed*speed/(8000.0*min(speed*0.25,2.0)),key);
    if(distance(centre,view.camera_pos.xz)>p.rates.w){return;}
    // A uniform ellipse's second moment along an axis is a quarter of its semi-axis squared.
    plan.s1=2.0*sqrt(l1);plan.s2=2.0*sqrt(l2);
    // A round footprint has no axis: one entry point gives m.x == m.z, and m.y is 0 or the residue
    // of the centre's rounding (about 1e-13). atan2 of (0, 0) is π/2 under DXC, undefined in SPIR-V
    // and NaN under FXC, which loses the whole crown; the residue gave ±π/4 on every backend. Below
    // a millionth of the moments the footprint takes the orientation 0.
    let ay=2.0*m.y;let ax=m.x-m.z;
    plan.phi=select(0.0,0.5*atan2(ay,ax),abs(ay)+abs(ax)>1e-6*(m.x+m.z));
    plan.centre=centre;plan.flow=water.zw;plan.key=key;
    plan.speed=min(speed,CROWN_LAUNCH/length(vec2f(crown_up(0u),crown_out(0u))));
    let mean=sqrt(plan.s1*plan.s2);
    plan.around=u32(clamp(ceil(2.0*PI*crown_ring(1u)*mean/CROWN_ARC),16.0,64.0));
    plan.sub=u32(clamp(ceil(crown_up(0u)*plan.speed*p.origin_cell_dt.w/0.04),1.0,4.0));
    let want=plan.around*CROWN_ROWS*plan.sub;
    let crowned=atomicAdd(&budget[2],want);
    let left=CROWN_PER_FRAME-min(crowned,CROWN_PER_FRAME);
    let born=atomicAdd(&budget[0],min(want,left));
    let granted=min(min(want,left),p.flags.w-min(born,p.flags.w));
    plan.base=atomicAdd(&state.head,granted);atomicAdd(&state.born,granted);
    plan.weight=min(events/f32(max(want,1u)),1.0);
    plan.granted=granted;
}
// Parcel `n` of a planned crown: row j about ring j, at its place i of `around`, born `k` of `sub`
// along the advance (its age the time since). Neighbouring rows sit half a place apart, so the
// lattice closes into a sheet.
fn crown_parcel(q:CrownPlan,n:u32){
    let i=n%q.around;let j=(n/q.around)%CROWN_ROWS;let k=n/(q.around*CROWN_ROWS);
    let seed=pcg_hash(q.key^(n*2246822519u));
    let theta=2.0*PI*(f32(i)+0.5*f32((j+k)&1u))/f32(q.around);
    let c=cos(q.phi);let s=sin(q.phi);
    let rot=mat2x2f(c,s,-s,c);
    let rho=crown_ring(j);
    let along=rot*vec2f(q.s1*cos(theta),q.s2*sin(theta));
    let normal=normalize(rot*vec2f(cos(theta)/q.s1,sin(theta)/q.s2));
    let mean=sqrt(q.s1*q.s2);
    let arc=2.0*PI*rho*mean/f32(q.around);
    let xz=q.centre+rho*along+normal*(spray_random(seed)-0.5)*0.2*arc;
    // Each leaves within 2 degrees of its row's way and 4 percent of its speed.
    let turn=(spray_random(seed+1u)-0.5)*0.07;
    let way=vec2f(normal.x*cos(turn)-normal.y*sin(turn),normal.x*sin(turn)+normal.y*cos(turn));
    let gain=q.speed*(1.0+(spray_random(seed+2u)-0.5)*0.08);
    let up=crown_up(j)*gain;let out=crown_out(j)*gain;
    var v=vec3f(way.x*out+q.flow.x,up,way.y*out+q.flow.y);
    let dt=p.origin_cell_dt.w;
    let tau=dt*(f32(k)+0.5)/f32(q.sub);
    let ground=surface_at(xz);
    var at=vec3f(xz.x,ground.x+0.02,xz.y)+v*tau-vec3f(0.0,4.905*tau*tau,0.0);
    v.y-=9.81*tau;
    // Its splat covers its share of the sheet: the arc to its neighbours, or the way to the next
    // row born after it.
    let radius=clamp(0.75*max(arc,up*dt/f32(q.sub)),0.01,0.06);
    var d:Droplet;
    d.position=vec4f(at,tau);d.velocity=vec4f(v,lifetime(CROWN));
    d.info=vec4u(CROWN,seed,q.key,crown_pack(j,radius,q.weight,out/(rho*mean)));
    droplets[(q.base+n)%p.count.w]=d;
}
// A body going in throws a crown (water.md 52): a stirring point whose layer passed down through
// the drawn surface since the feed's last list, at 1.2 m/s or more, pushed aside the water its
// waterplane swept, counted until three of its waterplane's radii are under (the crown is fed as
// the body's front goes in, not as it sinks on); its body's points together make one crown.
@compute @workgroup_size(64)
fn crown(@builtin(local_invocation_index) lane:u32){
    var pushed=vec4f(0.0);var area=0.0;
    if(lane<crowns.count.x&&p.origin_cell_dt.w>0.0){
        let e=crowns.points[lane];
        let size=max(e.was.w,1e-3);let a=e.now.w/size;
        let water=surface_at(e.now.xz);let speed=-e.velocity.y;
        if(water.y>0.05&&speed>=1.2&&inside(e.now.xz)){
            let reach=min(size,3.0*sqrt(a/PI));
            let before=clamp(water.x-(e.was.y-0.5*size),0.0,reach);
            let after=clamp(water.x-(e.now.y-0.5*size),0.0,reach);
            if(after>before){pushed=vec4f(a*(after-before),e.now.x,e.now.z,speed);area=a;}
        }
    }
    went[lane]=pushed;waterplane[lane]=area;
    workgroupBarrier();
    for(var b=0u;b<crowns.count.y;b++){
        if(lane==0u){plan_crown(b);}
        workgroupBarrier();
        let q=plan;
        for(var n=lane;n<q.granted;n+=64u){crown_parcel(q,n);}
        workgroupBarrier();
    }
}
@compute @workgroup_size(8,8)
fn emit(@builtin(global_invocation_id) id:vec3u){
    if(any(id.xy>=p.count.xy)){return;}
    if(p.flags.y==0u&&atomicLoad(&state.sources)==0u){return;}
    face(id.xy,0u);face(id.xy,1u);splash(id.xy);
}
// After the births, the slots of the parcels alive, for the draw to read so that it runs over them
// alone rather than over the whole ring. Listed in slot order, so the draw blends them in the
// same order every frame: `count` tallies each block of 64 slots, then `list` places a block's
// parcels after those of the blocks before it, each after the block's lower slots alive.
var<workgroup> lanes:array<u32,64>;
var<workgroup> before:array<u32,64>;
fn listed(slot:u32)->u32 {
    if(slot>=p.count.w){return 0u;}
    let d=droplets[slot];
    return select(0u,1u,d.velocity.w>0.0&&d.position.w<d.velocity.w);
}
@compute @workgroup_size(64)
fn count(@builtin(workgroup_id) block:vec3u,@builtin(local_invocation_index) lane:u32){
    lanes[lane]=listed(block.x*64u+lane);
    workgroupBarrier();
    for(var step=32u;step>0u;step=step/2u){
        if(lane<step){lanes[lane]+=lanes[lane+step];}
        workgroupBarrier();
    }
    if(lane==0u){state.blocks[block.x]=lanes[0];}
}
@compute @workgroup_size(64)
fn list(@builtin(workgroup_id) block:vec3u,@builtin(local_invocation_index) lane:u32){
    // The parcels of the blocks before this one, and of all of them: each lane sums every 64th
    // block, then the lanes' sums are added up.
    var earlier=0u;var all=0u;
    for(var b=lane;b<LIST_BLOCKS;b+=64u){
        let n=state.blocks[b];all+=n;
        if(b<block.x){earlier+=n;}
    }
    before[lane]=earlier;lanes[lane]=all;
    workgroupBarrier();
    for(var step=32u;step>0u;step=step/2u){
        if(lane<step){before[lane]+=before[lane+step];lanes[lane]+=lanes[lane+step];}
        workgroupBarrier();
    }
    let offset=before[0];
    if(block.x==0u&&lane==0u){atomicStore(&state.draw_instances,lanes[0]);}
    workgroupBarrier();
    // This block's own, an inclusive prefix sum over the lanes (Hillis and Steele's).
    let slot=block.x*64u+lane;
    let mine=listed(slot);
    lanes[lane]=mine;
    workgroupBarrier();
    for(var step=1u;step<64u;step=step*2u){
        let add=select(0u,lanes[max(lane,step)-step],lane>=step);
        workgroupBarrier();
        lanes[lane]+=add;
        workgroupBarrier();
    }
    if(mine==1u){state.drawn[offset+lanes[lane]-1u]=slot;}
}
