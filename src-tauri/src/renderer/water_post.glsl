// Bounded horizontal inundation; display-space sky/light approximation.
uniform int water_enabled;
uniform float water_level,water_opacity,water_roughness;
uniform vec3 water_color,water_sun_direction,water_sky_color,water_sun_color;
vec4 scene_display_water(vec4 source,vec3 origin,vec3 dir,inout float distance){
    if(water_enabled==0||water_opacity<=0.||abs(dir.z)<1e-7)return source;
    float h=clamp(water_level,0.,1.),curve=max(cloud_height_curve,1e-6);
    h=(h/(curve*(1.-h)+h))*(1.-1e-6)+1e-6;
    float z=cloud_height_offset+h*cloud_map_geometry.z;
    float t=(z-origin.z)/dir.z;
    if(t<=0.||(distance>=0.&&t>=distance-1e-5))return source;
    vec3 hit=origin+dir*t;
    vec2 local=(hit.xy-vec2(float(cloud_map_origin)))/cloud_map_geometry.xy;
    // Equivalent full/tiled camera arithmetic may differ by a few ulps at DEM edges.
    if(any(lessThan(local,vec2(-1e-6)))||any(greaterThan(local,vec2(1.000001))))return source;
    // Do not create a sheet above missing terrain or outside the DEM footprint.
    if(distance<0.||source.a<=0.)return source;
    float depth=max(0.,(distance-t)*abs(dir.z));
    float scale=max(abs(cloud_map_geometry.z),1e-4);
    float absorption=1.-exp(-depth/scale*12.);
    float fresnel=.02+.98*pow(1.-clamp(abs(dir.z),0.,1.),5.);
    vec3 body=mix(source.rgb,scene_filter(water_color),.25+.75*absorption);
    vec3 normal=vec3(0.,0.,dir.z<0.?1.:-1.);
    vec3 halfway=water_sun_direction-dir;
    halfway/=max(length(halfway),1e-6);
    float rough=clamp(water_roughness,.02,1.);
    float highlight=pow(max(dot(normal,halfway),0.),mix(900.,8.,rough*rough));
    vec3 color=mix(body,scene_filter(water_sky_color),fresnel*.8)+scene_filter(water_sun_color)*highlight*(1.-rough)*.5;
    float alpha=clamp(water_opacity,0.,1.);
    distance=t;
    return vec4(mix(source.rgb,color,alpha),source.a);
}
