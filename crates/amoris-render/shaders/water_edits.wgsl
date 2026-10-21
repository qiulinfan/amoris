// CPU groups edits by column, preserving their order within that column. One invocation owns
// each run, so additions and clamped removals need no float atomic or storage race.
struct Params { count:u32, unused:vec3u }
@group(0) @binding(0) var<uniform> p:Params;
@group(0) @binding(1) var<storage,read_write> field:array<vec4f>;
@group(0) @binding(2) var<storage,read_write> runs:array<vec4u>;
@group(0) @binding(3) var<storage,read> changes:array<f32>;
@compute @workgroup_size(64)
fn apply_edits(@builtin(global_invocation_id) id:vec3u) {
    if(id.x>=p.count){return;}
    let run=runs[id.x];var c=field[run.x];runs[id.x].w=bitcast<u32>(c.y);
    for(var i=0u;i<run.z;i++){c.y=max(c.y+changes[run.y+i],0.0);}
    field[run.x]=c;
}

@group(0) @binding(4) var<storage,read_write> edit_receipt:vec4f;
var<workgroup> edit_sums:array<vec4f,64>;
@compute @workgroup_size(64)
fn report_edits(@builtin(local_invocation_index) lane:u32){
    var sum=vec4f(0.0);
    for(var n=lane;n<p.count;n+=64u){
        let r=runs[n];let actual=field[r.x].y-bitcast<f32>(r.w);sum.x+=actual;
        if(abs(actual)>3.402823e38||actual!=actual){sum.z+=1.0;}
    }
    edit_sums[lane]=sum;workgroupBarrier();
    for(var stride=32u;stride>0u;stride/=2u){if(lane<stride){edit_sums[lane]+=edit_sums[lane+stride];}workgroupBarrier();}
    if(lane==0u){edit_receipt=edit_sums[0];}
}
