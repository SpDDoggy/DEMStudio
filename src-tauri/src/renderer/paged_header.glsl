// Export-only, deterministic query memoization and resumable height traversal.
uniform sampler2DArray pg_state_tex, pg_memo_a, pg_memo_b;
uniform sampler2DArray pg_low_height;
uniform sampler2D pg_coarse_height, pg_page_table;
uniform ivec2 pg_source_size;
uniform int pg_query_index, pg_bank, pg_resume, pg_iteration_budget, pg_reset_query, pg_primary_cached;
layout(location=0) out vec4 pg_out0;
layout(location=1) out vec4 pg_out1;
layout(location=2) out vec4 pg_out2;
layout(location=3) out vec4 pg_out3;
ivec2 pg_brick_core=ivec2(0);
int pg_brick_layer=0;
#ifdef BC_LEAF_BOUND_TEX
uniform sampler2DArray pg_leaf_bounds;
float pg_leaf_raw(vec2 cell){
    return texelFetch(pg_leaf_bounds,ivec3(ivec2(cell-vec2(pg_brick_core)+vec2(64.)),pg_brick_layer),0).x;
}
#endif
#ifdef PG_TIGHT_BOUNDS
uniform sampler2DArray pg_tight_low;
uniform sampler2D pg_tight_coarse;
float pg_tight_raw(vec2 cell,float level){
    int lod=int(level);float scale=exp2(level);
    if(lod>=5)return texelFetch(pg_tight_coarse,ivec2(cell/scale),lod-5).x;
    return texelFetch(pg_tight_low,ivec3(ivec2((cell-vec2(pg_brick_core)+vec2(64.))/scale),pg_brick_layer),lod-1).x;
}
#endif
int pg_page_slot(int page){
    int across=(pg_source_size.x+511)/512;
    return int(texelFetch(pg_page_table,ivec2(page%across,page/across),0).x)-1;
}
#ifdef PG_WIDE_MRT
layout(location=4) out vec4 pg_out4;
layout(location=5) out vec4 pg_out5;
layout(location=6) out vec4 pg_out6;
#endif
vec4 pg_s[7];
vec4 pg_radiance=vec4(0.);
int pg_ordinal=0;
bool pg_yield=false;
// Status: 0 running/need query, 1 wait page, 2 iteration yield,
// 3 current query complete, 4 complete path.
float pg_raw(vec2 p,float level){
    p=clamp(p,vec2(0.),vec2(pg_source_size-ivec2(1)));
    if(level>=4.5){
        vec2 dim=vec2(textureSize(pg_coarse_height,0));
        return textureLod(pg_coarse_height,((p+vec2(.5))/32.)/dim,level-5.).x;
    }
    return textureLod(pg_low_height,vec3((p-vec2(pg_brick_core)+vec2(64.5))/640.,float(pg_brick_layer)),level).x;
}
#ifdef BC_PRECOMPUTED_BOUNDS
uniform int pg_precomputed_bounds;
float pg_bound_raw(vec2 cell,float level){
    int lod=int(level);float scale=exp2(level);
    // Do not clamp cell to source dimensions: the stored footprint already
    // applies bc_bound's exact lo/hi clamps, including outside-edge cells.
    if(lod>=5)return texelFetch(pg_coarse_height,ivec2(cell/scale),lod-5).x;
    return texelFetch(pg_low_height,ivec3(ivec2((cell-vec2(pg_brick_core)+vec2(64.))/scale),pg_brick_layer),lod).x;
}
#endif
vec4 pg_texture_mipmap(vec2 uv,float level){
    return vec4(pg_raw(uv/mipmap_tex_inv_size-vec2(.5),level));
}
int pg_cell_page(vec2 p){
    ivec2 cell=ivec2(floor(clamp(p,vec2(0.),vec2(pg_source_size-ivec2(1)))/512.));
    return cell.y*((pg_source_size.x+511)/512)+cell.x;
}
vec4 pg_memo(sampler2DArray tex,int query){return texelFetch(tex,ivec3(ivec2(gl_FragCoord.xy),query),0);}

#ifdef PG_GEO_MERGED
uniform sampler2DArray pg_geo_merged_low;
uniform sampler2D pg_geo_merged_coarse;
// Only pg_walk's non-leaf node uses this exact premerged old Geo maximum.
bool pg_geo_merged_valid(vec2 cell,float level){
    int lod=int(level);float scale=exp2(level);
    ivec2 index=lod>=5?ivec2(cell/scale):ivec2((cell-vec2(pg_brick_core)+vec2(64.))/scale);
    ivec2 edge=lod>=5?textureSize(pg_geo_merged_coarse,lod-5):textureSize(pg_geo_merged_low,lod-1).xy;
    return all(greaterThanEqual(index,ivec2(0)))&&all(lessThan(index,edge));
}
float pg_geo_merged_raw(vec2 cell,float level){
    int lod=int(level);float scale=exp2(level);
    if(lod>=5)return texelFetch(pg_geo_merged_coarse,ivec2(cell/scale),lod-5).x;
    return texelFetch(pg_geo_merged_low,ivec3(ivec2((cell-vec2(pg_brick_core)+vec2(64.))/scale),pg_brick_layer),lod-1).x;
}
#endif

#ifdef PG_ANCESTOR_SEED
uniform sampler2DArray pg_ancestors_seed;
uniform float pg_seed_top;
#endif

#ifdef PG_BILINEAR_LEAF_GATHER
bool pg_leaf_gather_raw(vec2 p,vec2 logical_offset,out vec4 H){
    if(any(notEqual(p,floor(p))))return false;
    vec2 lo=clamp(p-logical_offset,vec2(0.),vec2(pg_source_size-ivec2(1)));
    vec2 hi=clamp(p+vec2(1.)-logical_offset,vec2(0.),vec2(pg_source_size-ivec2(1)));
    if(any(notEqual(hi-lo,vec2(1.))))return false;
    vec2 local=lo-vec2(pg_brick_core)+vec2(64.);
    if(any(lessThan(local,vec2(0.)))||any(greaterThan(local,vec2(638.))))return false;
    // OpenGL gather order: upper-left, upper-right, lower-right, lower-left.
    H=textureGather(pg_low_height,vec3((local+vec2(1.))/640.,float(pg_brick_layer)),0).wzxy;
    return true;
}
#endif
