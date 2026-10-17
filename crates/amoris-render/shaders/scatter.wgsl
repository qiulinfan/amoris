// The instance slots that changed this frame, scattered into the instance buffer (upload.rs).
// `src` holds their count n, then their n slot indices, then their n slots of 20 words each (an
// `Instance` of common.wgsl, 80 bytes). One invocation copies one word, so consecutive invocations
// read consecutive words and write a slot's 80 bytes together.

@group(0) @binding(0) var<storage, read> src: array<u32>;
@group(0) @binding(1) var<storage, read_write> dst: array<u32>;

const WORDS: u32 = 20u;
const GROUP: u32 = 256u;
// Workgroups per row of the dispatch (upload.rs `ROW`): past it the dispatch adds rows.
const ROW: u32 = 65535u;

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let n = src[0];
    let t = id.x + id.y * ROW * GROUP;
    if t >= n * WORDS {
        return;
    }
    let item = t / WORDS;
    let word = t - item * WORDS;
    dst[src[1u + item] * WORDS + word] = src[1u + n + t];
}
