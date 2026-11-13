#version 430 compatibility
layout(local_size_x=8,local_size_y=8) in;
uniform sampler2DArray bl_raw;
layout(r32f,binding=1) writeonly uniform image2DArray bl_output;
uniform ivec2 bl_origin,bl_size;
uniform int bl_offset,bl_slot;
void main(){
    ivec2 xy=ivec2(gl_GlobalInvocationID.xy);if(any(greaterThanEqual(xy,ivec2(640))))return;
    ivec2 cell=bl_origin+xy;
    ivec2 lo=clamp(cell-ivec2(1+bl_offset),ivec2(0),bl_size-ivec2(1));
    ivec2 hi=clamp(cell+ivec2(2-bl_offset),ivec2(0),bl_size-ivec2(1));
    if(any(lessThan(lo-bl_origin,ivec2(0)))||any(greaterThanEqual(hi-bl_origin,ivec2(640)))){
        imageStore(bl_output,ivec3(xy,bl_slot),vec4(uintBitsToFloat(0x7f800000u)));return;
    }
    float h=0.;
    for(int y=0;y<4;y++)for(int x=0;x<4;x++){
        ivec2 index=lo+ivec2(x,y);if(any(greaterThan(index,hi)))continue;
        float raw=texelFetch(bl_raw,ivec3(index-bl_origin,bl_slot),0).x;
        h=max(h,raw);
    }
    imageStore(bl_output,ivec3(xy,bl_slot),vec4(h));
}
