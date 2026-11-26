// Camera backdrop only; physical environment sampling stays unchanged.
vec3 scene_visible_sky_direction(vec3 dir){
    if(dir.z<0.)return dir;
    // Stay above the LUT's black lower-hemisphere interpolation footprint.
    return normalize(vec3(dir.xy,max(dir.z,.025)));
}
vec3 scene_sky_backdrop(vec3 dir){
    vec2 uv=(gl_FragCoord.xy+scene_sky_tile_origin)/max(scene_sky_full_size,vec2(1.));
    vec2 axis=scene_sky_axis;
    float extent=max(abs(axis.x)+abs(axis.y),1e-6);
    float t=clamp(.5+dot(uv-.5,axis)/extent,0.,1.);
    // One neutral, directional field for both sky misses and the shadow
    // catcher. No directional atmosphere lookup: it creates a horizon seam
    // when the camera crosses the ground plane, even with a shared backdrop.
    return vec3(mix(.002,.16,1.-t));
}
