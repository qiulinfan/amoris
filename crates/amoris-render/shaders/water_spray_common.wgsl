// What the spray's compute and draw passes and the liquid surface share (water_spray.rs `source`,
// fluid_surface.rs).
struct SprayParams { count:vec4u, origin_cell_dt:vec4f, flags:vec4u, rates:vec4f, absorb:vec4f, sound:vec4f }
struct Droplet { position:vec4f, velocity:vec4f, info:vec4u }
// An entry crown's parcel (kind 6, water.md 52) carries in `info.w` its row of the crown's lattice
// (bits 0-1), its radius at birth in millimetres (2-7), the share of a parcel's water it stands
// for in 255ths (8-15: its landing's foam and sound) and how fast the sheet about it stretches, in
// quarters of a reciprocal second (16-23).
struct CrownParcel { row:u32, radius_m:f32, weight:f32, stretch_rate:f32 }
fn crown_pack(row:u32,radius_m:f32,weight:f32,stretch_rate:f32)->u32 {
    let mm=u32(clamp(round(radius_m*1000.0),1.0,63.0));
    let w=u32(round(clamp(weight,0.0,1.0)*255.0));
    let s=u32(clamp(round(stretch_rate*4.0),0.0,255.0));
    return (row&3u)|(mm<<2u)|(w<<8u)|(s<<16u);
}
fn crown_unpack(bits:u32)->CrownParcel {
    var c:CrownParcel;
    c.row=bits&3u;
    c.radius_m=f32((bits>>2u)&63u)*0.001;
    c.weight=f32((bits>>8u)&255u)/255.0;
    c.stretch_rate=f32((bits>>16u)&255u)*0.25;
    return c;
}
// How far the sheet about a crown's parcel has stretched `age` seconds after birth: the ring it left
// on, widening, over that ring at birth.
fn crown_stretch(c:CrownParcel,age:f32)->f32 {return 1.0+c.stretch_rate*age;}
// A crown's sheet tears into drops as it stretches from this to this times its ring at birth.
const CROWN_TEARS:vec2f=vec2f(2.25,2.6);
// A crown's parcel's radius `age` seconds after birth: its share of the sheet, widening as the ring
// stretches (thinning it) until it tears.
fn crown_radius(c:CrownParcel,age:f32)->f32 {
    let stretch=crown_stretch(c,age);
    return c.radius_m*sqrt(stretch)*(1.0-smoothstep(CROWN_TEARS.x,CROWN_TEARS.y,stretch));
}
// The share of a crown's parcel the liquid surface draws (fluid_surface.wgsl), the rest drawn as a
// streak (water_spray_draw.wgsl): none under 1.5 pixels across, all over 2.5, `pixel` metres a
// pixel at its distance. Perspective views only: the surface draws in no other.
fn crown_share(radius_m:f32,pixel:f32)->f32 {
    if(view.proj[3][3]>0.5){return 0.0;}
    return smoothstep(1.5,2.5,2.0*radius_m/pixel);
}
// The share of a crown's water full of air (white), drawn per parcel from its seed (`info.y`).
fn crown_air(seed:u32)->f32 {return mix(0.1,0.4,unit_from_hash(pcg_hash(pcg_hash(seed^0x68bc21ebu))));}
