// Surface metadata: R=height, G=map hit, BA=texture coordinates at the ray hit.
uniform sampler2D workflow_surface, workflow_texture;
uniform int texture_overlay_enabled;
uniform vec2 workflow_texture_size;
uniform int texture_mapping, texture_fit, texture_blend;
uniform float texture_scale, texture_rotation, workflow_height_curve;
uniform vec2 texture_offset;
// Actual rendered terrain spans, divided by its longest horizontal edge.
uniform vec3 workflow_geometry;
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
vec2 workflow_tex_coord(vec2 st,vec2 extent){
    vec2 q=st-.5;
    if(texture_fit!=0){
        float plane=max(extent.x,1e-8)/max(extent.y,1e-8);
        float image=workflow_texture_size.x/workflow_texture_size.y;
        q*=vec2(max(plane/image,1.),max(image/plane,1.));
    }
    float a=radians(texture_rotation),c=cos(a),s=sin(a);
    return mat2(c,-s,s,c)*q/max(texture_scale,1e-6)+.5-texture_offset;
}
vec4 workflow_tex_sample(vec2 st,vec2 extent){
    vec2 q=workflow_tex_coord(st,extent);
    if(texture_fit==2){
        // Repeat must interpolate across the seam, while the default sampler
        // remains clamped for aligned imagery and transparent contain borders.
        ivec2 size=ivec2(workflow_texture_size);
        vec2 pixel=fract(q)*workflow_texture_size-.5;
        ivec2 lo=ivec2(floor(pixel));vec2 f=fract(pixel);
        ivec2 a=(lo%size+size)%size,b=((lo+1)%size+size)%size;
        return mix(mix(texelFetch(workflow_texture,a,0),texelFetch(workflow_texture,ivec2(b.x,a.y),0),f.x),
                   mix(texelFetch(workflow_texture,ivec2(a.x,b.y),0),texelFetch(workflow_texture,b,0),f.x),f.y);
    }
    if(any(lessThan(q,vec2(0.))) || any(greaterThan(q,vec2(1.))))return vec4(0.);
    vec2 tex_uv=(clamp(q,0.,1.)*(workflow_texture_size-1.)+.5)/workflow_texture_size;
    return texture2D(workflow_texture,tex_uv);
}
vec4 workflow_mapped_layer(vec4 info){
    // The exact old expression remains the default route, including edge texels.
    if(texture_mapping==0 && texture_fit==0 && texture_scale==1.
       && texture_rotation==0. && all(equal(texture_offset,vec2(0.)))){
        vec2 tex_uv=(clamp(info.ba,0.,1.)*(workflow_texture_size-1.)+.5)/workflow_texture_size;
        return texture2D(workflow_texture,tex_uv);
    }
    vec3 geometry=max(workflow_geometry,vec3(1e-8));
    if(texture_mapping==0)return workflow_tex_sample(info.ba,geometry.xy);
    if(texture_mapping==1)return workflow_tex_sample((info.ba-.5)*geometry.xy+.5,vec2(1.));
    float q=clamp((info.r-1e-6)/(1.-1e-6),0.,1.);
    float curve=max(workflow_height_curve,1e-6);
    float height=(q/(curve*(1.-q)+q))*(1.-1e-6)+1e-6;
    vec3 p=vec3(info.ba*geometry.xy,height*geometry.z);
    vec3 normal=abs(cross(dFdx(p),dFdy(p)));
    vec3 weights=pow(normal,vec3(4.));
    weights/=max(weights.x+weights.y+weights.z,1e-20);
    if(dot(weights,vec3(1.))<.5)weights=vec3(0.,0.,1.);
    // Blend premultiplied samples, then restore straight alpha to avoid fringes.
    vec3 centered=p-geometry*.5;
    vec4 x=workflow_tex_sample(centered.yz+.5,vec2(1.));
    vec4 y=workflow_tex_sample(centered.xz+.5,vec2(1.));
    vec4 z=workflow_tex_sample(centered.xy+.5,vec2(1.));
    float alpha=dot(vec3(x.a,y.a,z.a),weights);
    vec3 rgb=x.rgb*x.a*weights.x+y.rgb*y.a*weights.y+z.rgb*z.a*weights.z;
    return vec4(alpha>0.?rgb/alpha:vec3(0.),alpha);
}
vec3 workflow_texture_blend(vec3 base,vec3 layer){
    if(texture_blend==1)return layer;
    if(texture_blend==2)return base*layer;
    if(texture_blend==3){
        vec3 d=mix(((16.*base-12.)*base+4.)*base,sqrt(max(base,vec3(0.))),step(vec3(.25),base));
        return mix(base-(1.-2.*layer)*base*(1.-base),base+(2.*layer-1.)*(d-base),step(vec3(.5),layer));
    }
    return workflow_overlay(base,layer);
}
vec3 texture_display_overlay(vec3 base,vec2 uv){
    if(texture_overlay_enabled==0 || texture_opacity<=0.)return base;
    vec4 info=texture2D(workflow_surface,uv);
    vec4 layer=workflow_mapped_layer(info);
    if(info.g<.5)return base;
    // SRGB8_ALPHA8 decodes RGB only; restore display RGB once, leaving alpha linear.
    float opacity=clamp(layer.a*texture_opacity,0.,1.);
    if(opacity<=0.)return base;
    vec3 color=texture_filter(workflow_srgb(layer.rgb));
    return mix(base,workflow_texture_blend(base,color),opacity);
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
