// One radix-2 complex FFT per line of a power-of-two grid, in workgroup memory: bit-reversed
// load, iterative Cooley-Tukey butterflies, then a store scaled by 1/n for the inverse. Lines
// run along x (axis 0, contiguous) or z (axis 1, strided). One workgroup owns one line, so no
// result depends on another workgroup's order and a replay on one device is bit-identical.
struct Fft { width:u32, height:u32, axis:u32, inverse:u32 }
@group(0) @binding(0) var<uniform> f:Fft;
@group(0) @binding(1) var<storage,read_write> data:array<vec2f>;
var<workgroup> line:array<vec2f,1024>;

const TAU:f32=6.283185307179586;
fn fft_index(lane:u32,j:u32)->u32 {
    if(f.axis==0u){return lane*f.width+j;}
    return j*f.width+lane;
}
@compute @workgroup_size(256)
fn fft_line(@builtin(workgroup_id) group:vec3u,@builtin(local_invocation_index) lane:u32){
    let which=group.x;
    let n=select(f.height,f.width,f.axis==0u);
    let bits=countTrailingZeros(n);
    // Every slot the butterflies read is written here first; workgroup memory is not zeroed.
    for(var j=lane;j<n;j+=256u){
        let r=reverseBits(j)>>(32u-bits);
        line[r]=data[fft_index(which,j)];
    }
    workgroupBarrier();
    let sign=select(-1.0,1.0,f.inverse!=0u);
    for(var s=1u;s<=bits;s++){
        let half=1u<<(s-1u);
        let span=half<<1u;
        for(var t=lane;t<(n>>1u);t+=256u){
            let k=t%half;
            let i0=(t/half)*span+k;
            let i1=i0+half;
            let angle=sign*TAU*f32(k)/f32(span);
            let w=vec2f(cos(angle),sin(angle));
            let a=line[i0];
            let b=line[i1];
            let wb=vec2f(w.x*b.x-w.y*b.y,w.x*b.y+w.y*b.x);
            line[i0]=a+wb;
            line[i1]=a-wb;
        }
        workgroupBarrier();
    }
    let scale=select(1.0,1.0/f32(n),f.inverse!=0u);
    for(var j=lane;j<n;j+=256u){
        data[fft_index(which,j)]=line[j]*scale;
    }
}
