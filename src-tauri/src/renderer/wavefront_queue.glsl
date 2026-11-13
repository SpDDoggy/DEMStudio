#version 430 core
layout(local_size_x=256) in;
layout(std430,binding=1) buffer PhaseQueue { uint pq_words[]; };

uniform sampler2DArray pq_state;
uniform sampler2D pq_pages;
uniform ivec2 pq_extent;
uniform uint pq_capacity,pq_across,pq_mode,sw_count,pq_pair;
shared uint group_count[4],group_base[4],group_diagnostics[8];
uint diagnostics(){return 16u+4u*pq_capacity;}
uint segments(){return diagnostics()+8u;}
int classify(uint i){
    uint pixels=uint(pq_extent.x)*uint(pq_extent.y);uint lane=i/pixels;uint pixel=i%pixels;
    if(lane>=sw_count)return -1;atomicAdd(group_diagnostics[5],1u);
    ivec2 coord=ivec2(int(pixel%uint(pq_extent.x)),int(pixel/uint(pq_extent.x))+int(lane)*pq_extent.y);
    vec4 status=texelFetch(pq_state,ivec3(coord,5),0);
    if(any(isnan(status))||any(isinf(status))||status.y<0.||status.y>4.||floor(status.y)!=status.y){atomicAdd(group_diagnostics[0],1u);return -1;}
    if(status.y==4.){atomicAdd(group_diagnostics[4],1u);return -1;}
    float phase=texelFetch(pq_state,ivec3(coord,7),0).w;
    if(isnan(phase)||isinf(phase)||floor(phase)!=phase||phase<1.||phase>6.){atomicAdd(group_diagnostics[0],1u);return -1;}
    if(status.y==1.){
        ivec2 bounds=textureSize(pq_pages,0);
        // Validate before float-to-integer conversion or any page-table fetch.
        if(pq_across==0u||bounds.x<=0||bounds.y<=0||status.x<0.||floor(status.x)!=status.x
            ||status.x>=float(pq_across)*float(bounds.y)){
            atomicAdd(group_diagnostics[1],1u);return -1;
        }
        uint page=uint(status.x);ivec2 address=ivec2(int(page%pq_across),int(page/pq_across));
        if(any(greaterThanEqual(address,bounds))){atomicAdd(group_diagnostics[1],1u);return -1;}
        float slot=texelFetch(pq_pages,address,0).x;
        if(isnan(slot)||isinf(slot)||slot<0.||floor(slot)!=slot){atomicAdd(group_diagnostics[1],1u);return -1;}
        if(slot<1.){atomicAdd(group_diagnostics[3],1u);return -1;}
    }
    if(pq_pair==2u)return 0; // Fused: every ready phase runs the unchanged state machine.
    if(phase==1.)return 0;
    if(pq_pair!=0u)return 1; // OTHER: SURFACE, SHADOW/MATTE, SCATTER.
    if(phase==2.)return 1;
    if(phase==4.)return 3;
    return 2; // SHADOW, MATTE_SUN, MATTE_SKY.
}
void main(){
    uint local=gl_LocalInvocationID.x;
    if(pq_mode==0u){
        if(local<16u)pq_words[local]=(local%4u==1u)?1u:0u;
        if(local<8u)pq_words[diagnostics()+local]=0u;
        return;
    }
    if(pq_mode==2u){
        if(local<64u){uint phase=local/16u,seg=local%16u;
            uint count=min(pq_words[phase*4u],pq_capacity),base=seg*65536u;
            uint at=segments()+local*4u;
            pq_words[at]=count>base?min(count-base,65536u):0u;
            pq_words[at+1u]=1u;pq_words[at+2u]=base;pq_words[at+3u]=0u;
        }return;
    }
    uint i=gl_GlobalInvocationID.x;
    if(local<4u)group_count[local]=0u;
    if(local<8u)group_diagnostics[local]=0u;
    barrier();int queue=-1;uint offset=0u;
    if(i<uint(pq_extent.x)*uint(pq_extent.y)*sw_count)queue=classify(i);
    if(queue>=0)offset=atomicAdd(group_count[queue],1u);
    barrier();
    if(local<8u&&group_diagnostics[local]>0u)atomicAdd(pq_words[diagnostics()+local],group_diagnostics[local]);
    if(local<4u){uint count=group_count[local];group_base[local]=count>0u?atomicAdd(pq_words[local*4u],count):0u;
        if(group_base[local]+count>pq_capacity)atomicAdd(pq_words[diagnostics()+2u],group_base[local]+count-pq_capacity);
    }barrier();
    if(queue>=0){uint destination=group_base[queue]+offset;if(destination<pq_capacity)pq_words[16u+uint(queue)*pq_capacity+destination]=i;}
}
