// Export-only display adjustments. All zero returns the exact incoming color.
uniform float output_exposure, output_gamma;
uniform float output_contrast, output_saturation, output_temperature;
uniform float output_shadows, output_highlights;
vec3 output_filter(vec3 color){
    if(output_contrast==0. && output_saturation==0. && output_temperature==0.
       && output_shadows==0. && output_highlights==0.
       && output_exposure==1. && output_gamma==2.2) return color;
    if(output_exposure!=1.)color*=output_exposure*output_exposure;
    if(output_gamma!=2.2)color=pow(max(color,vec3(0.)),vec3(2.2/output_gamma));
    float light=dot(color,vec3(.2126,.7152,.0722));
    float shadows=1.-smoothstep(0.,.65,light);
    float highlights=smoothstep(.35,1.,light);
    color+=vec3((output_shadows*shadows+output_highlights*highlights)*.35);
    color=(color-.5)*exp2(output_contrast)+.5;
    light=dot(color,vec3(.2126,.7152,.0722));
    color=mix(vec3(light),color,1.+output_saturation);
    color*=vec3(1.+output_temperature*.15,1.,1.-output_temperature*.15);
    return clamp(color,0.,1.);
}
