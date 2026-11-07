// Texture/elevation display transform, after linear radiance accumulation.
// Material mode retains the original post shader. Scale all RGB
// channels together so bright colored slopes keep their chromaticity instead
// of independently clipping to the display gamut. Reinhard's shoulder remains
// monotonic for every finite positive input; black stays black.
uniform float display_mix;
vec3 relief_display(vec3 radiance){
    vec3 linear=max(radiance,vec3(0.))*expo;
    float peak=max(linear.r,max(linear.g,linear.b));
    vec3 mapped=linear/(1.+peak);
    vec3 legacy=clamp(pow(linear,vec3(gamma)),0.,1.);
    return mix(legacy,pow(mapped,vec3(gamma)),display_mix);
}
