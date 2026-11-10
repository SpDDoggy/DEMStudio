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
vec4 pg_texture_mipmap(vec2 uv,float level){
    return vec4(pg_raw(uv/mipmap_tex_inv_size-vec2(.5),level));
}
int pg_cell_page(vec2 p){
    ivec2 cell=ivec2(floor(clamp(p,vec2(0.),vec2(pg_source_size-ivec2(1)))/512.));
    return cell.y*((pg_source_size.x+511)/512)+cell.x;
}
vec4 pg_memo(sampler2DArray tex,int query){return texelFetch(tex,ivec3(ivec2(gl_FragCoord.xy),query),0);}
