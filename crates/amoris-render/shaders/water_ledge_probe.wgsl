// Verification-only queries of the actual medium inside/path functions, not a second model.
@group(0) @binding(9) var<storage,read> probe_queries:array<vec4f>;
@group(0) @binding(10) var<storage,read_write> probe_values:array<vec4f>;
@compute @workgroup_size(64)
fn probe_ledge_medium(@builtin(global_invocation_id) id:vec3u){
    if(id.x>=arrayLength(&probe_queries)/2u){return;}
    let q=probe_queries[id.x*2u];let d=probe_queries[id.x*2u+1u];var path=0.0;
    if(q.w>0.0){path=medium_length(q.xyz,d.xyz,q.w,1e10);}
    probe_values[id.x]=vec4f(f32(medium_inside(q.xyz)),path,0.0,0.0);
}
