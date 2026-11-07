// Nonseparable Color blend, evaluated in display RGB after the baseline tone map.
// elevation_color() and its uniforms are shared with appearance.glsl.
uniform int elevation_blend_enabled;
uniform float elevation_blend_opacity;
uniform sampler2D surface_info;

float blend_lum(vec3 c){return dot(c,vec3(.30,.59,.11));}
vec3 blend_clip_color(vec3 c){
    float lum=blend_lum(c);
    float low=min(c.r,min(c.g,c.b));
    float high=max(c.r,max(c.g,c.b));
    if(low<0.)c=vec3(lum)+(c-vec3(lum))*lum/(lum-low);
    if(high>1.)c=vec3(lum)+(c-vec3(lum))*(1.-lum)/(high-lum);
    return c;
}
vec3 blend_set_lum(vec3 c,float lum){
    return blend_clip_color(c+vec3(lum-blend_lum(c)));
}
vec3 elevation_display_blend(vec3 base,vec2 uv){
    if(elevation_blend_enabled==0)return base;
    vec2 info=texture2D(surface_info,uv).rg;
    float opacity=clamp(elevation_blend_opacity,0.,1.)*clamp(info.g,0.,1.);
    if(opacity<=0.)return base;
    vec3 shaded=clamp(base,0.,1.);
    vec3 color=blend_set_lum(elevation_color(info.r),blend_lum(shaded));
    return mix(shaded,color,opacity);
}
