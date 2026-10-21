@group(0) @binding(0) var<uniform> view:View;
@group(0) @binding(1) var<storage,read_write> quads:array<u32>;
struct Args { vertices:u32, instances:atomic<u32>, first:u32, instance:u32 }
@group(0) @binding(2) var<storage,read_write> args:Args;
@group(0) @binding(3) var mask_out:texture_storage_2d<r32uint,write>;
@compute @workgroup_size(64)
fn classify(@builtin(global_invocation_id) id:vec3u){
    let width=shallow.count.x-1u;let count=width*(shallow.count.y-1u);if(id.x>=count){return;}
    if(id.x==0u){args.vertices=72u;}
    let q=ledge_quad(vec2i(i32(id.x%width),i32(id.x/width)));
    textureStore(mask_out,vec2i(i32(id.x%width),i32(id.x/width)),vec4u(q.cuts*q.enabled,0u,0u,0u));
    if(q.enabled!=0u){let index=atomicAdd(&args.instances,1u);quads[index]=id.x;}
}
