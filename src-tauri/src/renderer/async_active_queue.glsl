#version 430 core
layout(local_size_x=256) in;
layout(std430,binding=1) buffer ActiveQueue { uint words[]; };
layout(std430,binding=4) readonly buffer AsyncSamples { uint as_words[]; };
uniform sampler2DArray aaq_state;
uniform sampler2D aaq_pages;
uniform ivec2 aaq_extent;
uniform uint aaq_target,aaq_mode;
shared uint group_count,group_base;
void main(){
    uint i=gl_GlobalInvocationID.x;
    if(aaq_mode==0u){if(i==0u){words[0]=0u;words[1]=1u;words[2]=0u;words[3]=0u;}return;}
    bool eligible=false;
    if(i<uint(aaq_extent.x)*uint(aaq_extent.y)){
        uint cursor=as_words[4u+2u*i];bool initialized=as_words[5u+2u*i]!=0u;
        if(cursor<aaq_target){
            eligible=!initialized;
            if(initialized){
                ivec2 coord=ivec2(int(i%uint(aaq_extent.x)),int(i/uint(aaq_extent.x)));
                vec4 status=texelFetch(aaq_state,ivec3(coord,5),0);
                bool waiting=status.y>.5&&status.y<1.5;
                eligible=!waiting;
                if(waiting){int page=int(status.x);ivec2 dim=textureSize(aaq_pages,0);
                    if(page>=0&&page<dim.x*dim.y)eligible=texelFetch(aaq_pages,ivec2(page%dim.x,page/dim.x),0).x>0.;
                }
            }
        }
    }
    if(gl_LocalInvocationID.x==0u)group_count=0u;
    barrier();
    uint local_offset=0u;if(eligible)local_offset=atomicAdd(group_count,1u);
    barrier();
    if(gl_LocalInvocationID.x==0u)group_base=group_count>0u?atomicAdd(words[0],group_count):0u;
    barrier();
    if(eligible)words[4u+group_base+local_offset]=i;
}
