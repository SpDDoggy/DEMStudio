// Coverage stays in the accumulation alpha; display colors enter after tone mapping.
uniform int scene_background_mode,scene_background_transparent,scene_ground_shadow;
uniform int scene_sky_follow,scene_background_transition,scene_layout_shadow_only;
uniform vec2 scene_sky_axis,scene_sky_full_size,scene_sky_tile_origin;
float scene_visible_sky_scale(){
    if(scene_sky_follow==0)return 1.;
    vec2 uv=(gl_FragCoord.xy+scene_sky_tile_origin)/max(scene_sky_full_size,vec2(1.));
    float extent=max(abs(scene_sky_axis.x)+abs(scene_sky_axis.y),1e-6);
    float t=clamp(.5+dot(uv-.5,scene_sky_axis)/extent,0.,1.);
    return 1.5-t;
}
bool scene_background_override(){return scene_background_transition!=0||scene_background_mode!=0||scene_background_transparent!=0;}
vec4 scene_visible_background(){return vec4(0.);}

uniform float scene_background_physical_weight;
