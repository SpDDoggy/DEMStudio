// Experimental coarse mathematical classifier, NOT a float-equivalence certificate.
const uint VP_CLEAR=0u,VP_OCCLUDED=1u,VP_UNKNOWN=2u;
uniform sampler2D vp_bounds;
uniform ivec2 vp_size;
uniform int vp_top,vp_padded,vp_geo_offset;
uniform uint vp_budget;
uniform vec3 vp_height; // scale, world offset, quant step (zero disables)
uniform vec3 vp_volume_min,vp_volume_max;
bool vp_finite(float x){return !isnan(x)&&!isinf(x);}
bool vp_finite3(vec3 x){return !any(isnan(x))&&!any(isinf(x));}
float vp_margin(float magnitude){return max(1e-5,magnitude*7.62939453125e-6);}
// Returns 0 empty, 1 valid, 2 boundary/uncertain. Direction zero is rejected upstream.
uint vp_clip(vec3 o,vec3 dir,vec3 lower,vec3 upper,inout vec2 interval){
    vec3 a=(lower-o)/dir,b=(upper-o)/dir;
    vec3 near_t=min(a,b),far_t=max(a,b);
    interval.x=max(interval.x,max(near_t.x,max(near_t.y,near_t.z)));
    interval.y=min(interval.y,min(far_t.x,min(far_t.y,far_t.z)));
    if(!vp_finite(interval.x)||!vp_finite(interval.y))return 2u;
    float margin=vp_margin(max(abs(interval.x),abs(interval.y)));
    if(interval.x>interval.y+margin)return 0u;
    if(interval.y<=interval.x+margin)return 2u;return 1u;
}
uint vp_classify(vec3 o,vec3 dir,vec2 interval,out uint nodes){
    nodes=0u;
    if(!vp_finite3(o)||!vp_finite3(dir)||any(equal(dir,vec3(0.)))||!vp_finite3(vp_height)
       ||!vp_finite3(vp_volume_min)||!vp_finite3(vp_volume_max)||!vp_finite(interval.x)||!vp_finite(interval.y)
       ||vp_height.x<0.||vp_height.z<0.||vp_top<5||vp_top>20||vp_budget==0u
       ||vp_padded!=(1<<vp_top)||any(greaterThan(vp_size+ivec2(2*vp_geo_offset),ivec2(vp_padded)))
       ||any(notEqual(textureSize(vp_bounds,0),ivec2(1<<(vp_top-5))))
       ||any(lessThanEqual(vp_size,ivec2(0)))||vp_geo_offset<0||vp_geo_offset>1)return VP_UNKNOWN;
    if(interval.y<0.)interval.y=3.402823466e38;
    vec3 lower=max(vp_volume_min,vec3(0.,0.,vp_volume_min.z));
    vec3 upper=min(vp_volume_max,vec3(vec2(vp_size+ivec2(2*vp_geo_offset)),vp_volume_max.z));
    if(any(greaterThanEqual(lower,upper)))return VP_UNKNOWN;
    uint clipped=vp_clip(o,dir,lower,upper,interval);if(clipped==0u)return VP_CLEAR;if(clipped==2u)return VP_UNKNOWN;
    ivec3 stack[64];int count=1;stack[0]=ivec3(0,0,vp_top);bool uncertain=false;
    while(count>0){
        if(nodes>=vp_budget)return VP_UNKNOWN;
        ivec3 node=stack[--count];nodes++;
        float scale=exp2(float(node.z));
        vec3 nlo=max(lower,vec3(vec2(node.xy)*scale,lower.z));
        vec3 nhi=min(upper,vec3(vec2(node.xy+ivec2(1))*scale,upper.z));
        if(any(greaterThanEqual(nlo,nhi)))continue;
        vec2 segment=interval;uint hit=vp_clip(o,dir,nlo,nhi,segment);
        if(hit==0u)continue;if(hit==2u){uncertain=true;continue;}
        vec2 bounds=texelFetch(vp_bounds,node.xy,node.z-5).rg;
        if(any(isnan(bounds))||any(isinf(bounds))||bounds.x>bounds.y){uncertain=true;continue;}
        bounds*=vp_height.x;
        if(vp_height.z>0.)bounds=ceil(bounds/vp_height.z)*vp_height.z;
        bounds+=vp_height.y;
        vec2 z=vec2(o.z+dir.z*segment.x,o.z+dir.z*segment.y);
        if(any(isnan(bounds))||any(isinf(bounds))||any(isnan(z))||any(isinf(z))){uncertain=true;continue;}
        float margin=vp_margin(max(max(abs(bounds.x),abs(bounds.y)),max(abs(z.x),abs(z.y))));
        if(min(z.x,z.y)>bounds.y+margin)continue;
        if(bounds.x>0.&&max(z.x,z.y)<bounds.x-margin)return VP_OCCLUDED;
        if(node.z==5){uncertain=true;continue;}
        if(count+4>64)return VP_UNKNOWN;
        for(int child=3;child>=0;--child)stack[count++]=ivec3(node.xy*2+ivec2(child&1,child>>1),node.z-1);
    }
    return uncertain?VP_UNKNOWN:VP_CLEAR;
}
