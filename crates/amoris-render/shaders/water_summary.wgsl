// Unkeyed BLAKE3 over the resident field's little-endian packed f32 bytes. Algorithm:
// https://github.com/BLAKE3-team/BLAKE3-specs/blob/master/blake3.pdf (sections 2 and 4).
// Independently written WGSL, tested against the pinned Rust blake3 crate.
// One invocation hashes a 1024-byte chunk; parent dispatches reduce the fixed binary tree.
struct Params { columns:u32, width:u32, count:u32, words:u32 }
struct Node {
    cv_lo:vec4u,
    cv_hi:vec4u,
    // maximum depth, maximum absolute face speed, maximum pressure jump, summed depth
    maximum:vec4f,
    // summed wet-edge surface, wet-edge count, invalid component count, unused
    edge:vec4f,
}
@group(0) @binding(0) var<uniform> p:Params;
@group(0) @binding(1) var<storage,read> field:array<vec4u>;
@group(0) @binding(2) var<storage,read> input:array<Node>;
@group(0) @binding(3) var<storage,read_write> output:array<Node>;
const IV:array<u32,8>=array<u32,8>(0x6a09e667u,0xbb67ae85u,0x3c6ef372u,0xa54ff53au,0x510e527fu,0x9b05688cu,0x1f83d9abu,0x5be0cd19u);
const PERM:array<u32,16>=array<u32,16>(2u,6u,3u,10u,7u,0u,4u,13u,1u,11u,12u,5u,9u,14u,15u,8u);
fn blake_rotr(x:u32,n:u32)->u32 { return (x>>n) | (x<<(32u-n)); }
fn mix_word(v:ptr<function,array<u32,16>>,a:u32,b:u32,c:u32,d:u32,x:u32,y:u32) {
    (*v)[a]=(*v)[a]+(*v)[b]+x; (*v)[d]=blake_rotr((*v)[d]^(*v)[a],16u);
    (*v)[c]=(*v)[c]+(*v)[d]; (*v)[b]=blake_rotr((*v)[b]^(*v)[c],12u);
    (*v)[a]=(*v)[a]+(*v)[b]+y; (*v)[d]=blake_rotr((*v)[d]^(*v)[a],8u);
    (*v)[c]=(*v)[c]+(*v)[d]; (*v)[b]=blake_rotr((*v)[b]^(*v)[c],7u);
}
fn compress(cv:array<u32,8>,words:array<u32,16>,counter:u32,len:u32,flags:u32)->array<u32,16> {
    var v=array<u32,16>(cv[0],cv[1],cv[2],cv[3],cv[4],cv[5],cv[6],cv[7],IV[0],IV[1],IV[2],IV[3],counter,0u,len,flags);
    var m=words;
    for(var r=0u;r<7u;r++) {
        mix_word(&v,0u,4u,8u,12u,m[0],m[1]);mix_word(&v,1u,5u,9u,13u,m[2],m[3]);
        mix_word(&v,2u,6u,10u,14u,m[4],m[5]);mix_word(&v,3u,7u,11u,15u,m[6],m[7]);
        mix_word(&v,0u,5u,10u,15u,m[8],m[9]);mix_word(&v,1u,6u,11u,12u,m[10],m[11]);
        mix_word(&v,2u,7u,8u,13u,m[12],m[13]);mix_word(&v,3u,4u,9u,14u,m[14],m[15]);
        var next:array<u32,16>;
        for(var j=0u;j<16u;j++){next[j]=m[PERM[j]];}
        m=next;
    }
    for(var j=0u;j<8u;j++){v[j]=v[j]^v[j+8u];v[j+8u]=v[j+8u]^cv[j];}
    return v;
}
fn pressure(a:vec4f,b:vec4f)->f32 {
    if(a.y<=0.0001 && b.y<=0.0001){return 0.0;}
    return abs(clamp((b.x+b.y)-(a.x+a.y),-a.y,b.y));
}
@compute @workgroup_size(64)
fn leaves(@builtin(global_invocation_id) id:vec3u) {
    let chunk=id.x;
    if(chunk>=p.count){return;}
    let words_count=p.words;
    let start=chunk*256u;
    let blocks=min(16u,(words_count-start+15u)/16u);
    var cv=IV;
    for(var block=0u;block<blocks;block++) {
        var words:array<u32,16>;
        for(var j=0u;j<16u;j++){
            let at=start+block*16u+j;
            words[j]=0u;
            if(at<words_count){words[j]=field[at/4u][at%4u];}
        }
        var flags=select(0u,1u,block==0u);
        if(block+1u==blocks){flags=flags | 2u;if(p.count==1u){flags=flags | 8u;}}
        let len=min(64u,(words_count-start-block*16u)*4u);
        let result=compress(cv,words,chunk,len,flags);
        for(var j=0u;j<8u;j++){cv[j]=result[j];}
    }
    var node:Node;
    node.cv_lo=vec4u(cv[0],cv[1],cv[2],cv[3]);node.cv_hi=vec4u(cv[4],cv[5],cv[6],cv[7]);
    node.maximum=vec4f(0.0);node.edge=vec4f(0.0);
    let height=p.columns/p.width;
    for(var at=chunk*64u;at<min((chunk+1u)*64u,p.columns);at++) {
        let bits=field[at];let c=bitcast<vec4f>(bits);
        for(var j=0u;j<4u;j++){if((bits[j]&0x7f800000u)==0x7f800000u){node.edge.z+=1.0;}}
        if(c.y<0.0){node.edge.z+=1.0;}
        node.maximum.x=max(node.maximum.x,c.y);
        node.maximum.y=max(node.maximum.y,max(abs(c.z),abs(c.w)));
        node.maximum.w+=c.y;
        let x=at%p.width;let z=at/p.width;
        if(x+1u<p.width){node.maximum.z=max(node.maximum.z,pressure(c,bitcast<vec4f>(field[at+1u])));}
        if(z+1u<height){node.maximum.z=max(node.maximum.z,pressure(c,bitcast<vec4f>(field[at+p.width])));}
        if(c.y>0.01 && (x==0u || z==0u || x+1u==p.width || z+1u==height)){
            node.edge.x+=c.x+c.y;node.edge.y+=1.0;
        }
    }
    for(var at=max(chunk*64u,p.columns);at<min((chunk+1u)*64u,p.words/4u);at++){
        let bits=field[at];let value=bitcast<vec4f>(bits);
        for(var j=0u;j<4u;j++){
            if((bits[j]&0x7f800000u)==0x7f800000u){node.edge.z+=1.0;}
            node.maximum.y=max(node.maximum.y,abs(value[j]));
        }
    }
    output[chunk]=node;
}
@compute @workgroup_size(64)
fn parents(@builtin(global_invocation_id) id:vec3u) {
    let at=id.x*2u;
    if(at>=p.count){return;}
    let left=input[at];
    if(at+1u==p.count){output[id.x]=left;return;}
    let right=input[at+1u];
    var words:array<u32,16>;
    for(var j=0u;j<4u;j++){
        words[j]=left.cv_lo[j];words[j+4u]=left.cv_hi[j];
        words[j+8u]=right.cv_lo[j];words[j+12u]=right.cv_hi[j];
    }
    let result=compress(IV,words,0u,64u,select(4u,12u,p.count==2u));
    var node:Node;
    node.cv_lo=vec4u(result[0],result[1],result[2],result[3]);node.cv_hi=vec4u(result[4],result[5],result[6],result[7]);
    node.maximum=vec4f(max(left.maximum.xyz,right.maximum.xyz),left.maximum.w+right.maximum.w);
    node.edge=left.edge+right.edge;
    output[id.x]=node;
}
