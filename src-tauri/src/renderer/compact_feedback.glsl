#version 430 core
layout(local_size_x=256) in;
layout(std430,binding=0) buffer FeedbackBuffer { uint words[]; };
uniform sampler2DArray state_image;
uniform ivec2 extent;
uniform uint page_count;
uniform uint mode;
const uint HEADER=8u;
shared uint counts[7];
void inspect(uint i) {
    atomicAdd(counts[4],1u);
    vec4 s=texelFetch(state_image,ivec3(int(i%uint(extent.x)),int(i/uint(extent.x)),5),0);
    if(any(isnan(s))||any(isinf(s))) { atomicAdd(counts[3],1u); return; }
    // Check range before float-to-int conversion; GLSL overflow is undefined.
    if(s.y<1.0||s.y>=5.0) { atomicAdd(counts[3],1u); return; }
    int status=int(s.y);
    if(status==4) { atomicAdd(counts[0],1u); atomicAdd(counts[1],1u); }
    else if(status==3) { atomicAdd(counts[1],1u); atomicAdd(counts[6],1u); }
    else if(status==2) { atomicAdd(counts[2],1u); atomicAdd(counts[6],1u); }
    else if(s.x>=0.0&&s.x<float(page_count)) { atomicAdd(words[HEADER+uint(s.x)],1u); atomicAdd(counts[5],1u); }
    else atomicAdd(counts[3],1u);
}
void main() {
    uint i=gl_GlobalInvocationID.x+gl_GlobalInvocationID.y*gl_NumWorkGroups.x*256u;
    if(mode==0u) {
        if(i<HEADER+page_count) words[i]=0u;
        return;
    }
    uint total=uint(extent.x)*uint(extent.y);
    uint local=gl_LocalInvocationID.x;
    if(local<7u) counts[local]=0u;
    barrier();
    if(i<total) inspect(i);
    barrier();
    if(local<7u&&counts[local]!=0u) atomicAdd(words[local],counts[local]);
}
