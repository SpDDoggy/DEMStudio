// An unassociated black shadow matte. Terrain keeps the original path tracer.
// Both visibility terms use the scene's sun cone and cosine-weighted sky samples.
vec4 catch_shadow(vec3 p){
    p.z+=0.002;
    vec3 sun=max(LIGHT0.L,vec3(0.))*max(LIGHT0.o.z,0.)/PI;
    vec3 sky=max(Le,vec3(0.));
    vec3 weights=vec3(.2126,.7152,.0722);
    float ws=dot(sun,weights),wk=dot(sky,weights);
    float visibleSun=shadow(p,LIGHT0.o.xyz,INF);
    vec3 skyDir=sample_hemi_cos(jitter2(lobes[0]));
    float visibleSky=shadow(p,skyDir,INF);
    float total=ws+wk;
    float alpha=total>1e-8?clamp((ws*(1.-visibleSun)+wk*(1.-visibleSky))/total,0.,1.):0.;
    return vec4(0.,0.,0.,alpha);
}
