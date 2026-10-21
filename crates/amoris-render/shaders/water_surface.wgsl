// Current full-resolution shading attributes. Geometry LOD never downsamples these values.
@group(0) @binding(0) var normal_depth_out:texture_storage_2d<rgba32float,write>;
@group(0) @binding(1) var flow_height_out:texture_storage_2d<rgba32float,write>;
// Declared for the shared projection helpers, unused by this entry point.
@group(0) @binding(2) var<uniform> view:View;
@compute @workgroup_size(8,8)
fn attributes(@builtin(global_invocation_id) id:vec3u) {
    if(any(id.xy>=shallow.count.xy)){return;}
    let at=vec2i(id.xy);let c=shallow_vertex(at.x,at.y);
    textureStore(normal_depth_out,at,vec4f(shallow_normal(at),c.y));
    textureStore(flow_height_out,at,vec4f(c.zw,length(c.zw),c.x));
}
