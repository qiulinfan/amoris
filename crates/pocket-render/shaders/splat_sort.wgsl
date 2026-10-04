// A portable GPU radix sort of (key, value) pairs of u32 (docs/spec/splats.md 5): least significant
// digit first, 8 bits per pass, stable. Only WebGPU core features: no subgroup operations, 256
// threads and 10 KiB of workgroup memory per workgroup, at most 6 storage buffers.
//
// A pass sorts by the digit at `sp.shift` in three dispatches over tiles of TILE keys:
//   histogram  one workgroup per tile counts the tile's digits;
//   scan_bins  one workgroup per digit scans that digit's counts over the tiles, so each tile knows
//              where its keys of that digit start among all keys of that digit, and stores the
//              digit's total;
//   scatter    one workgroup per tile scans the 256 totals (where each digit starts), then ranks
//              its keys in rounds of 256: each key sets its bit in a 256-bit mask per digit, and
//              its rank among the round's keys of its digit is the popcount of the lower bits. A
//              key's destination is its digit's start, plus its tile's offset, plus the keys of its
//              digit in earlier rounds, plus that rank, so equal digits keep their order.
// The key count is read from `control[0]` (written on the GPU); histogram and scatter are
// dispatched indirectly with one workgroup per tile.

const WG: u32 = 256u;
const ROUNDS: u32 = 16u;
const TILE: u32 = 4096u;        // WG * ROUNDS; sort.rs TILE
const BINS: u32 = 256u;

struct SortPass {
    shift: u32,
    _p0: u32,
    _p1: u32,
    _p2: u32,
};

@group(0) @binding(0) var<uniform> sp: SortPass;
@group(0) @binding(1) var<storage, read> control: array<u32>;
@group(0) @binding(2) var<storage, read> keys_in: array<u32>;
@group(0) @binding(3) var<storage, read> vals_in: array<u32>;
@group(0) @binding(4) var<storage, read_write> keys_out: array<u32>;
@group(0) @binding(5) var<storage, read_write> vals_out: array<u32>;
// [digit * tiles + tile]: the tile's count of the digit, then (after scan_bins) the tile's offset
// among the digit's keys; [BINS * tiles + digit]: the digit's total.
@group(0) @binding(6) var<storage, read_write> hist: array<u32>;

var<workgroup> counts: array<atomic<u32>, 256>;
var<workgroup> scan: array<u32, 256>;
var<workgroup> masks: array<atomic<u32>, 2048>;   // 256 digits x 256 bits
var<workgroup> offsets: array<u32, 256>;

fn tile_count(n: u32) -> u32 {
    return (n + TILE - 1u) / TILE;
}

// The exclusive prefix sum of `v` over the workgroup (Hillis-Steele; call in uniform control flow).
fn exclusive_scan(t: u32, v: u32) -> u32 {
    scan[t] = v;
    workgroupBarrier();
    for (var o = 1u; o < WG; o = o << 1u) {
        var x = 0u;
        if (t >= o) {
            x = scan[t - o];
        }
        workgroupBarrier();
        scan[t] = scan[t] + x;
        workgroupBarrier();
    }
    return scan[t] - v;
}

@compute @workgroup_size(256)
fn histogram(@builtin(workgroup_id) wid: vec3u, @builtin(local_invocation_index) t: u32) {
    atomicStore(&counts[t], 0u);
    workgroupBarrier();
    let n = control[0];
    let base = wid.x * TILE;
    for (var r = 0u; r < ROUNDS; r++) {
        let i = base + r * WG + t;
        if (i < n) {
            atomicAdd(&counts[(keys_in[i] >> sp.shift) & 255u], 1u);
        }
    }
    workgroupBarrier();
    hist[t * tile_count(n) + wid.x] = atomicLoad(&counts[t]);
}

@compute @workgroup_size(256)
fn scan_bins(@builtin(workgroup_id) wid: vec3u, @builtin(local_invocation_index) t: u32) {
    let tiles = tile_count(control[0]);
    let row = wid.x * tiles;
    let per = (tiles + WG - 1u) / WG;
    let lo = min(t * per, tiles);
    let hi = min(lo + per, tiles);
    var sum = 0u;
    for (var i = lo; i < hi; i++) {
        sum += hist[row + i];
    }
    var run = exclusive_scan(t, sum);
    for (var i = lo; i < hi; i++) {
        let c = hist[row + i];
        hist[row + i] = run;
        run += c;
    }
    if (t == WG - 1u) {
        hist[BINS * tiles + wid.x] = run;
    }
}

@compute @workgroup_size(256)
fn scatter(@builtin(workgroup_id) wid: vec3u, @builtin(local_invocation_index) t: u32) {
    let n = control[0];
    let tiles = tile_count(n);
    let start = exclusive_scan(t, hist[BINS * tiles + t]);
    offsets[t] = start + hist[t * tiles + wid.x];
    for (var k = 0u; k < 8u; k++) {
        atomicStore(&masks[t * 8u + k], 0u);
    }
    workgroupBarrier();
    let base = wid.x * TILE;
    let word = t >> 5u;
    let bit = 1u << (t & 31u);
    for (var r = 0u; r < ROUNDS; r++) {
        let i = base + r * WG + t;
        let valid = i < n;
        var key = 0u;
        var val = 0u;
        var d = 0u;
        if (valid) {
            key = keys_in[i];
            val = vals_in[i];
            d = (key >> sp.shift) & 255u;
            atomicOr(&masks[d * 8u + word], bit);
        }
        workgroupBarrier();
        if (valid) {
            var rank = countOneBits(atomicLoad(&masks[d * 8u + word]) & (bit - 1u));
            for (var k = 0u; k < word; k++) {
                rank += countOneBits(atomicLoad(&masks[d * 8u + k]));
            }
            let dst = offsets[d] + rank;
            keys_out[dst] = key;
            vals_out[dst] = val;
        }
        workgroupBarrier();
        // Thread t owns digit t: count its keys of this round and clear its mask.
        var c = 0u;
        for (var k = 0u; k < 8u; k++) {
            c += countOneBits(atomicExchange(&masks[t * 8u + k], 0u));
        }
        offsets[t] += c;
        workgroupBarrier();
    }
}
