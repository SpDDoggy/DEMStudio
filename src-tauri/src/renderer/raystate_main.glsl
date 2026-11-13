#ifndef IW_COUNT
#define IW_COUNT(i)
#define IW_BEGIN()
#define IW_FLUSH()
#endif
// State layout: 0..2 traversal cursor; 3 world result; 4 primary id;
// 5 feedback (page,status,visibility,reserved); 6 completed sample;
// 7 origin/phase; 8 direction/inv_pdf; 9 throughput/alpha;
// 10 radiance/jitter; 11 normal/bounce; 12 albedo/num_diffuse;
// 13 shadow direction/distance; 14 path mask;
// 15 matte sky direction/sun visibility; 16 matte weights/reserved.
// 17..18 immutable first-world hit/id, copied once to memo layer zero.
uniform int rs_state_bank;
const int RS_WORLD=1,RS_SURFACE=2,RS_SHADOW=3,RS_SCATTER=4;
const int RS_MATTE_SUN=5,RS_MATTE_SKY=6,RS_DONE=7;
void rs_finish(vec3 Lo,float alpha){pg_s[6]=max(vec4(Lo*alpha,alpha),0.);pg_s[7].w=float(RS_DONE);pg_s[5]=vec4(-1.,4.,0.,0.);}
void rs_clear_query(){pg_s[2].w=0.;pg_s[5]=vec4(-1.,0.,0.,0.);}
bool rs_visibility(vec3 o,vec3 dir,float d,out float att){
    if(d>0.)d=max(d-MIN,0.);att=1.;
#ifdef OPT_GROUND
    att=ray_vs_ground_shadow(o,dir,vec2(OFFSET,d));
#endif
    if(att>0.){float distance;vec3 normal;vec2 id;if(!pg_walk(o,dir,vec2(OFFSET,d),true,distance,normal,id,att))return false;}
    return true;
}
#ifdef RS_TERMINAL_VISIBILITY
vec3 rs_environment_radiance(vec3 Lo,vec3 A,vec3 dir,vec3 n,float inv_pdf){
#ifdef OPT_SUN_DISK
    if(bounce>0)Lo+=eval_light_sun(A,dir,INF,n,inv_pdf,0.,0.,path_mask.xy);
#endif
    Lo+=eval_light_sky(A,dir,INF,n,inv_pdf,0.,0.,path_mask.xy);
    return Lo;
}
bool rs_terminal_visibility(vec3 o,vec3 dir,out float att){
    att=1.;
#ifdef OPT_GROUND
    att=ray_vs_ground_shadow(o,dir,vec2(OFFSET,INF));
#endif
    if(att>0.){
        float distance;vec3 normal;vec2 id;
        if(!pg_walk(o,dir,vec2(OFFSET,INF),true,distance,normal,id,att))return false;
    }
    return true;
}
#endif
void rs_run(){
    // Recreate this sample's immutable LIGHT0 without consuming the saved RNG.
    init_rand();gen_light_samples();
    if(pg_resume==0){
        IW_COUNT(0);
        vec3 o,dir,A;sample_ray(output_st(st),o,dir,A);
        pg_s[7]=vec4(o,float(RS_WORLD));pg_s[8]=vec4(dir,0.);
        pg_s[9]=vec4(A,0.);pg_s[10]=vec4(0.,0.,0.,jitter_rand);
        pg_s[11]=vec4(0.);pg_s[12]=vec4(0.);pg_s[14]=vec4(0.,1.,0.,1.);
        rs_clear_query();
#ifdef RS_MATTE
        if(surface_pass==0&&geo_shadow_catcher!=0&&(any(lessThan(o.xy,vec2(.5)))||any(greaterThanEqual(o.xy,geo_source_size+vec2(.5))))){
            o.z=.002;pg_s[7]=vec4(o,float(RS_MATTE_SUN));
            vec3 skyDir=sample_hemi_cos(jitter2(lobes[0]));
            vec3 sun=max(LIGHT0.L,vec3(0.))*max(LIGHT0.o.z,0.)/PI;
            vec3 sky=max(scene_sky(skyDir),vec3(0.));vec3 weights=vec3(.2126,.7152,.0722);
            pg_s[15]=vec4(skyDir,0.);pg_s[16]=vec4(dot(sun,weights),dot(sky,weights),dot(sky_mean_radiance+sun_mean_radiance,weights),0.);
            pg_s[10].w=jitter_rand;
        }
#endif
    }
    jitter_rand=pg_s[10].w;bounce=int(pg_s[11].w);num_diffuse=int(pg_s[12].w);
    path_mask=pg_s[14];path_alpha=pg_s[9].w;
    // Ready phases stay local; page faults and traversal budgets still yield.
    // The transition bound limits work before the host can check cancellation.
    for(int transitions=0;transitions<8;++transitions){
        int phase=int(pg_s[7].w);vec3 o=pg_s[7].xyz,dir=pg_s[8].xyz;
        vec3 A=pg_s[9].xyz,Lo=pg_s[10].xyz;float inv_pdf=pg_s[8].w;
        if(phase==RS_DONE)return;
        if(phase==RS_WORLD){
            IW_COUNT(1);
#ifdef RS_TERMINAL_VISIBILITY
            if(surface_pass==0&&bounce>0&&(bounce>=max_bounce||num_diffuse>=max_num_diffuse)){
                float terminal_att;
                if(!rs_terminal_visibility(o,dir,terminal_att))break;
                path_alpha=1.;pg_s[9].w=path_alpha;
                if(terminal_att>0.)Lo=rs_environment_radiance(Lo,A,dir,vec3(0.),inv_pdf);
                rs_clear_query();rs_finish(Lo,path_alpha);return;
            }
#endif
            float d=INF;vec3 n=vec3(0.);vec2 id;float att;
            if(pg_primary_cached!=0&&bounce==0){IW_COUNT(7);vec4 hit=pg_memo(pg_memo_a,0);d=hit.x;n=hit.yzw;id=pg_memo(pg_memo_b,0).xy;}
            else{
                if(!pg_walk(o,dir,vec2(OFFSET,d),false,d,n,id,att))break;
#ifdef OPT_GROUND
                ray_vs_ground(o,dir,vec2(OFFSET,d),d,n,id);
#endif
            }
            pg_s[3]=vec4(d,n);pg_s[4]=vec4(id,1.,1.);
            if(bounce==0){pg_s[17]=pg_s[3];pg_s[18]=pg_s[4];}
            rs_clear_query();
            pg_s[7].w=float(RS_SURFACE);continue;
        }
        if(phase==RS_SURFACE){
            IW_COUNT(2);
            float d=pg_s[3].x;vec3 n=pg_s[3].yzw;vec2 id=pg_s[4].xy;
            if(surface_pass!=0){
                if(is_map(id.y)&&!is_n_zero(n)){
                    float t=source_height((o.z+dir.z*d-appearance_height.x)/max(appearance_height.y,1e-12));
                    vec2 tex_st=clamp((o.xy+dir.xy*d-vec2(appearance_origin))/max(appearance_extent-1.,vec2(1.)),0.,1.);
                    pg_s[6]=vec4(clamp(t,0.,1.),1.,tex_st);
                }else pg_s[6]=vec4(0.);
                pg_s[7].w=float(RS_DONE);pg_s[5]=vec4(-1.,4.,0.,0.);return;
            }
            path_alpha=1.;pg_s[9].w=path_alpha;
            if(is_empty(max(id.x,id.y))){
#ifdef RS_TERMINAL_VISIBILITY
                Lo=rs_environment_radiance(Lo,A,dir,n,inv_pdf);rs_finish(Lo,path_alpha);return;
#else
#ifdef OPT_SUN_DISK
                if(bounce>0)Lo+=eval_light_sun(A,dir,INF,n,inv_pdf,0.,0.,path_mask.xy);
#endif
                Lo+=eval_light_sky(A,dir,INF,n,inv_pdf,0.,0.,path_mask.xy);rs_finish(Lo,path_alpha);return;
#endif
            }
            if(is_n_zero(n)){rs_finish(Lo,path_alpha);return;}
            o+=dir*d;vec3 K=color(id.y,o);
#ifdef OPT_GRID
            vec4 Kg=grid_line(id,o,n);K=mix(K,Kg.xyz,Kg.w);
#endif
            if(bounce>=max_bounce||num_diffuse>=max_num_diffuse){rs_finish(Lo,path_alpha);return;}
            ++bounce;++num_diffuse;pg_s[7]=vec4(o,float(RS_SHADOW));
            pg_s[11]=vec4(n,float(bounce));pg_s[12]=vec4(K,float(num_diffuse));
            vec3 wo=LIGHT0.o.xyz-o*LIGHT0.o.w;float wd=norm(wo);pg_s[13]=vec4(wo,mix(INF,wd,LIGHT0.o.w));
            pg_s[5]=vec4(-1.,2.,0.,0.);continue;
        }
        if(phase==RS_SHADOW){
            IW_COUNT(3);
            vec3 n=pg_s[11].xyz,K=pg_s[12].xyz;mat3 rot=surface_n_to_rot(n);vec3 wi=(-dir)*rot;
            vec3 wo=LIGHT0.o.xyz-o*LIGHT0.o.w;float wd=norm(wo);
            vec2 mask=LIGHT0.mask*mix(surf_mask_diffuse.xy,surf_mask_diffuse.zw,LIGHT0.o.w);
            float inv_light_P=inv_positive(mask.x);float brdf_P=mask.y;
            inv_light_P*=max(dot(-wo,LIGHT0.n.xyz),1.-LIGHT0.n.w)*inv_positive(wd*wd);
            vec3 WL=LIGHT0.L*inv_light_P;
            if(max3(WL)>0.){float pdf_brdf;vec3 brdf=mtl_diffuse_brdf(wi,wo*rot,pdf_brdf,K,0.);
                if(max3(brdf)>0.){float att;if(!rs_visibility(o,pg_s[13].xyz,pg_s[13].w,att))break;
                    WL*=A*brdf*att/(1.+pdf_brdf*brdf_P*LIGHT0.inv_pdf*inv_light_P);
                    Lo+=(bounce>1?clamp_scale(WL,clamp_L):WL);
                }
            }
            pg_s[10].xyz=Lo;rs_clear_query();pg_s[7].w=float(RS_SCATTER);continue;
        }
        if(phase==RS_SCATTER){
            IW_COUNT(4);
            mat3 rot=surface_n_to_rot(pg_s[11].xyz);vec3 wi=(-dir)*rot;vec3 wo;float pdf_o;
            A*=sample_mtl_diffuse_brdf(bounce,wi,wo,pdf_o,pg_s[12].xyz,0.);
            A*=step_greater_zero(dot_n(wo));wo=rot*wo;inv_pdf=inv_positive(pdf_o);path_mask=surf_mask_diffuse;
            pg_s[8]=vec4(wo,inv_pdf);pg_s[9].xyz=A;pg_s[14]=path_mask;pg_s[7].w=float(RS_WORLD);
            pg_s[5]=vec4(-1.,2.,0.,0.);continue;
        }
#ifdef RS_MATTE
        if(phase==RS_MATTE_SUN||phase==RS_MATTE_SKY){
#ifdef IW_WORK
            if(phase==RS_MATTE_SUN){IW_COUNT(5);}else{IW_COUNT(6);}
#endif
            vec3 lightDir=phase==RS_MATTE_SUN?LIGHT0.o.xyz:pg_s[15].xyz;float att;
            if(!rs_visibility(o,lightDir,INF,att))break;rs_clear_query();
            if(phase==RS_MATTE_SUN){pg_s[15].w=att;pg_s[7].w=float(RS_MATTE_SKY);pg_s[5]=vec4(-1.,2.,0.,0.);continue;}
            float total=pg_s[16].z;
            float alpha=total>1e-8?max((pg_s[16].x*(1.-pg_s[15].w)+pg_s[16].y*(1.-att))/total,0.):0.;
            pg_s[6]=vec4(0.,0.,0.,alpha);pg_s[7].w=float(RS_DONE);pg_s[5]=vec4(-1.,4.,0.,0.);return;
        }
#endif
    }
    pg_s[10].w=jitter_rand;
}
void main(){
    for(int layer=0;layer<19;++layer)pg_s[layer]=pg_resume!=0?texelFetch(pg_state_tex,ivec3(ivec2(gl_FragCoord.xy),layer),0):vec4(0.);
    bool done=pg_s[5].y>3.5;bool waiting=pg_s[5].y>.5&&pg_s[5].y<1.5;
    if(!done&&(!waiting||pg_page_slot(int(pg_s[5].x))>=0)){pg_s[5]=vec4(-1.,0.,0.,0.);rs_run();}
#ifdef PG_WIDE_MRT
    int first=rs_state_bank*7;
#else
    int first=rs_state_bank*4;
#endif
    pg_out0=first<19?pg_s[first]:vec4(0.);pg_out1=first+1<19?pg_s[first+1]:vec4(0.);
    pg_out2=first+2<19?pg_s[first+2]:vec4(0.);pg_out3=first+3<19?pg_s[first+3]:vec4(0.);
#ifdef PG_WIDE_MRT
    pg_out4=first+4<19?pg_s[first+4]:vec4(0.);pg_out5=first+5<19?pg_s[first+5]:vec4(0.);pg_out6=first+6<19?pg_s[first+6]:vec4(0.);
#endif
}
