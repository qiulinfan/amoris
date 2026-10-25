struct SprayParams { count:vec4u, origin_cell_dt:vec4f, flags:vec4u, rates:vec4f, absorb:vec4f, entry:vec4f }
struct Droplet { position:vec4f, velocity:vec4f, info:vec4u }
@group(0) @binding(0) var<uniform> view:View;
@group(0) @binding(1) var<uniform> p:SprayParams;
@group(0) @binding(2) var<storage,read> droplets:array<Droplet>;
struct Out { @builtin(position) clip:vec4f, @location(0) uv:vec2f, @location(1) color:vec4f }
// Each kind's look: drawn radius (metres, at birth), its growth over life, opacity, and how far
// speed stretches it. A falling sheet's water is drawn as large overlapping blobs so a curtain
// reads as a sheet; mist is a few soft, faint, growing puffs; an entry's crown is dense blobs
// stretched along their flight.
fn kind_look(kind:u32)->vec4f {
    return array<vec4f,7>(
        vec4f(0.075,-0.3,0.42,3.0),
        vec4f(0.026,-0.45,0.3,2.0),
        vec4f(0.026,-0.45,0.3,2.0),
        vec4f(0.04,-0.4,0.5,2.0),
        vec4f(0.07,-0.2,0.55,3.0),
        vec4f(0.7,1.6,0.07,0.0),
        vec4f(0.065,-0.3,0.75,2.5))[min(kind,6u)];
}
@vertex
fn vs(@builtin(vertex_index) vi:u32,@builtin(instance_index) ii:u32)->Out {
    var out:Out;let d=droplets[ii];
    if(d.velocity.w<=0.0||d.position.w>=d.velocity.w){out.clip=vec4f(2.0,2.0,2.0,1.0);return out;}
    let corners=array<vec2f,6>(vec2f(-1.0,-1.0),vec2f(1.0,-1.0),vec2f(1.0,1.0),vec2f(-1.0,-1.0),vec2f(1.0,1.0),vec2f(-1.0,1.0));let corner=corners[vi];
    let right=vec3f(view.view[0].x,view.view[1].x,view.view[2].x);let up=vec3f(view.view[0].y,view.view[1].y,view.view[2].y);
    let screen=vec2f(dot(d.velocity.xyz,right),dot(d.velocity.xyz,up));let direction=select(vec2f(0.0,1.0),screen/max(length(screen),0.00001),length(screen)>0.01);
    let look=kind_look(d.info.x);
    let t=d.position.w/d.velocity.w;
    let jitter=f32(pcg_hash(d.info.y)>>8u)/16777216.0;
    let radius=look.x*(0.7+0.6*jitter)*max(1.0+look.y*t,0.05);
    let stretch=1.0+min(length(d.velocity.xyz)*0.08,look.w);
    let offset=(right*direction.x+up*direction.y)*corner.y*radius*stretch+(right*(-direction.y)+up*direction.x)*corner.x*radius;
    out.clip=view.view_proj*vec4f(d.position.xyz+offset,1.0);out.uv=corner;
    let white=select(smoothstep(3.0,7.0,length(d.velocity.xyz)),0.9,d.info.x>=3u);
    // Sky light and the sun on a small translucent drop, in the forward pass's units (the sun's
    // irradiance over pi, as a diffuse surface sees it).
    let light=vec3f(max(view.sun_color.w,0.15)*0.5)+view.sun_color.rgb*view.sun_dir.w*(0.2+0.45*max(view.sun_dir.y,0.0))/3.14159265;
    // A drop a few metres from the eye is drawn as a disc many pixels wide, where a real one is a
    // blurred streak: drops fade out from 4 m plus six radii to 1 m plus one, so a surfer or a
    // swimmer in the white water sees the spray beyond them rather than grey blobs around them.
    let near=smoothstep(1.0+radius,4.0+6.0*radius,distance(d.position.xyz,view.camera_pos.xyz));
    let alpha=select((0.12+0.45*white)*(1.0-t),look.z*(1.0-t),d.info.x==0u||d.info.x>=3u)*near;
    out.color=vec4f(mix(vec3f(0.6,0.75,0.85),vec3f(0.95),white)*exp(-p.absorb.xyz*(2.0*min(radius,0.1)))*light,alpha);return out;
}
@fragment
fn fs(in:Out)->@location(0) vec4f {
    let r=dot(in.uv,in.uv);if(r>1.0){discard;}
    let a=in.color.a*(1.0-r)*(1.0-r);return vec4f(in.color.rgb*a,a);
}
