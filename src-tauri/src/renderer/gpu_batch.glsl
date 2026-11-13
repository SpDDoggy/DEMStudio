#version 430 core
layout(local_size_x=1) in;
layout(std430,binding=2) buffer RayBatchControl { uint rs_batch_words[]; };
layout(std430,binding=0) readonly buffer FeedbackBuffer { uint gb_feedback[]; };
layout(std430,binding=1) readonly buffer ActiveQueue { uint gb_queue[]; };
uniform uint gb_mode, gb_expected, gb_total;
void main(){
    if(gb_mode==4u){
        // PhaseQueue has four DrawArraysIndirect commands at words 0/4/8/12.
        // Classification has finished behind an SSBO barrier before this dispatch.
        for(uint phase=0u;phase<4u;++phase){
            uint eligible_count=rs_batch_words[0]==gb_expected?gb_queue[phase*4u]:0u;
            rs_batch_words[5]+=eligible_count;
            rs_batch_words[6]+=eligible_count==0u?1u:0u;
            rs_batch_words[7]+=1u;
            rs_batch_words[8]=eligible_count;
            rs_batch_words[12]+=gb_total;
        }
        return;
    }
    if(gb_mode==3u){
        uint eligible_count=rs_batch_words[0]==gb_expected?gb_queue[0]:0u;
        rs_batch_words[5]+=eligible_count;
        rs_batch_words[6]+=eligible_count==0u?1u:0u;
        rs_batch_words[7]+=1u;
        rs_batch_words[8]=eligible_count;
        rs_batch_words[12]+=gb_total;
        return;
    }
    if(rs_batch_words[0]!=gb_expected)return;
    if(gb_mode==0u){
        // Start one draw; never reset initialized or accumulated sample done.
        rs_batch_words[8]=0u;
    }else if(gb_mode==1u){
        // Consume the existing device reduction. No per-ray global atomics.
        rs_batch_words[1]=gb_feedback[0];
        rs_batch_words[9]|=gb_feedback[3];
        rs_batch_words[10]=gb_feedback[2];
        rs_batch_words[11]+=1u;
        // Existing sample-boundary reduction; no extra state scan or readback.
        rs_batch_words[4]+=gb_feedback[0];
        rs_batch_words[13]+=gb_feedback[5];
        rs_batch_words[14]+=gb_feedback[6];
        rs_batch_words[15]+=gb_feedback[4];
        rs_batch_words[2]=1u;
        rs_batch_words[3]=gb_feedback[0]==gb_total&&gb_feedback[4]==gb_total&&gb_feedback[3]==0u&&rs_batch_words[9]==0u?1u:0u;
    }else if(gb_mode==2u){
        if(rs_batch_words[3]!=0u){
            rs_batch_words[0]+=1u;
            rs_batch_words[1]=0u;
            rs_batch_words[2]=0u;
            rs_batch_words[3]=0u;
        }else rs_batch_words[2]=1u;
    }
}
