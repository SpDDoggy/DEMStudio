// Deterministic primary-ray fog; distances and heights use the terrain long edge.
uniform float scene_fog_density,scene_fog_scale;
uniform float scene_fog_start,scene_fog_end;
uniform float scene_fog_height_enabled,scene_fog_height_base,scene_fog_height_falloff;
uniform vec3 scene_fog_color;
// Integral of a constant lower layer with exponential density falloff above its base.
float scene_fog_height_primitive(float z){
    float h=max(scene_fog_height_falloff,1e-6),base=scene_fog_height_base;
    return z<=base?z:base+h*(1.-exp(-(z-base)/h));
}
vec3 scene_apply_fog(vec3 radiance,float distance,float origin_z,float direction_z){
    if(scene_fog_density<=0.||distance<0.)return radiance;
    float scale=max(scene_fog_scale,1e-6);
    float begin=max(scene_fog_start,0.),end=min(distance/scale,max(scene_fog_end,begin));
    float optical_length=max(end-begin,0.);
    if(optical_length<=0.)return radiance;
    if(scene_fog_height_enabled>0.5&&optical_length>0.){
        float za=origin_z/scale+direction_z*begin,zb=origin_z/scale+direction_z*end;
        if(abs(zb-za)<1e-5)optical_length*=exp(-max((za+zb)*.5-scene_fog_height_base,0.)/max(scene_fog_height_falloff,1e-6));
        else optical_length*=clamp((scene_fog_height_primitive(zb)-scene_fog_height_primitive(za))/(zb-za),0.,1.);
    }
    float transmission=exp(-scene_fog_density*optical_length);
    return mix(scene_fog_color,radiance,transmission);
}
vec3 scene_apply_fog(vec3 radiance,float distance){return scene_apply_fog(radiance,distance,0.,0.);}
