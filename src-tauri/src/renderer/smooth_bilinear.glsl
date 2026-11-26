// Interpolate central-difference vertex slopes; hit distance and shadow traversal stay bilinear.
vec2 sn_gradient(vec4 h,vec2 cell,vec2 uv,float scale){
    float lod=log2(scale);
    float l0=heightLod(cell+vec2(-scale,0.),lod);
    float l1=heightLod(cell+vec2(-scale,scale),lod);
    float r0=heightLod(cell+vec2(2.*scale,0.),lod);
    float r1=heightLod(cell+vec2(2.*scale,scale),lod);
    float d0=heightLod(cell+vec2(0.,-scale),lod);
    float d1=heightLod(cell+vec2(scale,-scale),lod);
    float u0=heightLod(cell+vec2(0.,2.*scale),lod);
    float u1=heightLod(cell+vec2(scale,2.*scale),lod);
    return vec2(bilinear(vec4(h.y-l0,r0-h.x,h.w-l1,r1-h.z),uv),
                bilinear(vec4(h.z-d0,h.w-d1,u0-h.x,u1-h.y),uv))*.5;
}
