// Spray as water (water.md 69). A parcel of the compute pass's spray is drawn as three things:
// - a ligament, the sheet or thread of water the parcel carries drawn out behind it along its
//   flight, its radius log-uniform between the kind's bounds, white where its water is full of air,
//   breaking up into beads and drops as it flies;
// - up to SPRAY_SPRITES - 2 drops shed from it, their radii from a truncated power law, each a
//   streak over a frame's time as opaque as the share of the pixel it covers;
// - where a fall lands, a cloud of the drops too fine to draw, a Gaussian whose optical depth thins
//   as it spreads.
// All are lit as water scatters light: the sky behind seen through the water, and the sun scattered
// mostly forward (Henyey and Greenstein's phase function), so spray glows against the sun; white
// water is lit as a white surface; the sun's shadow at the parcel darkens all of it. The parcels
// are drawn in slot order (the compute pass's list), the same every frame.
@group(0) @binding(0) var<uniform> view:View;
@group(0) @binding(1) var<uniform> p:SprayParams;
@group(0) @binding(2) var<storage,read> droplets:array<Droplet>;
@group(0) @binding(3) var<uniform> sh:array<vec4f,9>;
@group(0) @binding(4) var shadow_map:texture_depth_2d_array;
@group(0) @binding(5) var shadow_sampler:sampler_comparison;
// The compute pass's state: its counters and indirect arguments, then the slots of the parcels
// alive this frame, one an instance.
struct SprayList { header:array<u32,8>, drawn:array<u32> }
@group(0) @binding(6) var<storage,read> list:SprayList;
struct Out {
    @builtin(position) clip:vec4f,
    // A ligament's or a drop's place on its streak, -1 to 1 across and 0 to 1 from tail to head; a
    // cloud's in standard deviations of its drops' places.
    @location(0) uv:vec2f,
    // The sprite's light.
    @location(1) color:vec3f,
    // x: the sprite's opacity (a cloud's optical depth through its middle); y: 0 a drop, 1 a
    // cloud, 2 a ligament; z: 1 where a ligament is resolved (over a few pixels across); w: a
    // ligament's length over its width.
    @location(2) look:vec4f,
    // A ligament's erosion (0 whole, rising as it breaks up) and the offset of its noise.
    @location(3) @interpolate(flat) shape:vec2f,
}
// A frame's time: what the eye sees a fast drop smeared over on a display drawing 60 a second.
const SMEAR_S:f32=1.0/60.0;
// How far behind the parcel its ligament reaches, in seconds of its flight.
const LIGAMENT_S:f32=1.0/45.0;
// The share of the sun's light a drop reflects at its surface, about evenly all around; the rest
// passes through it, bent forward.
const REFLECTED:f32=0.06;
// Each kind (0 a falling sheet's water, 1 spray thrown where it lands, 2 rapids, 3 a splash, 4 a
// breaking lip, 5 mist, 6 an entry's crown): its ligament's smallest and largest radius at birth
// (m, zero for none; a crown's parcel carries its own), the share of its water full of air (white)
// at least and at most.
fn kind_ligament(kind:u32)->vec4f {
    return array<vec4f,7>(
        vec4f(0.02,0.05,0.3,0.7),
        vec4f(0.006,0.025,0.5,0.95),
        vec4f(0.006,0.02,0.5,0.9),
        vec4f(0.01,0.035,0.4,0.9),
        vec4f(0.015,0.05,0.5,0.9),
        vec4f(0.0,0.0,0.0,0.0),
        vec4f(0.01,0.06,0.1,0.4))[min(kind,6u)];
}
// The seconds a kind's ligament takes to break up into drops: its volume goes at an even rate (a
// crown's tears as it stretches instead).
fn kind_breakup(kind:u32)->f32 {
    return array<f32,7>(0.8,0.6,0.5,0.35,0.4,1.0,0.35)[min(kind,6u)];
}
// A cloud's widest spread (m), and the optical depth at its sprite's edge.
const MIST_SPREAD_M:f32=0.8;
const CLOUD_EDGE:f32=0.01;
// How many of the parcel's drops a drawn drop stands for: its coverage is that many drops'.
const SAMPLED:f32=8.0;
// Its drops: x how many are drawn, y and z their smallest and largest radius (mm), w how fast they
// spread from the ligament (m/s).
fn kind_drops(kind:u32)->vec4f {
    return array<vec4f,7>(
        vec4f(12.0,0.4,3.0,0.3),
        vec4f(16.0,0.3,2.5,0.6),
        vec4f(12.0,0.3,2.5,0.6),
        vec4f(16.0,0.3,2.5,0.5),
        vec4f(20.0,0.3,3.0,0.5),
        vec4f(0.0,0.0,0.0,0.4),
        vec4f(20.0,0.3,3.0,0.5))[min(kind,6u)];
}
// Its cloud of fine drops: x its spread at birth (m, the standard deviation of their places), y
// its optical depth through the middle there, z how fast it spreads (m/s), w the asymmetry of its
// scattering (finer drops scatter more forward). Only a fall's mist has one: elsewhere it would show
// little, and a cloud for every parcel where a fall lands cost the river falls 4.6 ms of the M5's
// frame (liquids.md 21).
fn kind_cloud(kind:u32)->vec4f {
    return array<vec4f,7>(
        vec4f(0.08,0.0,0.4,0.8),
        vec4f(0.08,0.0,0.5,0.8),
        vec4f(0.08,0.0,0.5,0.8),
        vec4f(0.08,0.0,0.4,0.8),
        vec4f(0.1,0.0,0.4,0.8),
        vec4f(0.4,0.06,0.25,0.75),
        vec4f(0.08,0.0,0.5,0.8))[min(kind,6u)];
}
fn sh_irradiance(n:vec3f)->vec3f {
    let c1=0.429043;let c2=0.511664;let c3=0.743125;let c4=0.886227;let c5=0.247708;
    return max(vec3f(0.0),
        c1*sh[8].xyz*(n.x*n.x-n.y*n.y)+c3*sh[6].xyz*n.z*n.z+c4*sh[0].xyz
        -c5*sh[6].xyz+2.0*c1*(sh[4].xyz*n.x*n.y+sh[7].xyz*n.x*n.z+sh[5].xyz*n.y*n.z)
        +2.0*c2*(sh[3].xyz*n.x+sh[1].xyz*n.y+sh[2].xyz*n.z));
}
// The sun's shadow at a point, one comparison tap of its cascade (water_medium.wgsl
// `medium_shadow`).
fn spray_shadow(world:vec3f)->f32 {
    let view_depth=-(view.view*vec4f(world,1.0)).z;
    if(view.sky_color.w<0.5||view_depth<0.0){return 1.0;}
    var c=0u;
    for(var i=0u;i<4u;i++){if(view_depth>view.cascade_splits[i]){c=i+1u;}}
    if(c>=4u){return 1.0;}
    let q=view.cascades[c]*vec4f(world,1.0);
    let uv=q.xy*vec2f(0.5,-0.5)+0.5;
    if(any(uv<vec2f(0.0))||any(uv>vec2f(1.0))||q.z>1.0){return 1.0;}
    return textureSampleCompareLevel(shadow_map,shadow_sampler,uv,c,q.z-0.0005);
}
// Henyey and Greenstein's phase function, per steradian.
fn phase(g:f32,cosine:f32)->f32 {
    let d=1.0+g*g-2.0*g*cosine;
    return (1.0-g*g)/(4.0*PI*d*sqrt(d));
}
// The radiance of clear water drops scattering toward the eye along `to_eye`: what lies behind
// them seen through them (the sky's irradiance about the way back, over pi), what they reflect of
// the sky above, and the sun's irradiance `sun` scattered by the phase function. White water,
// full of air (`white` 1), scatters many times: lit by the sky above and the sun as a white
// surface would be, about evenly in every direction.
fn drop_light(to_eye:vec3f,sun:vec3f,g:f32,white:f32)->vec3f {
    let ambient=view.sun_color.w/PI;
    let above=sh_irradiance(vec3f(0.0,1.0,0.0))*ambient;
    let sky=mix((1.0-REFLECTED)*sh_irradiance(-to_eye)*ambient+REFLECTED*above,above,white);
    let cosine=dot(-view.sun_dir.xyz,to_eye);
    let scattered=mix((1.0-REFLECTED)*phase(g,cosine)+REFLECTED/(4.0*PI),max(view.sun_dir.y,0.0)/PI,white);
    return sky+sun*scattered;
}
fn hidden()->Out {var o:Out;o.clip=vec4f(2.0,2.0,2.0,1.0);return o;}
// A streak's frame: along `path` (or up the screen when it hardly moves) and across it facing the
// eye.
fn streak_axes(path:vec3f,to_eye:vec3f)->array<vec3f,2> {
    let up=vec3f(view.view[0].y,view.view[1].y,view.view[2].y);
    let length_m=length(path);
    let along=select(up,path/max(length_m,1e-6),length_m>1e-5);
    var across=cross(along,to_eye);
    if(dot(across,across)<1e-8){across=vec3f(view.view[0].x,view.view[1].x,view.view[2].x);}
    return array<vec3f,2>(normalize(across),along);
}
@vertex
fn vs(@builtin(vertex_index) vi:u32,@builtin(instance_index) ii:u32)->Out {
    let d=droplets[list.drawn[ii]];
    if(d.velocity.w<=0.0||d.position.w>=d.velocity.w){return hidden();}
    let sprite=vi/6u;
    let corner=array<vec2f,6>(vec2f(-1.0,0.0),vec2f(1.0,0.0),vec2f(1.0,1.0),vec2f(-1.0,0.0),vec2f(1.0,1.0),vec2f(-1.0,1.0))[vi%6u];
    let kind=d.info.x;
    let ligament=kind_ligament(kind);let drops=kind_drops(kind);let cloud=kind_cloud(kind);
    let age=d.position.w;let life=age/d.velocity.w;
    let ending=1.0-smoothstep(0.85,1.0,life);
    let to_eye_m=view.camera_pos.xyz-d.position.xyz;
    let far=length(to_eye_m);let to_eye=to_eye_m/max(far,1e-4);
    let sun=view.sun_color.rgb*view.sun_dir.w*spray_shadow(d.position.xyz);
    let pixel=2.0*far/(view.proj[1][1]*view.viewport.y);
    let seed=pcg_hash(d.info.y^0x68bc21ebu);
    let crown=crown_unpack(d.info.w);let stretch=crown_stretch(crown,age);
    let white=select(mix(ligament.z,ligament.w,unit_from_hash(pcg_hash(seed))),crown_air(d.info.y),kind==6u);
    var o:Out;
    if(sprite==1u){
        // The cloud: the optical depth through its middle falls as the spread's square grows.
        // Its spread stops at MIST_SPREAD_M: wider, it costs fill and shows nothing.
        let spread=min(cloud.x+cloud.z*age,MIST_SPREAD_M);
        // Faded right at the eye, so a swimmer in white water is not inside a fog of sprites.
        let fade=smoothstep(spread,0.5+4.0*spread,far)*ending;
        let depth=cloud.y*(cloud.x/spread)*(cloud.x/spread)*fade;
        if(depth<=CLOUD_EDGE){return hidden();}
        // The sprite reaches out to where the optical depth falls to CLOUD_EDGE.
        let half=sqrt(2.0*log(depth/CLOUD_EDGE));
        let q=vec2f(corner.x,corner.y*2.0-1.0)*half;
        let right=vec3f(view.view[0].x,view.view[1].x,view.view[2].x);
        let up=vec3f(view.view[0].y,view.view[1].y,view.view[2].y);
        o.clip=view.view_proj*vec4f(d.position.xyz+(right*q.x+up*q.y)*spread,1.0);
        o.uv=q;
        o.look=vec4f(depth,1.0,0.0,0.0);
        // Light scatters many times among a cloud's drops before it leaves (a mist is far deeper
        // along most paths through it than through its middle across): lit mostly as white water.
        o.color=drop_light(to_eye,sun,cloud.w,max(1.0-exp(-depth),0.6));
        return o;
    }
    if(sprite==0u){
        if(ligament.y<=0.0){return hidden();}
        // The ligament: water thrown off a surface leaves as sheets and threads drawn out along
        // its flight, here behind the parcel over LIGAMENT_S of its path. Its radius is
        // log-uniform between the kind's bounds, its volume shed as drops at an even rate until it
        // has broken up. A crown's parcel is its share of a sheet: its radius covers its share,
        // which widens as the sheet stretches (thinning it), until it tears; wide enough on the
        // screen, the liquid surface draws it instead (`crown_share`).
        var r:f32;
        var handed=0.0;
        if(kind==6u){
            r=crown_radius(crown,age);
            if(r<=0.0){return hidden();}
            handed=crown_share(r,pixel);
        }else{
            let left=1.0-age/kind_breakup(kind);
            if(left<=0.0){return hidden();}
            r=ligament.x*pow(ligament.y/ligament.x,unit_from_hash(seed))*pow(left,1.0/3.0);
        }
        let path=d.velocity.xyz*LIGAMENT_S;
        let axes=streak_axes(path,to_eye);
        let length_m=length(path)+2.0*r;
        let width=max(2.0*r,1.5*pixel);
        // Clear water is seen through but for what it reflects and bends; white water hides
        // what is behind it.
        let opacity=(2.0*r/width)*mix(0.4,0.9,white)*(1.0-handed);
        let fade=ending*smoothstep(0.15,0.4,far);
        if(opacity*fade<=1e-4){return hidden();}
        let world=d.position.xyz+axes[1]*((corner.y-1.0)*length_m+r)+axes[0]*(corner.x*0.5*width);
        o.clip=view.view_proj*vec4f(world,1.0);
        o.uv=corner;
        o.look=vec4f(opacity*fade,2.0,smoothstep(2.0,4.0,width/pixel),length_m/(2.0*r));
        o.color=drop_light(to_eye,sun,0.85,white)*exp(-p.absorb.xyz*2.0*r);
        let erosion=select(age/kind_breakup(kind),smoothstep(1.0,CROWN_TEARS.y,stretch),kind==6u);
        o.shape=vec2f(0.1+0.6*erosion,97.0*unit_from_hash(pcg_hash(seed+1u)));
        return o;
    }
    let k=sprite-2u;
    // A crown's parcel sheds as many drops as the water it stands for.
    let shown=select(drops.x,round(drops.x*crown.weight),kind==6u);
    if(f32(k)>=shown){return hidden();}
    let h0=pcg_hash(d.info.y^((k+1u)*2654435761u));let h1=pcg_hash(h0);let h2=pcg_hash(h1);let h3=pcg_hash(h2);
    // Shed from the ligament: a way uniform over the sphere, at a speed uniform over the ball of
    // the kind's spread, since the parcel left the water.
    let z=2.0*unit_from_hash(h0)-1.0;let phi=2.0*PI*unit_from_hash(h1);
    let way=vec3f(sqrt(max(1.0-z*z,0.0))*cos(phi),z,sqrt(max(1.0-z*z,0.0))*sin(phi));
    let spread=way*pow(unit_from_hash(h2),1.0/3.0)*drops.w;
    let at=d.position.xyz+spread*age;
    let velocity=d.velocity.xyz+spread;
    // Its radius: a truncated power law, as many drops of each radius as the radius to the -3
    // (the inverse of its distribution; r0 and r1 the bounds).
    let a=1.0/(drops.y*drops.y);let b=1.0/(drops.z*drops.z);
    let r=1e-3/sqrt(a+unit_from_hash(h3)*(b-a));
    let path=velocity*SMEAR_S;
    let length_m=length(path);
    let drop_far=distance(view.camera_pos.xyz,at);
    let axes=streak_axes(path,(view.camera_pos.xyz-at)/max(drop_far,1e-4));
    // A drop narrower than a pixel is drawn a pixel and a half wide, as faint as it covers of it.
    let drop_pixel=2.0*drop_far/(view.proj[1][1]*view.viewport.y);
    let width=max(2.0*r,1.5*drop_pixel);
    let coverage=SAMPLED*(2.0*r/width)*(2.0*r/(length_m+2.0*r));
    // Shed as the ligament breaks up: the k-th drop shows once it has lost its share; a crown's
    // as its sheet tears.
    let shed=select(smoothstep(0.0,0.05,age-kind_breakup(kind)*f32(k)/max(drops.x,1.0)),
        smoothstep(CROWN_TEARS.x,CROWN_TEARS.y,stretch),kind==6u);
    let fade=ending*shed*smoothstep(0.1,0.3,drop_far);
    if(coverage*fade<=1e-4){return hidden();}
    let world=at-path*(1.0-corner.y)+axes[1]*((corner.y*2.0-1.0)*r)+axes[0]*(corner.x*0.5*width);
    o.clip=view.view_proj*vec4f(world,1.0);
    o.uv=vec2f(corner.x,corner.y);
    o.look=vec4f(coverage*fade,0.0,0.0,0.0);
    o.color=drop_light(to_eye,sun,0.85,0.0)*exp(-p.absorb.xyz*2.0*r);
    return o;
}
// Worley's cell noise: the distance from `q` to the nearest of one random point per unit cell,
// at most 1.
fn cells(q:vec2f)->f32 {
    let i=floor(q);let f=q-i;var d=1.0;
    for(var y=-1;y<=1;y++){
        for(var x=-1;x<=1;x++){
            let c=vec2i(i)+vec2i(x,y);
            let h=pcg_hash((bitcast<u32>(c.x)*73856093u)^(bitcast<u32>(c.y)*19349663u));
            let o=vec2f(unit_from_hash(h),unit_from_hash(pcg_hash(h)));
            d=min(d,length(vec2f(f32(x),f32(y))+o-f));
        }
    }
    return d;
}
@fragment
fn fs(in:Out)->@location(0) vec4f {
    var a:f32;var color=in.color;
    if(in.look.y>1.5){
        // A ligament: a capsule a width across and `look.w` widths long, its edge where the
        // distance from its axis (in half widths) reaches 1. It breaks up as a jet does, into
        // beads about 4.5 widths apart (Rayleigh's fastest-growing wavelength, 9.02 radii): cell
        // noise stretched along it narrows it, more as the
        // parcel breaks up, until it parts. Under a few pixels across, it parts along its length
        // only (its width is not resolved, its length is).
        let widths=in.look.w;
        let a_w=in.uv.y*widths;
        let end=max(abs(a_w-0.5*widths)-(0.5*widths-0.5),0.0)*2.0;
        let dist=length(vec2f(in.uv.x,end));
        let froth=1.0-cells(vec2f(in.uv.x*0.5,a_w/4.5)+in.shape.y);
        let edge=(0.55+0.45*froth)*(1.0-in.shape.x);
        let soft=1.0-smoothstep(0.5*edge,edge,dist);
        let beads=1.0-cells(vec2f(0.0,a_w/4.5)+in.shape.y);
        let parted=smoothstep(in.shape.x-0.1,in.shape.x+0.1,0.55+0.45*beads);
        let thin=parted*max(1.0-dist*dist,0.0)*1.5*(1.0-0.5*in.shape.x);
        let torn=mix(thin,soft,in.look.z);
        if(torn<=0.0){discard;}
        a=min(in.look.x*torn,1.0);
    }else if(in.look.y>0.5){
        // A cloud: the optical depth through a Gaussian of drops.
        a=1.0-exp(-in.look.x*exp(-0.5*dot(in.uv,in.uv)));
    }else{
        // A drop's streak: across it a profile averaging one, so the streak keeps its coverage.
        a=min(in.look.x*1.5*(1.0-in.uv.x*in.uv.x),1.0);
    }
    return vec4f(color*a,a);
}
