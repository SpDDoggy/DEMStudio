uniform sampler2D tex_sky_model;
uniform int sky_model;
uniform float sky_azimuth;
vec3 scene_sky(vec3 dir){
    if(sky_model==0)return Le;
    if(dir.z<0.)return vec3(0.);
    float c=cos(sky_azimuth),s=sin(sky_azimuth);
    vec3 local=vec3(c*dir.x+s*dir.y,-s*dir.x+c*dir.y,dir.z);
    return Le*texture2D(tex_sky_model,dir_to_uv(local)).rgb;
}
