layout(local_size_x=8,local_size_y=8) in;
layout(r32f,binding=1) writeonly uniform image2D ancestor_output;
uniform int ancestor_top;
void main(){
    ivec2 index=ivec2(gl_GlobalInvocationID.xy);if(any(greaterThanEqual(index,ivec2(40))))return;
    pg_brick_core=gm_core;pg_brick_layer=gm_slot;
    vec2 cell=vec2(gm_origin)+vec2(index)*16.;
    float certificate=uintBitsToFloat(0x7f800000u);bool valid=true;
    for(int lod=4;lod<=ancestor_top;++lod){
#ifdef ANCESTOR_BICUBIC_BOUNDS
        // bc_bound aligns independently at each level before pg_bound_raw.
        float scale=exp2(float(lod));vec2 aligned=floor(cell/scale)*scale;
        ivec2 address=lod>=5?ivec2(aligned/scale):ivec2((aligned-vec2(pg_brick_core)+vec2(64.))/scale);
        ivec2 extent=lod>=5?textureSize(pg_coarse_height,lod-5):textureSize(pg_low_height,lod).xy;
        if(any(lessThan(address,ivec2(0)))||any(greaterThanEqual(address,extent))
            ||(lod<5&&(gm_slot<0||gm_slot>=textureSize(pg_low_height,lod).z))){valid=false;break;}
        float height=pg_bound_raw(aligned,float(lod));
#else
        float height=geo_height(cell,float(lod));
#endif
        if(isnan(height)||isinf(height))valid=false;
        certificate=min(certificate,height);
    }
    if(!valid)certificate=uintBitsToFloat(0xff800000u);
    imageStore(ancestor_output,index,vec4(certificate));
}
