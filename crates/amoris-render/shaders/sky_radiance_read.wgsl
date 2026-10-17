// Convert the renderer's rgba16float sky into an f32 buffer without changing texture usage.
@group(0) @binding(0) var sky: texture_2d_array<f32>;
@group(0) @binding(1) var<storage, read_write> radiance: array<vec4f>;

@compute @workgroup_size(8, 8, 1)
fn read_cube(@builtin(global_invocation_id) id: vec3u) {
    let size = textureDimensions(sky, 0);
    if (id.x >= size.x || id.y >= size.y || id.z >= 6u) {
        return;
    }
    let index = (id.z * size.y + id.y) * size.x + id.x;
    radiance[index] = textureLoad(sky, vec2i(id.xy), i32(id.z), 0);
}
