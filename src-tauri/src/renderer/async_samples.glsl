#version 430 core
layout(local_size_x=256) in;
layout(std430,binding=4) buffer AsyncSamples { uint as_words[]; };
uniform uint as_mode,as_total,as_target;
uniform ivec2 as_extent;
layout(std430,binding=2) buffer RayBatchControl {uint rs_batch_words[];};
uniform sampler2DArray as_activity_state;
uniform sampler2D as_activity_pages;
shared uint group_sum,group_done,group_invalid;

void main(){
    uint i=gl_GlobalInvocationID.x,local=gl_LocalInvocationID.x;
    if(as_mode==0u){
        if(i<as_total){as_words[4u+2u*i]=1u;as_words[5u+2u*i]=0u;}
        if(i==0u){as_words[0]=0u;as_words[1]=0u;as_words[2]=0u;as_words[3]=0u;}
        return;
    }
    if(as_mode==1u){if(i==0u){as_words[0]=0u;as_words[1]=0u;as_words[2]=0u;as_words[3]=0u;}return;}
    if(as_mode==4u){
        if(i==0u){uint eligible_count=rs_batch_words[8];rs_batch_words[5]+=eligible_count;rs_batch_words[6]+=uint(eligible_count==0u);rs_batch_words[7]+=1u;rs_batch_words[12]+=as_total;rs_batch_words[8]=0u;}return;
    }
    if(as_mode==3u){
        if(local==0u)group_sum=0u;barrier();
        bool eligible=false;
        if(i<as_total){uint cursor=as_words[4u+2u*i];bool initialized=as_words[5u+2u*i]!=0u;
            if(cursor<as_target){
                eligible=!initialized;
                if(initialized){
                    ivec2 coord=ivec2(int(i%uint(as_extent.x)),int(i/uint(as_extent.x)));
                    vec4 status=texelFetch(as_activity_state,ivec3(coord,5),0);
                    bool waiting=status.y>.5&&status.y<1.5;
                    eligible=!waiting;
                    if(waiting){int page=int(status.x);ivec2 dim=textureSize(as_activity_pages,0);
                        if(page>=0&&page<dim.x*dim.y)eligible=texelFetch(as_activity_pages,ivec2(page%dim.x,page/dim.x),0).x>0.;
                    }
                }
            }
        }
        if(eligible)atomicAdd(group_sum,1u);barrier();
        if(local==0u)atomicAdd(rs_batch_words[8],group_sum);return;
    }
    if(local==0u){group_sum=0u;group_done=0u;group_invalid=0u;}
    barrier();
    if(i<as_total){uint cursor=as_words[4u+2u*i];if(cursor<1u||cursor>as_target||as_words[5u+2u*i]>1u)atomicAdd(group_invalid,1u);else{atomicAdd(group_sum,cursor-1u);if(cursor==as_target)atomicAdd(group_done,1u);}}
    barrier();
    if(local==0u){atomicAdd(as_words[0],group_sum);atomicAdd(as_words[1],group_done);atomicAdd(as_words[2],group_invalid);}
}
