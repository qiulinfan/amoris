// Convert the resident SWE field into the existing surface shader's row-major input entirely on
// GPU. Face velocities are centred exactly as the CPU presentation extractor centres them. The
// drawn surface is the free surface: bodies' displaced head stands on the water column.
struct Params { size: vec2<u32>, datum: f32, padding: u32 };
@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var<storage, read> field: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read_write> columns: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read> occupancy: array<f32>;
@compute @workgroup_size(8, 8)
fn present(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id.xy >= p.size) { return; }
    let n = id.x + p.size.x * id.y;
    let c = field[n];
    let east=id.y;let north=p.size.y+id.x;let base=p.size.x*p.size.y;
    var right = field[base+east/4u][east%4u];
    var above = field[base+north/4u][north%4u];
    if id.x + 1u < p.size.x { right = field[n + 1u].z; }
    if id.y + 1u < p.size.y { above = field[n + p.size.x].w; }
    let head = select(0.0, occupancy[n], c.y > 0.0);
    columns[n] = vec4<f32>(p.datum + c.x + c.y + head, c.y, (c.z + right) * 0.5, (c.w + above) * 0.5);
}
