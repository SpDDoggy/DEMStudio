// Final display-space clouds, after terrain appearance and before scene adjustments.
uniform vec3 cloud_view_origin,cloud_map_geometry;
uniform mat3 cloud_view_rot;
uniform float cloud_view_fov,cloud_height_offset,cloud_height_curve;
uniform vec2 cloud_camera_frame,cloud_geo_step;
uniform int cloud_perspective,cloud_ground_enabled,cloud_map_origin,cloud_bypass;
// WATER_DISPLAY
vec4 scene_display_cloud(vec4 source,vec2 uv){
    if(cloud_bypass!=0||(water_enabled==0&&(scene_cloud_density<=0.||scene_cloud_coverage<=0.)))return source;
    vec2 pixel=gl_FragCoord.xy+(output_tiled!=0?output_tile_origin:vec2(0.));
    vec2 q=(pixel*2.-output_full_size)/min(output_full_size.x,output_full_size.y)+cloud_camera_frame;
    vec3 origin=cloud_view_origin,dir;
    if(cloud_perspective!=0)dir=normalize(cloud_view_rot*vec3(q*cloud_view_fov,-1.));
    else{origin+=cloud_view_rot*vec3(q*cloud_view_fov*cloud_geo_step,0.);dir=normalize(cloud_view_rot*vec3(0.,0.,-1.));}
    vec4 info=texture2D(workflow_surface,uv);float distance=-1.;
    if(info.g>.5){
        float h=clamp((info.r-1e-6)/(1.-1e-6),0.,1.);float curve=max(cloud_height_curve,1e-6);
        h=(h/(curve*(1.-h)+h))*(1.-1e-6)+1e-6;
        vec3 hit=vec3(vec2(float(cloud_map_origin))+info.ba*cloud_map_geometry.xy,cloud_height_offset+h*cloud_map_geometry.z);
        distance=max(dot(hit-origin,dir),0.);
    }
    source=scene_display_water(source,origin,dir,distance);
    if(distance>=0.&&scene_cloud_shadow_strength>0.)source.rgb*=mix(1.,cloud_sun_transmittance(origin+dir*distance),scene_cloud_shadow_strength);
    if(scene_cloud_density<=0.||scene_cloud_coverage<=0.)return source;
    if(cloud_ground_enabled!=0&&dir.z< -1e-7){float ground_d=-origin.z/dir.z;if(ground_d>0.&&(distance<0.||ground_d<distance))distance=ground_d;}
    vec4 cloud=scene_apply_cloud(vec4(0.),origin,dir,distance);
    float alpha=cloud.a+source.a*(1.-cloud.a);
    vec3 cloud_color=cloud.a>0.?scene_filter(cloud.rgb/cloud.a):vec3(0.);
    vec3 rgb=cloud_color*cloud.a+source.rgb*source.a*(1.-cloud.a);
    return vec4(alpha>0.?rgb/alpha:vec3(0.),alpha);
}
