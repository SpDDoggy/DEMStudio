int radiance(inout vec3 o,inout vec3 dir,float d,vec3 n,vec2 id,inout vec3 A,inout vec3 Lo,inout float inv_pdf_o_brdf){
#if defined(OPT_BG_A)||defined(OPT_BG_C)
if(path_alpha==0.0&&is_empty(max(id.x,id.y))){return BRDF_EMIT;}
#endif
path_alpha=1.0;
#ifdef OPT_FOG
if(is_empty(id.x)){int ref=mtl_air(o,dir,dir,d,A,Lo,air_Ks,air_et,air_ea,air_eg,inv_pdf_o_brdf);if(ref!=BRDF_REFRACT){return ref;}}
#endif
if(is_empty(max(id.x,id.y))){
#ifdef OPT_SUN_DISK
Lo+=eval_light_sun(A,dir,INF,n,inv_pdf_o_brdf,0.0,0.0,path_mask.xy);
#endif
Lo+=eval_light_sky(A,dir,INF,n,inv_pdf_o_brdf,0.0,0.0,path_mask.xy);return BRDF_EMIT;}if(is_n_zero(n)){return BRDF_EMIT;}o+=dir*d;float segment_d=d;vec3 K=color(id.y,o);vec4 M=material(id.y);
#if defined(OPT_HOR)
if(is_ground(id.y)&&num_diffuse<=0&&num_scatter<=0){float att=exp(-segment_d*hor_et);Lo+=eval_light_sky(A*(1.0-att),dir,INF,vec3(0.0),inv_pdf_o_brdf,0.0,0.0,vec2(0.0,1.0));A*=att;}
#endif
vec4 Kg=vec4(0.0);
#ifdef OPT_GRID
Kg=grid_line(id,o,n);K=mix(K,Kg.xyz,Kg.w);
#endif
return mtl_diffuse(o,n,-dir,dir,A,Lo,K,inv_pdf_o_brdf);}vec4 pt(vec3 o,vec3 dir,vec3 A){float inv_pdf_o_brdf=0.0;vec3 Lo=vec3(0.0);int ref=BRDF_REFRACT;while(ref>BRDF_EMIT){float d=INF;vec3 n=vec3(0.0);path_id=ray_vs_world(o,dir,d,n);ref=radiance(o,dir,d,n,path_id,A,Lo,inv_pdf_o_brdf);}
#ifdef OPT_LE
if(ref==BRDF_END){Lo+=eval_light_sky(A,dir,INF,vec3(0.0),inv_pdf_o_brdf,0.0,0.0,path_mask.xy);}
#endif
#if defined(OPT_BG_C)
Lo=mix(Lb,Lo,path_alpha);path_alpha=1.0;
#endif
return vec4(Lo*path_alpha,path_alpha);}varying vec2 st;void main(void){init_rand();gen_light_samples();vec3 o,dir,A;sample_ray(st,o,dir,A);gl_FragColor=max(pt(o,dir,A),0.0);}

