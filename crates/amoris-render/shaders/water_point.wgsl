// Pure shared footprint for coupled point sampling and its transpose impulse distribution.
fn water_point_corner(point:vec2f,corner:u32,size:vec2u)->vec3f {
    let q=clamp(point-0.5,vec2f(0.0),vec2f(size-vec2u(1u)));
    let lo=vec2u(floor(q));let hi=min(lo+vec2u(1u),size-vec2u(1u));
    let t=fract(q);let upper=vec2u(corner%2u,corner/2u);
    let at=select(lo,hi,upper==vec2u(1u));
    let weight=select(1.0-t.x,t.x,upper.x==1u)*select(1.0-t.y,t.y,upper.y==1u);
    return vec3f(vec2f(at),weight*0.5);
}
