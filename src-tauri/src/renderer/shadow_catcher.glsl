// An unassociated black shadow matte. Terrain keeps the original path tracer.
// Both visibility terms use the scene's sun cone and cosine-weighted sky samples.
uniform vec3 sky_mean_radiance,sun_mean_radiance;
vec4 catch_shadow(vec3 p){
    p.z+=0.002;
    vec3 sun=max(LIGHT0.L,vec3(0.))*max(LIGHT0.o.z,0.)/PI;
    vec3 skyDir=sample_hemi_cos(jitter2(lobes[0]));
    vec3 sky=max(scene_sky(skyDir),vec3(0.));
    vec3 weights=vec3(.2126,.7152,.0722);
    float ws=dot(sun,weights),wk=dot(sky,weights);
    float visibleSun=shadow(p,LIGHT0.o.xyz,INF);
    float visibleSky=shadow(p,skyDir,INF);
    float total=dot(sky_mean_radiance+sun_mean_radiance,weights);
    // Keep the Monte Carlo estimate unclipped until after sample accumulation.
    float alpha=total>1e-8?max((ws*(1.-visibleSun)+wk*(1.-visibleSky))/total,0.):0.;
    return vec4(0.,0.,0.,alpha);
}
