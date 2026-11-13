// Included before pg_walk. Explicit candidate: shadow, non-leaf coarse nodes.
uniform sampler2DArray dcsf_bounds;
uniform int dcsf_enabled,dcsf_top;
bool dcsf_cached=false;
int dcsf_cached_lod=-1;
ivec2 dcsf_lo,dcsf_hi;
float dcsf_raw;
int dcsf_direction(ivec2 s){return s.y<0?s.x+1:(s.y==0?(s.x<0?3:4):s.x+6);}
ivec2 dcsf_sign(int d){
    if(d<3)return ivec2(d-1,-1);
    if(d<5)return ivec2(d==3?-1:1,0);
    return ivec2(d-6,1);
}
bool dcsf_empty(bool visibility,float lod,vec2 cell,vec2 direction,float z0,float z1){
    if(dcsf_enabled==0||!visibility||lod<5.||lod>float(dcsf_top)||lod!=floor(lod)
        ||any(isnan(direction))||any(isinf(direction))||isnan(z0)||isinf(z0)||isnan(z1)||isinf(z1)
        ||!(mipmap_size.z>=0.)||isnan(mipmap_size.z)||isinf(mipmap_size.z)
        ||isnan(mipmap_size.w)||isinf(mipmap_size.w))return false;
#ifdef OPT_LEVEL
    if(!(mipmap_step>0.)||isnan(mipmap_step)||isinf(mipmap_step))return false;
#endif
    int level=int(lod);ivec2 index=ivec2(cell/exp2(lod));
    ivec2 sign_dir=ivec2(sign(direction));
    if(all(equal(sign_dir,ivec2(0))))return false;
    if(!dcsf_cached||level!=dcsf_cached_lod||any(lessThan(index,dcsf_lo))||any(greaterThan(index,dcsf_hi))){
        ivec3 extent=textureSize(dcsf_bounds,level-5);
        if(any(lessThan(index,ivec2(0)))||any(greaterThanEqual(index,extent.xy)))return false;
        int direction_index=dcsf_direction(sign_dir);
        dcsf_raw=texelFetch(dcsf_bounds,ivec3(index,direction_index),level-5).x;
        dcsf_lo=index+min(sign_dir*3,ivec2(0));
        dcsf_hi=index+max(sign_dir*3,ivec2(0));
        dcsf_cached_lod=level;dcsf_cached=true;
    }
    if(isnan(dcsf_raw)||isinf(dcsf_raw))return false;
    float product=dcsf_raw*mipmap_size.z;
    float upper=product;
    float magnitude=max(max(abs(product),abs(mipmap_size.w)),max(abs(z0),abs(z1)));
    float rounding_guard=0.;
#ifdef OPT_LEVEL
    upper=ceil(upper/mipmap_step)*mipmap_step;
    // One extra whole quantization step covers a boundary rounding difference.
    rounding_guard=abs(mipmap_step);
    magnitude=max(magnitude,max(abs(upper),abs(mipmap_step)));
#endif
    upper=upper+mipmap_size.w;
    float margin=max(1e-5,magnitude*7.62939453125e-6)+rounding_guard;
    return !isnan(upper)&&!isinf(upper)&&!isnan(margin)&&!isinf(margin)&&min(z0,z1)>upper+margin;
}
