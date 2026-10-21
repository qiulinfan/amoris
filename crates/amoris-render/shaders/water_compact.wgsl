// Emit each selected patch's real directed triangles in deterministic patch-major order.
// Template emission has no collapsed triples; the original stable index stream is test-only.
@group(0) @binding(0) var<storage,read_write> mesh_indices:array<u32>;
@group(0) @binding(1) var<storage,read_write> mesh_args:array<u32>;
@group(0) @binding(2) var<storage,read> masks:array<u32>;
@group(0) @binding(3) var<storage,read> templates:array<u32>;
@group(0) @binding(4) var<storage,read> geometry:array<vec4u>;
// Header: patch count, valid cached list, cached dense count, reserved; then patch counts/offsets.
@group(0) @binding(5) var<storage,read_write> scratch:array<u32>;
@group(0) @binding(6) var<uniform> spec:vec4u;
// Count, scan and emit indirect commands; padding word 3 retains the device maximum axis.
// Prepare alone binds this buffer; none of its indirect consumers does.
@group(1) @binding(0) var<storage,read_write> dispatch:array<u32>;
var<workgroup> ranks:array<u32,256>;
// Template, triangle count, template offset, output triangle, begin x/z, width, morph IDs/patch.
var<workgroup> emit_data:array<u32,8>;

fn units()->vec2u {return select(spec.xy-vec2u(1u),spec.xy/max(spec.w,1u),spec.z!=0u);}
fn patch_count()->u32 {let tiles=(units()+vec2u(15u))/16u;return tiles.x*tiles.y;}
fn patch_bounds(patch_id:u32)->vec4u {
    let u=units();let begin=vec2u(patch_id%((u.x+15u)/16u),patch_id/((u.x+15u)/16u))*16u;
    return vec4u(begin,min(vec2u(16u),u-begin));
}
fn triangles(patch_id:u32)->u32 {
    let key=masks[patch_id];if(key<2u){return 0u;}
    let level=key-2u;
    if(level>0u){return templates[level*2u+1u]/3u;}
    let size=patch_bounds(patch_id).zw;return size.x*size.y*select(2u,6u,spec.z!=0u);
}
fn exclusive_scan(lane:u32,value:u32)->u32 {
    ranks[lane]=value;workgroupBarrier();
    for(var offset=1u;offset<256u;offset=offset<<1u) {
        var add=0u;if(lane>=offset){add=ranks[lane-offset];}
        workgroupBarrier();ranks[lane]+=add;workgroupBarrier();
    }
    return ranks[lane]-value;
}
fn set_grid(at:u32,count:u32) {
    let x=min(count,dispatch[3]);dispatch[at]=x;
    dispatch[at+1u]=select(1u,(count+max(x,1u)-1u)/max(x,1u),count!=0u);dispatch[at+2u]=1u;
}
@compute @workgroup_size(1)
fn prepare() {
    if(scratch[1]!=0u && mesh_args[10]==0u) {
        mesh_args[0]=scratch[2];mesh_args[18]=0u;
        set_grid(0u,0u);set_grid(4u,0u);set_grid(8u,0u);return;
    }
    scratch[0]=patch_count();mesh_args[18]=1u;
    set_grid(0u,(scratch[0]+255u)/256u);set_grid(4u,1u);set_grid(8u,scratch[0]);
}
@compute @workgroup_size(256)
fn count(@builtin(workgroup_id) group:vec3u,@builtin(num_workgroups) grid:vec3u,@builtin(local_invocation_index) lane:u32) {
    let patch_id=(group.x+group.y*grid.x)*256u+lane;
    if(patch_id<scratch[0]){scratch[4u+patch_id]=triangles(patch_id);}
}
// Each lane handles a contiguous patch range, retaining deterministic patch ID order.
@compute @workgroup_size(256)
fn scan(@builtin(local_invocation_index) lane:u32) {
    let count=scratch[0];let per=(count+255u)/256u;
    let lo=min(lane*per,count);let hi=min(lo+per,count);var sum=0u;
    for(var n=lo;n<hi;n++){sum+=scratch[4u+n];}
    var run=exclusive_scan(lane,sum);
    for(var n=lo;n<hi;n++){let value=scratch[4u+n];scratch[4u+n]=run;run+=value;}
    if(lane==255u){mesh_args[0]=run*3u;scratch[2]=run*3u;scratch[1]=1u;}
}
fn emitted_index(patch_id:u32,word:u32)->u32 {
    let level=emit_data[0];let begin=vec2u(emit_data[4],emit_data[5]);
    if(level>0u) {
        let local=templates[emit_data[2]+word];
        if(level>=4u){return 0x80000000u | (patch_id*emit_data[7]+local);}
        return begin.x+local%17u+spec.x*(begin.y+local/17u);
    }
    let blocky=spec.z!=0u;let vertices=select(6u,18u,blocky);let q=word/vertices;
    let at=begin+vec2u(q%emit_data[6],q/emit_data[6]);
    if(blocky){return (at.x+units().x*at.y)*18u+word%18u;}
    let a=at.x+spec.x*at.y;
    return a+array<u32,6>(0u,spec.x,spec.x+1u,0u,spec.x+1u,1u)[word%6u];
}
@compute @workgroup_size(256)
fn emit(@builtin(workgroup_id) group:vec3u,@builtin(num_workgroups) grid:vec3u,@builtin(local_invocation_index) lane:u32) {
    let patch_id=group.x+group.y*grid.x;
    if(lane==0u) {
        // Padded workgroups still initialize every word and participate in the uniform barrier.
        for(var n=0u;n<8u;n++){emit_data[n]=0u;}
        if(patch_id<scratch[0]) {
            let key=masks[patch_id];let level=select(0u,key-2u,key>=2u);let bounds=patch_bounds(patch_id);
            emit_data[0]=level;emit_data[1]=triangles(patch_id);emit_data[2]=templates[level*2u];
            emit_data[3]=scratch[4u+patch_id];emit_data[4]=bounds.x;emit_data[5]=bounds.y;
            emit_data[6]=bounds.z;emit_data[7]=geometry[0].x;
        }
    }
    workgroupBarrier();
    for(var triangle=lane;triangle<emit_data[1];triangle+=256u) {
        let word=triangle*3u;let dest=(emit_data[3]+triangle)*3u;
        mesh_indices[dest]=emitted_index(patch_id,word);
        mesh_indices[dest+1u]=emitted_index(patch_id,word+1u);
        mesh_indices[dest+2u]=emitted_index(patch_id,word+2u);
    }
}
