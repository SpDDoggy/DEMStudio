// Original source textures remain read-only. Logical nodes, not source texels.
uniform sampler2DArray pg_low_height;
uniform sampler2D pg_coarse_height;
uniform ivec2 pg_source_size;
uniform vec2 mipmap_tex_inv_size;
uniform ivec2 gm_origin;
uniform int gm_lod;
ivec2 pg_brick_core=ivec2(0);
int pg_brick_layer=0;
uniform ivec2 gm_core;
uniform int gm_slot;
__PG_RAW__
vec4 pg_texture_mipmap(vec2 uv,float level){
    return vec4(pg_raw(uv/mipmap_tex_inv_size-vec2(.5),level));
}
#define texture2DLod(t,uv,level) pg_texture_mipmap(uv,level)
__GEO_SAMPLER__
// GM_COMPUTE_MAIN
layout(local_size_x=8,local_size_y=8) in;
layout(r32f,binding=1) writeonly uniform image2D gm_output;
uniform ivec2 gm_edge;
void main(){
    ivec2 index=ivec2(gl_GlobalInvocationID.xy);if(any(greaterThanEqual(index,gm_edge)))return;
    pg_brick_core=gm_core;pg_brick_layer=gm_slot;
    vec2 cell=vec2(gm_origin)+vec2(index)*exp2(float(gm_lod));
    imageStore(gm_output,index,vec4(geo_height(cell,float(gm_lod))));
}
