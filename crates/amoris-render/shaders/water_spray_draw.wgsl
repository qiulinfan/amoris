struct SprayParams { count:vec4u, origin_cell_dt:vec4f, flags:vec4u, rates:vec4f, absorb:vec4f }
struct Droplet { position:vec4f, velocity:vec4f, info:vec4u }
@group(0) @binding(0) var<uniform> view:View;
@group(0) @binding(1) var<uniform> p:SprayParams;
@group(0) @binding(2) var<storage,read> droplets:array<Droplet>;
struct Out { @builtin(position) clip:vec4f, @location(0) uv:vec2f, @location(1) color:vec4f }
@vertex
fn vs(@builtin(vertex_index) vi:u32,@builtin(instance_index) ii:u32)->Out {
    var out:Out;let d=droplets[ii];
    if(d.velocity.w<=0.0||d.position.w>=d.velocity.w){out.clip=vec4f(2.0,2.0,2.0,1.0);return out;}
    let corners=array<vec2f,6>(vec2f(-1.0,-1.0),vec2f(1.0,-1.0),vec2f(1.0,1.0),vec2f(-1.0,-1.0),vec2f(1.0,1.0),vec2f(-1.0,1.0));let corner=corners[vi];
    let right=vec3f(view.view[0].x,view.view[1].x,view.view[2].x);let up=vec3f(view.view[0].y,view.view[1].y,view.view[2].y);
    let screen=vec2f(dot(d.velocity.xyz,right),dot(d.velocity.xyz,up));let direction=select(vec2f(0.0,1.0),screen/max(length(screen),0.00001),length(screen)>0.01);
    let t=d.position.w/d.velocity.w;let radius=(0.018+0.016*f32(pcg_hash(d.info.y)>>8u)/16777216.0)*(1.0-0.45*t);
    let stretch=1.0+min(length(d.velocity.xyz)*0.08,2.0);
    let offset=(right*direction.x+up*direction.y)*corner.y*radius*stretch+(right*(-direction.y)+up*direction.x)*corner.x*radius;
    out.clip=view.view_proj*vec4f(d.position.xyz+offset,1.0);out.uv=corner;
    let white=smoothstep(3.0,7.0,length(d.velocity.xyz));let light=vec3f(max(view.sun_color.w,0.15)*0.7)+view.sun_color.rgb*min(view.sun_dir.w/10000.0,8.0)*max(view.sun_dir.y,0.0)*0.25;
    out.color=vec4f(mix(vec3f(0.6,0.75,0.85),vec3f(0.95),white)*exp(-p.absorb.xyz*(2.0*radius))*light,(0.12+0.45*white)*(1.0-t));return out;
}
@fragment
fn fs(in:Out)->@location(0) vec4f {
    let r=dot(in.uv,in.uv);if(r>1.0){discard;}
    let a=in.color.a*(1.0-r)*(1.0-r);return vec4f(in.color.rgb*a,a);
}
