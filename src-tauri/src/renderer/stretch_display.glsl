#version 330 compatibility
uniform sampler2D image, surface_info, stretch_lut;
uniform vec2 image_size;
uniform int stretch_high_precision;
vec3 precise_lut(float v){
    int code=int(floor(clamp(v,0.,1.)*65535.+.5));
    return texelFetch(stretch_lut,ivec2(code%256,code/256),0).rgb;
}
void main(){
    vec2 uv=gl_FragCoord.xy/image_size;
    vec4 color=texture2D(image,uv);
    if(texture2D(surface_info,uv).g>.5 && color.a>0.){
        if(stretch_high_precision!=0) {
            color.rgb=vec3(precise_lut(color.r).r,precise_lut(color.g).g,precise_lut(color.b).b);
        } else {
        color.rgb=vec3(
            texture2D(stretch_lut,vec2((color.r*255.+.5)/256.,.5)).r,
            texture2D(stretch_lut,vec2((color.g*255.+.5)/256.,.5)).g,
            texture2D(stretch_lut,vec2((color.b*255.+.5)/256.,.5)).b);
        }
    }
    gl_FragColor=color;
}
