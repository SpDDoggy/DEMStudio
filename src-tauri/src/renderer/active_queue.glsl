#version 430 core
layout(local_size_x=256) in;
// DrawArraysIndirectCommand followed by compact pixel indices.
layout(std430,binding=1) buffer ActiveQueue { uint words[]; };
layout(std430,binding=2) readonly buffer RayBatchControl { uint aq_batch_words[]; };
uniform sampler2DArray aq_state;
uniform sampler2D aq_pages;
uniform ivec2 aq_extent;
uniform int aq_across, aq_resume, aq_query_memo, aq_reset_query, aq_mode;
uniform int aq_batch_mode, aq_expected_sample;
shared uint group_count;
shared uint group_base;
void main(){
    uint i=gl_GlobalInvocationID.x+gl_GlobalInvocationID.y*gl_NumWorkGroups.x*256u;
    if(aq_mode==0){if(i==0u){words[0]=0u;words[1]=1u;words[2]=0u;words[3]=0u;}return;}
    // Tail invocations must participate in every workgroup barrier.
    bool batch_ready=aq_batch_mode==0||aq_batch_words[0]==uint(aq_expected_sample);
    bool resume=aq_resume!=0||(aq_batch_mode!=0&&aq_batch_words[2]!=0u);
    bool eligible=batch_ready&&i<uint(aq_extent.x)*uint(aq_extent.y);
    if(resume&&eligible){
        ivec2 p=ivec2(int(i%uint(aq_extent.x)),int(i/uint(aq_extent.x)));
        vec4 s=texelFetch(aq_state,ivec3(p,5),0);
        bool done=s.y>3.5;
        bool reset=aq_query_memo!=0&&aq_reset_query!=0&&!done;
        bool current_done=aq_query_memo!=0&&!reset&&texelFetch(aq_state,ivec3(p,4),0).w>.5;
        bool waiting=!reset&&s.y>.5&&s.y<1.5;
        bool ready=!waiting;
        if(waiting&&s.x>=0.0){
            int page=int(s.x);
            ivec2 address=ivec2(page%aq_across,page/aq_across);
            ivec2 bounds=textureSize(aq_pages,0);
            ready=all(greaterThanEqual(address,ivec2(0)))&&all(lessThan(address,bounds))
                &&int(texelFetch(aq_pages,address,0).x)-1>=0;
        }
        eligible=!done&&!current_done&&ready;
    }
    if(gl_LocalInvocationID.x==0u)group_count=0u;
    barrier();
    uint local_offset=0u;
    if(eligible)local_offset=atomicAdd(group_count,1u);
    barrier();
    // Reserve a contiguous range once per nonempty group rather than once
    // per pixel. Within-group atomics stay in fast shared memory.
    if(gl_LocalInvocationID.x==0u)group_base=group_count>0u?atomicAdd(words[0],group_count):0u;
    barrier();
    if(eligible)words[4u+group_base+local_offset]=i;
}
