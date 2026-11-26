uniform int scene_background_mode,scene_background_transparent,output_tiled;
uniform vec3 scene_background_color,scene_background_color2;
uniform vec2 scene_background_axis,output_full_size,output_tile_origin;
uniform int scene_background_transition,scene_background_from_mode;
uniform float scene_background_mix;
uniform int transition_component_count;
uniform int transition_component_mode[32],transition_component_follow[32];
uniform vec3 transition_component_color[32],transition_component_color2[32];
uniform vec2 transition_component_axis[32];
uniform float transition_component_weight[32];
uniform vec3 scene_background_from_color,scene_background_from_color2;
uniform vec2 scene_background_from_axis,transition_sky_axis,transition_camera_frame;
uniform int transition_from_sky_follow,transition_sky_follow,transition_perspective;
uniform float transition_sky_model;
uniform vec3 transition_sky_radiance;
uniform sampler2D transition_sky_lut;
uniform mat3 transition_view_rot;
uniform float transition_fov,transition_sky_azimuth;
float transition_gradient(vec2 uv,vec2 axis){return clamp(.5+dot(uv-.5,axis)/max(abs(axis.x)+abs(axis.y),1e-6),0.,1.);}
// Production post source substitutes the call with its active display transform.
vec3 TRANSITION_SKY_DISPLAY(vec3 radiance){return radiance;}
vec3 transition_sky(vec2 uv,int follow){
    vec3 radiance;
    if(follow!=0){radiance=vec3(mix(.16,.002,transition_gradient(uv,transition_sky_axis)));}
    else {
        vec2 st=(uv*2.-1.)*output_full_size/min(output_full_size.x,output_full_size.y)+transition_camera_frame;
        vec3 dir=normalize(transition_view_rot*(transition_perspective!=0?vec3(st*transition_fov,-1.):vec3(0.,0.,-1.)));
        dir=normalize(vec3(dir.xy,max(abs(dir.z),.025)));
        float c=cos(transition_sky_azimuth),s=sin(transition_sky_azimuth);
        vec3 local=vec3(c*dir.x+s*dir.y,-s*dir.x+c*dir.y,dir.z);
        vec2 skyuv=1.-vec2(atan(local.y,local.x)+3.14159265359,acos(clamp(local.z,-1.,1.)))/vec2(6.28318530718,3.14159265359);
        radiance=transition_sky_radiance*(transition_sky_model>0.?mix(vec3(1.),texture2D(transition_sky_lut,skyuv).rgb,transition_sky_model):vec3(1.));
    }
    return TRANSITION_SKY_DISPLAY(radiance);
}
vec3 transition_background(vec2 uv,int mode,vec3 color,vec3 color2,vec2 axis,int follow){
    if(mode==0)return transition_sky(uv,follow);
    return mode==2?mix(color,color2,transition_gradient(uv,axis)):color;
}
vec4 scene_composite_background(vec4 value){
    if((scene_background_mode==0&&scene_background_transition==0)||scene_background_transparent!=0)return value;
    vec2 pixel=gl_FragCoord.xy+(output_tiled!=0?output_tile_origin:vec2(0.));
    vec2 uv=pixel/max(output_full_size,vec2(1.));
    vec2 axis=scene_background_axis;
    float extent=max(abs(axis.x)+abs(axis.y),1e-6);
    float t=clamp(.5+dot(uv-vec2(.5),axis)/extent,0.,1.);
    vec3 background=scene_background_mode==2?mix(scene_background_color,scene_background_color2,t):scene_background_color;
    if(scene_background_transition!=0){
        vec3 source=vec3(0.);float source_weight=0.;
        for(int i=0;i<32;++i){if(i>=transition_component_count)break;
            if(transition_component_mode[i]==0&&transition_component_follow[i]==0)continue;
            source+=transition_background(uv,transition_component_mode[i],transition_component_color[i],transition_component_color2[i],transition_component_axis[i],transition_component_follow[i])*transition_component_weight[i];
            source_weight+=transition_component_weight[i];
        }

        vec3 target=transition_background(uv,scene_background_mode,scene_background_color,scene_background_color2,scene_background_axis,transition_sky_follow);
        float target_weight=(scene_background_mode==0&&transition_sky_follow==0)?0.:1.;
        float total_weight=0.;for(int i=0;i<32;++i){if(i>=transition_component_count)break;total_weight+=transition_component_weight[i];}
        source/=max(total_weight,1e-12);source_weight/=max(total_weight,1e-12);
        float remainder=source_weight*(1.-scene_background_mix)+target_weight*scene_background_mix;
        background=(source*(1.-scene_background_mix)+target*target_weight*scene_background_mix)/max(remainder,1e-12);
    }
    float coverage=clamp(value.a,0.,1.);
    return vec4(value.rgb*coverage+background*(1.-coverage),1.);
}
