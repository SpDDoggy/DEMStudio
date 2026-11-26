uniform sampler2D tex_sky_model;
uniform int sky_model;
uniform float sky_azimuth;
uniform int sky_follow_light;
uniform float transition_sky_follow,transition_sky_model;
uniform vec3 sky_sun_direction;
vec3 scene_base_sky(vec3 dir){
    if(transition_sky_model>=0.){
        if(transition_sky_model<=0.)return Le;
        vec3 atmospheric=vec3(0.);
        if(dir.z>=0.){
            float c=cos(sky_azimuth),s=sin(sky_azimuth);
            vec3 local=vec3(c*dir.x+s*dir.y,-s*dir.x+c*dir.y,dir.z);
            atmospheric=Le*texture2D(tex_sky_model,dir_to_uv(local)).rgb;
        }
        return mix(Le,atmospheric,transition_sky_model);
    }
    if(sky_model==0)return Le;
    if(dir.z<0.)return vec3(0.);
    float c=cos(sky_azimuth),s=sin(sky_azimuth);
    vec3 local=vec3(c*dir.x+s*dir.y,-s*dir.x+c*dir.y,dir.z);
    return Le*texture2D(tex_sky_model,dir_to_uv(local)).rgb;
}
vec3 scene_sky(vec3 dir){
    vec3 value=scene_base_sky(dir);
    if(transition_sky_follow<0.&&sky_follow_light==0)return value;
    // Normalize the gradient to preserve a constant sky's mean irradiance
    // on a horizontal receiver while redistributing its incoming direction.
    float weight=(1.+.5*dot(normalize(dir),sky_sun_direction))/(1.+sky_sun_direction.z/3.);
    return transition_sky_follow>=0.?value*mix(1.,weight,transition_sky_follow):value*weight;
}
