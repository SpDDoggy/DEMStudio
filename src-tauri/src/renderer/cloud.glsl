// Finite camera-visible cloud volume, in terrain-long-edge units.
uniform float scene_cloud_density,scene_cloud_coverage,scene_cloud_height,scene_cloud_thickness,scene_cloud_scale,scene_cloud_world_scale,scene_cloud_seed;
uniform vec2 scene_cloud_offset,scene_cloud_extent;
uniform vec3 scene_cloud_color;
uniform float scene_cloud_range;
uniform int scene_cloud_seed_count;
uniform float scene_cloud_seeds[32],scene_cloud_seed_weights[32];
uniform float scene_cloud_shadow_strength;
uniform vec3 scene_cloud_sun_direction;
vec2 cloud_center(){return scene_cloud_extent*.5+scene_cloud_offset;}
float cloud_radius(){return max(.01,scene_cloud_range);}
float cloud_hash(vec3 p){p=fract(p*vec3(.1031,.1030,.0973));p+=dot(p,p.yxz+33.33);return fract((p.x+p.y)*p.z);}
float cloud_noise(vec3 p){vec3 i=floor(p),f=fract(p);f=f*f*(3.-2.*f);return mix(mix(mix(cloud_hash(i),cloud_hash(i+vec3(1,0,0)),f.x),mix(cloud_hash(i+vec3(0,1,0)),cloud_hash(i+vec3(1,1,0)),f.x),f.y),mix(mix(cloud_hash(i+vec3(0,0,1)),cloud_hash(i+vec3(1,0,1)),f.x),mix(cloud_hash(i+vec3(0,1,1)),cloud_hash(i+vec3(1,1,1)),f.x),f.y),f.z);}
float cloud_pattern(vec3 q,float seed){
    q+=vec3(seed*.137,seed*.271,seed*.079);
    float n=cloud_noise(q)*.58+cloud_noise(q*2.03+7.1)*.26+cloud_noise(q*4.07+15.3)*.12+cloud_noise(q*8.13+29.7)*.04;
    float threshold=mix(.87,.18,scene_cloud_coverage);
    return smoothstep(threshold,threshold+.19,n);
}
float cloud_field(vec3 p){
    float radius=cloud_radius();
    float border=1.-smoothstep(radius*.65,radius,length(p.xy-cloud_center()));
    float h=abs((p.z-scene_cloud_height)/max(scene_cloud_thickness*.5,.0005));
    float envelope=1.-smoothstep(.25,1.,h);
    vec3 q=(p-vec3(scene_cloud_offset,scene_cloud_height))/max(scene_cloud_scale,.01);
    float density=0.;
    if(scene_cloud_seed_count==0)density=cloud_pattern(q,scene_cloud_seed);
    else for(int i=0;i<32;i++){if(i>=scene_cloud_seed_count)break;density+=cloud_pattern(q,scene_cloud_seeds[i])*scene_cloud_seed_weights[i];}
    return density*envelope*border;
}
// Display-space approximation: trace the same cloud field from the surface to the sun.
// This is a transmittance mask, not a replacement for terrain path integration.
float cloud_sun_transmittance(vec3 surface){
    if(scene_cloud_shadow_strength<=0.||scene_cloud_density<=0.||scene_cloud_coverage<=0.||scene_cloud_sun_direction.z<=0.)return 1.;
    vec3 o=surface/max(scene_cloud_world_scale,1.);
    vec3 dir=normalize(scene_cloud_sun_direction);
    vec2 center=cloud_center();float radius=cloud_radius();
    vec3 box_min=vec3(center-vec2(radius),scene_cloud_height-scene_cloud_thickness*.5);
    vec3 box_max=vec3(center+vec2(radius),scene_cloud_height+scene_cloud_thickness*.5);
    float begin=0.,end=1e20;
    for(int axis=0;axis<3;axis++){
        if(abs(dir[axis])<1e-7){if(o[axis]<box_min[axis]||o[axis]>box_max[axis])return 1.;}
        else{float a=(box_min[axis]-o[axis])/dir[axis],b=(box_max[axis]-o[axis])/dir[axis];begin=max(begin,min(a,b));end=min(end,max(a,b));}
    }
    if(end<=begin)return 1.;
    float step_size=(end-begin)/24.,optical_depth=0.;
    for(int i=0;i<24;i++){
        optical_depth+=cloud_field(o+dir*(begin+(float(i)+.5)*step_size))*scene_cloud_density*step_size*90.;
        if(optical_depth>8.)break;
    }
    return exp(-optical_depth);
}
vec4 scene_apply_cloud(vec4 source,vec3 origin,vec3 direction,float hit_distance){
    if(scene_cloud_density<=0.||scene_cloud_coverage<=0.)return source;
    vec3 o=origin/max(scene_cloud_world_scale,1.),dir=direction;
    float half_height=scene_cloud_thickness*.5;
    float lo=scene_cloud_height-half_height,hi=scene_cloud_height+half_height;
    float begin=0.,end=hit_distance>0.?hit_distance/max(scene_cloud_world_scale,1.):1e20;
    vec2 center=cloud_center();float radius=cloud_radius();
    vec3 box_min=vec3(center-vec2(radius),lo),box_max=vec3(center+vec2(radius),hi);
    for(int axis=0;axis<3;axis++){
        if(abs(dir[axis])<1e-7){if(o[axis]<box_min[axis]||o[axis]>box_max[axis])return source;}
        else{float a=(box_min[axis]-o[axis])/dir[axis],b=(box_max[axis]-o[axis])/dir[axis];begin=max(begin,min(a,b));end=min(end,max(a,b));}
    }
    if(end<=begin)return source;
    float step_size=(end-begin)/48.;float transmittance=1.;vec3 sum=vec3(0.);
    for(int i=0;i<48;i++){
        vec3 p=o+dir*(begin+(float(i)+.5)*step_size);float density=cloud_field(p);
        float opacity=1.-exp(-density*scene_cloud_density*step_size*90.);
        // A short directional density probe gives soft cloud self shading.
        float shade=cloud_field(p+vec3(-.025,-.018,.03))*.65+cloud_field(p+vec3(-.06,-.04,.075))*.35;
        float vertical_light=clamp((p.z-lo)/max(hi-lo,.001),0.,1.);
        vec3 light=scene_cloud_color*(.53+.40*exp(-shade*3.)+.07*vertical_light);
        sum+=transmittance*opacity*light;transmittance*=1.-opacity;
        if(transmittance<.005)break;
    }
    return vec4(sum+source.rgb*transmittance,1.-transmittance*(1.-source.a));
}
