#if defined(OPT_ATMO)&&defined(OPT_SUN)
uniform vec3 k_ray,k_mie,k_ext;uniform float g_mie;uniform vec2 R_H;float density(vec3 p){return exp(-max(length(p)-R_H.x,0.0));}float optic(vec3 o,vec3 dir,float t,float end){float len=(end-t)/8.0;t+=0.5*len;float sum=0.0;for(int i=0;i<8;i++,t+=len){sum+=density(o+dir*t);}return sum*len;}vec3 in_scatter(vec3 dir,vec3 l){vec3 o=vec3(0.0,0.0,R_H.x);float len=ray_vs_sphere_exit(o,dir,R_H.y)/16.0;float n=0.0;float seg_t=0.0;vec3 sum=vec3(0.0);for(int i=0;i<16;i++,seg_t+=len){float t=len*jitter1(dices[i].z);vec3 p=o+dir*(seg_t+t);float den=density(p);float n0=den*t;float n1=optic(p,l,0.0,ray_vs_sphere_exit(p,l,R_H.y));sum+=den*exp(-(n+n0+n1)*k_ext);n+=den*len;}sum*=len;float c=dot(dir,l);float cc=c*c;vec3 scatter=sum*(k_ray*phase_ray(cc)+k_mie*phase_mie(g_mie,c,cc));return scatter;}vec3 out_scatter(vec3 dir){vec3 o=vec3(0.0,0.0,R_H.x);float n=optic(o,dir,0.0,ray_vs_sphere_exit(o,dir,R_H.y));return exp(-n*k_ext);}
#else
vec3 out_scatter(vec3 dir){return vec3(1.0);}
#endif
uniform vec2 light_mask_sun,light_mask_sky,light_mask_area;uniform vec4 surf_mask_diffuse,surf_mask_ggx,surf_mask_cloud,surf_mask_air;uniform float env_r;struct light_sample_t{vec4 o;vec4 n;vec3 L;float inv_pdf;vec2 mask;vec2 padding;}LIGHT0,LIGHT1;void init_light_sample(out light_sample_t light){light.o=vec4(0.0);light.n=vec4(0.0);light.L=vec3(0.0);light.inv_pdf=0.0;light.mask=vec2(0.0);}uniform vec3 sun_I;uniform mat3 sun_rot;uniform float sun_cmax;uniform vec2 sun_r;void sample_light_sun(out light_sample_t light){vec3 dir=normalize(sun_rot*sample_cone_uniform(jitter2(sun_r),sun_cmax));light.o=vec4(dir,0.0);light.n=vec4(0.0);light.inv_pdf=inv_positive(pdf_cone_uniform(sun_cmax));light.L=sun_I*out_scatter(dir);light.mask=light_mask_sun;}vec3 light_sun(vec3 dir,float R0,float R1,out float pdf_o,out vec2 mask){mask=light_mask_sun;if(min(dot(dir,sun_rot[2]),1.0)<=sun_cmax){pdf_o=0.0;return vec3(0.0);}pdf_o=pdf_cone_uniform(sun_cmax);return sun_I*pdf_o*out_scatter(dir);}uniform vec3 Le;
#if defined(OPT_IBL)
uniform sampler2D tex_ibl,tex_ibl_cdf;uniform vec2 tex_ibl_size;uniform float ibl_I,ibl_total;uniform vec2 ibl_rot;uniform vec3 ibl_r;float pdf_ibl(vec3 L,vec3 dir){return(max3(L)/ibl_total)*(tex_ibl_size.x*tex_ibl_size.y)/(PI*PI2)/max(length(dir.xy),1e-6);}vec2 binary_search_2d(vec3 r){float p=r.z*ibl_total;vec2 uv=vec2(0.0);{float start=0.0;float end=tex_ibl_size.y-1.0;while(end>start+0.5){float mid=floor((start+end)*0.5);if(p<texture2D(tex_ibl_cdf,vec2(tex_ibl_size.x-0.5,mid+0.5)/tex_ibl_size).r){end=mid;}else{start=mid+1.0;}}uv.y=clamp(start,0.0,tex_ibl_size.y-1.0);}{float start=0.0;float end=tex_ibl_size.x-1.0;while(end>start+0.5){float mid=floor((start+end)*0.5);if(p<texture2D(tex_ibl_cdf,vec2(mid+0.5,uv.y+0.5)/tex_ibl_size).r){end=mid;}else{start=mid+1.0;}}uv.x=clamp(start,0.0,tex_ibl_size.x-1.0);}return(uv+r.xy)/tex_ibl_size;}void sample_light_sky(out light_sample_t light){vec2 uv=binary_search_2d(jitter3(ibl_r));vec3 IL=texture2D(tex_ibl,uv).xyz;vec3 dir=uv_to_dir(uv-ibl_rot);light.o=vec4(dir,0.0);light.n=vec4(0.0);light.inv_pdf=inv_positive(pdf_ibl(IL,dir));light.L=IL*ibl_I*light.inv_pdf;light.mask=light_mask_sky;}vec3 light_sky(vec3 dir,float R0,float R1,out float pdf_o,out vec2 mask){mask=light_mask_sky;vec2 uv=dir_to_uv(dir)+ibl_rot;vec3 IL=texture2D(tex_ibl,uv).xyz;pdf_o=pdf_ibl(IL,dir);return IL*ibl_I;}
#elif defined(OPT_ATMO)&&defined(OPT_SUN)
vec3 light_sky(vec3 dir,float R0,float R1,out float pdf_o,out vec2 mask){mask=light_mask_sky;vec3 sun_dir=normalize(sun_rot*sample_cone_uniform(jitter2(sun_r),sun_cmax));pdf_o=0.0;return sun_I*in_scatter(dir,sun_dir);}
#else
vec3 light_sky(vec3 dir,float R0,float R1,out float pdf_o,out vec2 mask){mask=light_mask_sky;pdf_o=0.0;return Le;}
#endif
#ifdef OPT_AREA
uniform vec3 area_p,area_n,area_L;uniform vec2 area_r;uniform float area_pdf;void sample_light_area(out light_sample_t light){vec2 r=jitter2(area_r);vec3 area_o=area_p+abs(area_n.zxy*r.x+area_n.yzx*r.y);area_o+=vol_origin;
#ifdef OPT_SCALE
area_o*=world_scale;
#endif
light.o=vec4(area_o,1.0);light.n=vec4(area_n,1.0);light.inv_pdf=inv_positive(area_pdf);light.L=area_L*light.inv_pdf;light.mask=light_mask_area;}
#endif
vec3 light_area(vec3 dir,vec3 K,vec4 M,out float pdf_o,out vec2 mask){mask=light_mask_area;mask.x*=is_solid(path_id.x)?0.0:1.0;pdf_o=M.z;return K*(bounce==0?M.y:M.x-BASE_EMIT);}void gen_light_samples(void){init_light_sample(LIGHT0);init_light_sample(LIGHT1);
#if defined(OPT_SUN)||defined(OPT_IBL)
float env_sel=jitter1(env_r)*(light_mask_sun.x+light_mask_sky.x);
#endif
#ifdef OPT_SUN
#ifdef OPT_IBL
if(env_sel<light_mask_sun.x)
#endif
{sample_light_sun(LIGHT0);}
#endif
#ifdef OPT_IBL
#ifdef OPT_SUN
else if(env_sel<light_mask_sun.x+light_mask_sky.x)
#endif
{sample_light_sky(LIGHT0);}
#endif
#ifdef OPT_AREA
{sample_light_area(LIGHT1);}
#endif
media_amount=jitter_att(dices[0].w);}

