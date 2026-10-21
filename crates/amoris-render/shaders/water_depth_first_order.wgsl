// First-order depth transport paired with the CPU-corresponding semi-Lagrangian method.
@compute @workgroup_size(8, 8)
fn depth_first_donors(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x; let k = id.y;
    if i >= p.size.x || k >= p.size.y { return; }
    let n = at(i, k);
    var rates=vec4f(max(-vx(i,k),0.0),max(vx(i+1u,k),0.0),max(-vz(i,k),0.0),max(vz(i,k+1u),0.0));
    // A characteristic face has its own interfacial depth: limit its actual volume flux,
    // rather than assuming the boundary flux equals interior depth times face velocity.
    let h=max(src[n].y,0.00000000000000000001);
    if(i==0u&&p.edge_kind.x>=4u){rates.x=max(-edge_flux[k].x,0.0)/h;}
    if(i+1u==p.size.x&&p.edge_kind.y>=4u){rates.y=max(edge_flux[p.size.y+k].x,0.0)/h;}
    if(k==0u&&p.edge_kind.z>=4u){rates.z=max(-edge_flux[2u*p.size.y+i].x,0.0)/h;}
    if(k+1u==p.size.y&&p.edge_kind.w>=4u){rates.w=max(edge_flux[2u*p.size.y+p.size.x+i].x,0.0)/h;}
    let out=(rates.x+rates.y+rates.z+rates.w)*p.dt/p.cell;
    scale[n] = 1.0 / max(out, 1.0);
}
fn depth_first_flow(v: f32, a: u32, b: u32) -> f32 {
    if v > 0.0 { return v * src[a].y * scale[a]; }
    return v * src[b].y * scale[b];
}
@compute @workgroup_size(8, 8)
fn depth_first_transport(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x; let k = id.y;
    if i >= p.size.x || k >= p.size.y { return; }
    let n = at(i, k);
    if awake[n] == 0u { dst[n] = src[n]; return; }
    var net = 0.0;
    if i > 0u { net += depth_first_flow(vx(i, k), n - 1u, n); }else if(p.edge_kind.x!=0u){net+=edge_flux[k].x;}
    if i + 1u < p.size.x { net -= depth_first_flow(vx(i + 1u, k), n, n + 1u); }else if(p.edge_kind.y!=0u){net-=edge_flux[p.size.y+k].x;}
    if k > 0u { net += depth_first_flow(vz(i, k), n - p.size.x, n); }else if(p.edge_kind.z!=0u){net+=edge_flux[p.size.y*2u+i].x;}
    if k + 1u < p.size.y { net -= depth_first_flow(vz(i, k + 1u), n, n + p.size.x); }else if(p.edge_kind.w!=0u){net-=edge_flux[p.size.y*2u+p.size.x+i].x;}
    dst[n] = vec4<f32>(src[n].x, max(src[n].y + net * p.dt / p.cell, 0.0), vec2f(velocity[n*2u],velocity[n*2u+1u]));
}
