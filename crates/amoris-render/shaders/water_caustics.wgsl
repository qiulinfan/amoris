// Caustics under a water region (water_caustics.rs). Every texel of the map over the region sends
// photons from where flat water would refract the sun onto it; each photon is refracted by the
// displayed surface (the field's surface as a smooth cubic B-spline, with the subgrid wavelets the
// surface shading draws) and splats where it lands on the bed. Light gathers under focusing crests
// and thins under troughs; flat water returns exactly 1.
// Photon splatting for water caustics after C. Wyman and S. Davis, "Interactive Image-Space
// Techniques for Approximating Caustics", I3D 2006, and M. Shah, J. Konttinen and S. Pattanaik,
// "Caustics Mapping: An Image-Space Technique for Real-Time Caustics", IEEE TVCG 13(2), 2007; here
// in the height field's own plane rather than the light's image space.
struct Region { origin:vec2f, cell:f32, time:f32, count:vec4u, absorb:vec4f, scatter:vec4f, surround:vec4f, fog:vec4f, optics:vec4f }
struct Map { origin:vec2f, texel:f32, on:f32, size:vec2u, unused:vec2u, sigma:vec4f }
@group(0) @binding(0) var<uniform> view:View;
@group(0) @binding(1) var<uniform> region:Region;
@group(0) @binding(2) var<storage,read> water:array<vec4f>;
@group(0) @binding(3) var<uniform> map:Map;
@group(0) @binding(4) var<storage,read_write> accum:array<atomic<u32>>;
@group(0) @binding(5) var out_map:texture_storage_2d<rgba16float,write>;
// Air over water.
const ETA:f32=0.75;
// A texel's light through flat water, in fixed-point units, and photons along a texel's side.
const UNIT:f32=1024.0;
const SUB:u32=2u;

fn column(ij:vec2i)->vec4f {
    let c=vec2u(clamp(ij,vec2i(0),vec2i(region.count.xy)-1));
    if(region.count.z!=0u){return water[((c.x/16u)+(region.count.x/16u)*(c.y/16u))*256u+c.x%16u+16u*(c.y%16u)];}
    return water[c.x+region.count.x*c.y];
}
fn wet(c:vec4f)->bool {return c.y>0.002&&abs(c.x)<1e10;}
// The water at a point, bilinear: surface (a dry column takes the point's own column's, so walls
// do not tilt the water), bed, and whether the point's own column is wet.
struct Level { surface:f32, bed:f32, wet:bool }
fn level_at(xz:vec2f)->Level {
    let u=(xz-region.origin)/region.cell-0.5;
    let i0=vec2i(floor(u));let t=u-floor(u);
    let home=column(vec2i(round(u)));
    let a=column(i0);let b=column(i0+vec2i(1,0));let c=column(i0+vec2i(0,1));let d=column(i0+vec2i(1,1));
    let s=vec4f(select(home.x,a.x,wet(a)),select(home.x,b.x,wet(b)),select(home.x,c.x,wet(c)),select(home.x,d.x,wet(d)));
    return Level(mix(mix(s.x,s.y,t.x),mix(s.z,s.w,t.x),t.y),
        mix(mix(a.x-a.y,b.x-b.y,t.x),mix(c.x-c.y,d.x-d.y,t.x),t.y),wet(home));
}
fn bspline(t:f32)->vec4f {
    let s=1.0-t;
    return vec4f(s*s*s,3.0*t*t*t-6.0*t*t+4.0,-3.0*t*t*t+3.0*t*t+3.0*t+1.0,t*t*t)/6.0;
}
fn bspline_slope(t:f32)->vec4f {
    let s=1.0-t;
    return vec4f(-s*s,3.0*t*t-4.0*t,-3.0*t*t+2.0*t+1.0,t*t)*0.5;
}
// Where light enters: the surface as a cubic B-spline over the columns (smooth to its curvature,
// which is what focuses light), its slope, and the flow there, bilinear.
struct Entry { surface:f32, slope:vec2f, flow:vec2f }
fn entry_at(xz:vec2f)->Entry {
    let u=(xz-region.origin)/region.cell-0.5;
    let i0=vec2i(floor(u));let t=u-floor(u);
    let home=column(vec2i(round(u)));
    let wx=bspline(t.x);let wz=bspline(t.y);let sx=bspline_slope(t.x);let sz=bspline_slope(t.y);
    var h=0.0;var g=vec2f(0.0);var flow=vec2f(0.0);
    for(var k=0;k<4;k++){
        for(var i=0;i<4;i++){
            let c=column(i0+vec2i(i-1,k-1));
            let s=select(home.x,c.x,wet(c));
            h+=s*wx[i]*wz[k];g+=s*vec2f(sx[i]*wz[k],wx[i]*sz[k]);
            if(i==1||i==2){if(k==1||k==2){flow+=c.zw*select(1.0-t.x,t.x,i==2)*select(1.0-t.y,t.y,k==2);}}
        }
    }
    return Entry(h,g/region.cell,flow);
}
// forward.wgsl `wavelets`, its constants tabulated per wave: direction, wavenumber and angular
// frequency; phase, amplitude and wavelength.
const WAVELETS=array<vec4f,28>(
    vec4f(0.95533651,0.29552022,2.02683401,4.45906305), vec4f(3.10081434,0.05000000,3.09999990,0.0),
    vec4f(-0.90405625,0.42741346,2.73896503,5.18355560), vec4f(0.84640449,0.04000000,2.29399991,0.0),
    vec4f(0.37790921,-0.92584264,3.70130396,6.02576065), vec4f(0.51029199,0.03200000,1.69755995,0.0),
    vec4f(0.34673908,0.93796164,5.00176191,7.00480413), vec4f(1.77788448,0.02560000,1.25619435,0.0),
    vec4f(-0.88925892,-0.45740411,6.75913763,8.14291954), vec4f(1.89509261,0.02048000,0.92958385,0.0),
    vec4f(0.96468395,-0.26341009,9.13397026,9.46595192), vec4f(1.60486984,0.01638400,0.68789202,0.0),
    vec4f(-0.53339708,0.84586495,12.34320259,11.00394535), vec4f(0.25718620,0.01310720,0.50904012,0.0),
    vec4f(-0.17806396,-0.98401892,16.68000412,12.79182720), vec4f(4.67848158,0.01048576,0.37668967,0.0),
    vec4f(0.79599512,0.60530305,22.54054642,14.87019730), vec4f(2.41009307,0.00838861,0.27875036,0.0),
    vec4f(-0.99581838,0.09135519,30.46019745,17.28625298), vec4f(4.05212879,0.00671089,0.20627527,0.0),
    vec4f(0.67257607,-0.74002802,41.16242981,20.09486008), vec4f(4.42155313,0.00536871,0.15264370,0.0),
    vec4f(0.00394495,0.99999219,55.62490463,23.35980034), vec4f(4.27069426,0.00429497,0.11295634,0.0),
    vec4f(-0.67839384,-0.73469847,75.16878510,27.15521622), vec4f(3.82072544,0.00343597,0.08358769,0.0),
    vec4f(0.99650830,0.08349359,101.57943726,31.56729698), vec4f(3.75161171,0.00274878,0.06185489,0.0));
fn wavelet_slopes(p:vec2f,t:f32,footprint:f32)->vec2f {
    var g=vec2f(0.0);
    for(var i=0u;i<14u;i++){
        let a=WAVELETS[2u*i];let b=WAVELETS[2u*i+1u];
        let keep=clamp(b.z/(footprint*4.0)-1.0,0.0,1.0);
        if(keep>0.0){g+=b.y*a.z*cos(a.z*dot(a.xy,p)-a.w*t+b.x)*a.xy*keep;}
    }
    return g;
}
// The surface shading's wavelets (forward.wgsl fs_shallow), advected by the flow in two phases.
fn detail(xz:vec2f,flow:vec2f)->vec2f {
    let phase=fract(region.time*0.2);let blend=abs(phase*2.0-1.0);
    let q0=xz-flow*phase*5.0;let q1=xz-flow*fract(phase+0.5)*5.0;
    let ripple=clamp(length(flow)*0.5,0.15,1.0)*region.optics.z;
    return mix(wavelet_slopes(q0,region.time,map.texel),wavelet_slopes(q1,region.time,map.texel),blend)*ripple*0.5;
}
@compute @workgroup_size(8,8)
fn photons(@builtin(global_invocation_id) id:vec3u){
    if(any(id.xy>=map.size*SUB)){return;}
    // The point under water this photon would light through a flat surface, and where it enters.
    let goal=map.origin+(vec2f(id.xy)+0.5)/f32(SUB)*map.texel;
    let here=level_at(goal);
    let depth=here.surface-here.bed;
    if(!here.wet||depth<=0.0){return;}
    let flat_ray=refract(-view.sun_dir.xyz,vec3f(0.0,1.0,0.0),ETA);
    let at=goal-flat_ray.xz/(-flat_ray.y)*depth;
    let e=entry_at(at);
    let g=e.slope+detail(at,e.flow);
    let ray=refract(-view.sun_dir.xyz,normalize(vec3f(-g.x,1.0,-g.y)),ETA);
    if(ray.y>-0.05){return;}
    let land=at+ray.xz/(-ray.y)*max(e.surface-here.bed,0.0);
    let q=(land-map.origin)/map.texel-0.5;
    let i0=vec2i(floor(q));let t=q-floor(q);
    let energy=UNIT/f32(SUB*SUB);
    for(var c=0u;c<4u;c++){
        let corner=vec2i(i32(c%2u),i32(c/2u));let cell=i0+corner;
        if(any(cell<vec2i(0))||any(cell>=vec2i(map.size))){continue;}
        let w=select(1.0-t.x,t.x,corner.x==1)*select(1.0-t.y,t.y,corner.y==1);
        let units=u32(round(energy*w));
        if(units!=0u){atomicAdd(&accum[u32(cell.x)+map.size.x*u32(cell.y)],units);}
    }
}
// Each texel: x the light it gathered over flat water's, y the depth to the bed, w the surface
// (a dry texel: 1, 0, and w far below anything). Resolving clears the sums for the next frame.
@compute @workgroup_size(8,8)
fn resolve(@builtin(global_invocation_id) id:vec3u){
    if(any(id.xy>=map.size)){return;}
    let i=id.x+map.size.x*id.y;
    let units=atomicExchange(&accum[i],0u);
    let here=level_at(map.origin+(vec2f(id.xy)+0.5)*map.texel);
    var out=vec4f(1.0,0.0,0.0,-1e4);
    if(here.wet&&here.surface>here.bed){
        out=vec4f(min(f32(units)/UNIT,16.0),here.surface-here.bed,0.0,here.surface);
    }
    textureStore(out_map,vec2i(id.xy),out);
}
