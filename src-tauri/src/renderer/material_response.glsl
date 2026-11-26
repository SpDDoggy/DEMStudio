// BRDF*cos(theta), with a matching mixture PDF for direct-light MIS and bounces.
uniform float material_roughness, material_specular;
uniform vec3 material_specular_color;
float material_mix(){return clamp(material_specular*.5,0.,.5);}
vec3 mtl_diffuse_brdf(vec3 wi,vec3 wo,out float pdf_o,vec3 Kd,float R1){
    if(material_specular<=0.){
        if(dot_n(wo)<=0.){pdf_o=0.;return vec3(0.);}
        pdf_o=pdf_hemi_cos(wo);return Kd*pdf_o;
    }
    float ni=dot_n(wi),no=dot_n(wo);
    if(ni<=0. || no<=0.){pdf_o=0.;return vec3(0.);}
    vec3 h=normalize(wi+wo);
    float ih=max(dot(wi,h),1e-8);
    float a=max(material_roughness*material_roughness,.0025),aa=a*a;
    float D=ndf_ggx(h,aa);
    float gi=2.*ni/(ni+sqrt(aa+(1.-aa)*ni*ni));
    float go=2.*no/(no+sqrt(aa+(1.-aa)*no*no));
    vec3 F=schlick_fresnel_3(clamp(material_specular_color*material_specular,0.,1.),ih);
    pdf_o=mix(pdf_hemi_cos(wo),D*max(dot_n(h),0.)/(4.*ih),material_mix());
    return (1.-F)*Kd*pdf_hemi_cos(wo)+F*(D*gi*go/(4.*ni));
}
vec3 sample_mtl_diffuse_brdf(int bounce,vec3 wi,out vec3 wo,out float pdf_o,vec3 Kd,float R1){
    if(material_specular<=0.){
        wo=sample_hemi_cos(jitter2(lobes[bounce]));pdf_o=pdf_hemi_cos(wo);return Kd;
    }
    vec2 r=jitter2(lobes[bounce]);float chance=material_mix();
    if(r.x<chance){
        r.x/=chance;float a=max(material_roughness*material_roughness,.0025);
        vec3 h=sample_ggx(r,a*a);wo=reflect(-wi,h);
    }else{r.x=(r.x-chance)/(1.-chance);wo=sample_hemi_cos(r);}
    vec3 value=mtl_diffuse_brdf(wi,wo,pdf_o,Kd,R1);
    return pdf_o>0.?value/pdf_o:vec3(0.);
}
