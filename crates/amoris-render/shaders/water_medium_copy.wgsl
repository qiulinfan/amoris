struct MediumTile { y:u32, rows:u32, stamp:u32, padding:u32 };
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var result: texture_storage_2d<rgba16float,write>;
@group(0) @binding(2) var<uniform> tile: MediumTile;
@compute @workgroup_size(8,8)
fn main(@builtin(global_invocation_id) id:vec3u){
    let size=textureDimensions(source);let pixel=vec2u(id.x,id.y+tile.y);
    if(pixel.x>=size.x || id.y>=tile.rows || pixel.y>=size.y){return;}
    textureStore(result,vec2i(pixel),textureLoad(source,vec2i(pixel),0));
}
