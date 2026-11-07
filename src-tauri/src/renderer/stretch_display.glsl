#version 330 compatibility
uniform sampler2D image, surface_info, stretch_lut;
uniform vec2 image_size;
void main(){
    vec2 uv=gl_FragCoord.xy/image_size;
    vec4 color=texture2D(image,uv);
    if(texture2D(surface_info,uv).g>.5 && color.a>0.){
        color.rgb=vec3(
            texture2D(stretch_lut,vec2((color.r*255.+.5)/256.,.5)).r,
            texture2D(stretch_lut,vec2((color.g*255.+.5)/256.,.5)).g,
            texture2D(stretch_lut,vec2((color.b*255.+.5)/256.,.5)).b);
    }
    gl_FragColor=color;
}
