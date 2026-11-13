// Surface metadata: R=height, G=map hit, BA=texture coordinates at the ray hit.
uniform sampler2D workflow_surface, workflow_texture;
uniform int texture_overlay_enabled;
uniform vec2 workflow_texture_size;
uniform float texture_opacity, texture_exposure, texture_contrast;
uniform float texture_shadows, texture_highlights, texture_saturation;
uniform float scene_exposure, scene_gamma, scene_brightness, scene_contrast;
uniform float scene_saturation, scene_temperature, scene_shadows, scene_highlights;
uniform int contour_enabled;
uniform sampler2D contour_source;
uniform vec2 contour_source_size, contour_source_step;
uniform vec2 contour_elevation;
uniform vec3 contour_color;
uniform float contour_interval, contour_width, contour_opacity, contour_smoothing;
uniform float contour_major_every, contour_major_width;
vec2 contour_raw(vec2 st){
    vec2 uv=(clamp(st,0.,1.)*(contour_source_size-1.)+.5)/contour_source_size;
    return texture2D(contour_source,uv).rg;
}
// Height and validity are interpolated together: missing samples never become
// an artificial zero-height shoreline. This field is independent of ray-hit Z.
vec3 contour_height(vec2 st){
    vec2 center=contour_raw(st);
    float center_h=center.r/max(center.g,1e-8);
    float lo=center_h,hi=center_h,valid=center.g;
    for(int axis=0;axis<2;++axis){
        vec2 offset=axis==0?vec2(contour_source_step.x,0.):vec2(0.,contour_source_step.y);
        for(int direction=-1;direction<=1;direction+=2){
            vec2 value=contour_raw(st+offset*float(direction));
            float h=value.r/max(value.g,1e-8);
            lo=min(lo,h);hi=max(hi,h);valid=min(valid,value.g);
        }
    }
    if(contour_smoothing<=0.)return vec3(center_h,valid,hi-lo);
    float total=0.,weight=0.;
    vec2 radius=contour_source_step*contour_smoothing;
    for(int y=-1;y<=1;++y){
        for(int x=-1;x<=1;++x){
            vec2 value=contour_raw(st+vec2(x,y)*radius);
            valid=min(valid,value.g);
            float w=(x==0?2.:1.)*(y==0?2.:1.);
            total+=value.r*w;weight+=value.g*w;
        }
    }
    return vec3(total/max(weight,1e-8),valid,hi-lo);
}
float contour_coverage(float level,float gradient,float width){
    float distance=abs(fract(level+.5)-.5)/max(gradient,1e-8);
    // Exact box footprint for a locally straight line, including subpixel widths.
    float coverage=clamp(min(.5,distance+width*.5)-max(-.5,distance-width*.5),0.,1.);
    return coverage*(1.-smoothstep(.25,.65,gradient));
}
vec3 contour_display(vec3 base,vec2 uv){
    if(contour_enabled==0 || contour_opacity<=0. || contour_elevation.y<=0.)return base;
    vec4 info=texture2D(workflow_surface,uv);
    vec3 sampled=contour_height(info.ba);
    float level=contour_elevation.x+contour_elevation.y*sampled.r;
    // Derivatives precede all per-fragment exits and use Euclidean pixel length.
    float gradient=length(vec2(dFdx(level),dFdy(level)));
    if(info.g<.5 || sampled.g<.999 || sampled.b<=1e-7 || gradient<1e-5)return base;
    float minor=contour_coverage(level,gradient,contour_width);
    float major=contour_coverage(level/contour_major_every,gradient/contour_major_every,contour_major_width);
    return mix(base,contour_color,clamp(max(minor,major)*contour_opacity,0.,1.));
}
float workflow_lum(vec3 c){return dot(c,vec3(.2126,.7152,.0722));}
vec3 workflow_tonal(vec3 c,float contrast,float saturation,float shadows,float highlights){
    float light=workflow_lum(c);
    float shift=(shadows*(1.-smoothstep(0.,.65,light))+highlights*smoothstep(.35,1.,light))*.35;
    c+=vec3(shift);
    c=(c-.5)*exp2(contrast)+.5;
    light=workflow_lum(c);
    return mix(vec3(light),c,1.+saturation);
}
vec3 workflow_srgb(vec3 c){
    return mix(c*12.92,1.055*pow(max(c,vec3(0.)),vec3(1./2.4))-.055,step(vec3(.0031308),c));
}
vec3 texture_filter(vec3 c){
    if(texture_exposure==0. && texture_contrast==0. && texture_shadows==0.
       && texture_highlights==0. && texture_saturation==0.)return c;
    c*=exp2(texture_exposure);
    return clamp(workflow_tonal(c,texture_contrast,texture_saturation,texture_shadows,texture_highlights),0.,1.);
}
vec3 workflow_overlay(vec3 base,vec3 layer){
    return mix(2.*base*layer,1.-2.*(1.-base)*(1.-layer),step(vec3(.5),base));
}
vec3 texture_display_overlay(vec3 base,vec2 uv){
    if(texture_overlay_enabled==0 || texture_opacity<=0.)return base;
    vec4 info=texture2D(workflow_surface,uv);
    if(info.g<.5)return base;
    vec2 tex_uv=(clamp(info.ba,0.,1.)*(workflow_texture_size-1.)+.5)/workflow_texture_size;
    // SRGB8_ALPHA8 decodes RGB only; restore display RGB once, leaving alpha linear.
    vec4 layer=texture2D(workflow_texture,tex_uv);
    float opacity=clamp(layer.a*texture_opacity,0.,1.);
    if(opacity<=0.)return base;
    vec3 color=texture_filter(workflow_srgb(layer.rgb));
    return mix(base,workflow_overlay(base,color),opacity);
}
vec3 scene_filter(vec3 c){
    if(scene_exposure==0. && scene_gamma==2.2 && scene_brightness==0.
       && scene_contrast==0. && scene_saturation==0. && scene_temperature==0.
       && scene_shadows==0. && scene_highlights==0.)return c;
    c*=exp2(scene_exposure);
    if(scene_gamma!=2.2)c=pow(max(c,vec3(0.)),vec3(2.2/scene_gamma));
    c+=vec3(scene_brightness);
    c=workflow_tonal(c,scene_contrast,scene_saturation,scene_shadows,scene_highlights);
    c*=vec3(1.+scene_temperature*.15,1.,1.-scene_temperature*.15);
    return clamp(c,0.,1.);
}
