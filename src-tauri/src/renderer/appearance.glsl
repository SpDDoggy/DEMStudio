// Surface albedo only. Every mode enters the same diffuse/path-lighting code.
uniform int appearance_mode, appearance_ramp, appearance_discrete;
uniform int appearance_color_blend;
uniform vec2 appearance_extent, appearance_texture_size, appearance_height;
uniform float appearance_origin;
uniform vec3 appearance_colors[5], appearance_linear_base;
uniform float appearance_opacity;
vec3 appearance_linear(vec3 c){
    return mix(c/12.92,pow((c+.055)/1.055,vec3(2.4)),step(vec3(.04045),c));
}
vec3 elevation_color(float t){
    t=clamp(t,0.,1.);
    if(appearance_ramp==1)return vec3(appearance_discrete==1?min(floor(t*5.),4.)/4.:t);
    vec3 a,b,c,d,e;
    if(appearance_ramp==3){
        a=appearance_colors[0];b=appearance_colors[1];c=appearance_colors[2];
        d=appearance_colors[3];e=appearance_colors[4];
    }else if(appearance_ramp==2){
        a=vec3(.20,.25,.65);b=vec3(.15,.65,.78);c=vec3(.65,.82,.38);
        d=vec3(.98,.72,.22);e=vec3(.75,.16,.20);
    }else{
        a=vec3(.15,.32,.23);b=vec3(.43,.57,.32);c=vec3(.70,.64,.43);
        d=vec3(.61,.52,.43);e=vec3(.94,.93,.89);
    }
    float q=t*4.;
    if(appearance_discrete==1){
        int band=int(min(floor(t*5.),4.));
        if(band==0)return a;if(band==1)return b;if(band==2)return c;
        if(band==3)return d;return e;
    }
    if(q<1.)return mix(a,b,q);
    if(q<2.)return mix(b,c,q-1.);
    if(q<3.)return mix(c,d,q-2.);
    return mix(d,e,q-3.);
}
vec3 color(float id,vec3 p){
    if(!is_map(id))return K_ground.xyz;
    // Texture is composited in display RGB after the unchanged material lighting.
    if(appearance_mode==1)return K_diffuse.xyz;
    if(appearance_mode==2){
        // Color blending preserves the uncolored material's path illumination.
        // The ramp supplies hue and saturation only in the display pass.
        if(appearance_color_blend!=0)return K_diffuse.xyz;
        float t=(p.z-appearance_height.x)/max(appearance_height.y,1e-12);
        return mix(K_diffuse.xyz,appearance_linear(elevation_color(t)),appearance_opacity);
    }
    return K_diffuse.xyz;
}
