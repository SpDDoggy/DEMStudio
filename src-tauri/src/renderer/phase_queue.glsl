#version 430 core
layout(local_size_x=256) in;
layout(std430,binding=1) buffer PhaseQueue { uint pq_words[]; };
layout(std430,binding=2) buffer RayBatchControl { uint pq_batch_words[]; };
uniform sampler2DArray pq_state;
uniform sampler2D pq_pages;
uniform ivec2 pq_extent;
uniform uint pq_capacity,pq_across,pq_mode,pq_batch_mode,pq_expected;
shared uint group_count[4],group_base[4],group_diagnostics[4];
uint diagnostics(){return 16u+4u*pq_capacity;}
uint compute_commands(){return diagnostics()+4u;}
int classify(uint i){
    if(pq_batch_mode!=0u&&pq_batch_words[0]!=pq_expected)return -1;
    ivec2 coord=ivec2(int(i%uint(pq_extent.x)),int(i/uint(pq_extent.x)));
    vec4 status=texelFetch(pq_state,ivec3(coord,5),0);
    if(any(isnan(status))||any(isinf(status))||status.y<0.||status.y>4.||floor(status.y)!=status.y){atomicAdd(group_diagnostics[0],1u);return -1;}
    if(status.y==4.)return -1;
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
    if(phase==1.)return 0;
    if(phase==2.)return 1;
    if(phase==4.)return 3;
    return 2; // SHADOW, MATTE_SUN, MATTE_SKY.
}
void main(){
    uint local=gl_LocalInvocationID.x;
    if(pq_mode==0u){
        if(local<4u){uint command=local*4u;pq_words[command]=0u;pq_words[command+1u]=1u;pq_words[command+2u]=0u;pq_words[command+3u]=0u;pq_words[diagnostics()+local]=0u;
            uint compute=compute_commands()+local*3u;
            pq_words[compute]=0u;pq_words[compute+1u]=1u;pq_words[compute+2u]=1u;}
        return;
    }
    uint i=gl_GlobalInvocationID.x+gl_GlobalInvocationID.y*gl_NumWorkGroups.x*256u;
    if(local<4u){group_count[local]=0u;group_diagnostics[local]=0u;}
    barrier();
    int queue=-1;uint offset=0u;
    if(i<uint(pq_extent.x)*uint(pq_extent.y))queue=classify(i);
    if(queue>=0)offset=atomicAdd(group_count[queue],1u);
    barrier();
    if(local<4u){
        if(group_diagnostics[local]>0u){
            atomicAdd(pq_words[diagnostics()+local],group_diagnostics[local]);
            if(pq_batch_mode!=0u&&local<2u)atomicOr(pq_batch_words[9],1u<<local);
        }
        uint count=group_count[local];
        group_base[local]=count>0u?atomicAdd(pq_words[local*4u],count):0u;
        // Reservations are monotonic, so the maximum rounded range end gives
        // the final indirect dispatch count without a cross-group barrier.
        if(count>0u)atomicMax(pq_words[compute_commands()+local*3u],(min(group_base[local]+count,pq_capacity)+63u)/64u);
        if(group_base[local]+count>pq_capacity){
            atomicAdd(pq_words[diagnostics()+2u],group_base[local]+count-pq_capacity);
            atomicMin(pq_words[local*4u],pq_capacity);
            if(pq_batch_mode!=0u)atomicOr(pq_batch_words[9],4u);
        }
    }
    barrier();
    if(queue>=0){uint destination=group_base[queue]+offset;
        if(destination<pq_capacity)pq_words[16u+uint(queue)*pq_capacity+destination]=i;
    }
}
