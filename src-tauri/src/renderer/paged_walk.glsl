#ifdef PG_BILINEAR_LEAF_GATHER
float pg_leaf_height_transform(float h){h=h*mipmap_size.z;
#ifdef OPT_LEVEL
    h=ceil(h/mipmap_step)*mipmap_step;
#endif
    return h+mipmap_size.w;
}
vec4 pg_leaf_heights(vec2 i_pos){
    vec4 H;
    if(!pg_leaf_gather_raw(i_pos,pg_logical_offset(),H)){
        return vec4(heightLod(i_pos,0.),heightLod(i_pos+vec2(1.,0.),0.),
                    heightLod(i_pos+vec2(0.,1.),0.),heightLod(i_pos+vec2(1.),0.));
    }
    return vec4(pg_leaf_height_transform(H.x),pg_leaf_height_transform(H.y),
                pg_leaf_height_transform(H.z),pg_leaf_height_transform(H.w));
}
#endif
// PG_LEAF_GATHER_END
#ifdef WP_PROFILE
#define WP_COUNT(i) if(wp_selected)wp_counts[i]++
#ifdef WP_CLOCK
#define WP_FLUSH() {if(wpc_selected){uvec2 wpc_end=clock2x32ARB();wpc_record(visibility?2u:1u,wpc_start,wpc_end);}if(wp_selected)wp_flush(visibility,wp_counts);}
#else
#define WP_FLUSH() if(wp_selected)wp_flush(visibility,wp_counts)
#endif
#else
#define WP_COUNT(i)
#define WP_FLUSH()
#endif
// Pause only at a complete traversal-iteration boundary. The current brick's
// aligned halo contains every low-LOD and bicubic footprint of this cell.
bool pg_walk(vec3 o,vec3 dir,vec2 interval,bool visibility,
             out float distance,out vec3 normal,out vec2 id,out float att){
#ifdef WP_PROFILE
    bool wp_selected=wp_sampled();
#ifdef WP_CLOCK
    bool wpc_selected=wpc_sampled();uvec2 wpc_start=uvec2(0u);if(wpc_selected)wpc_start=clock2x32ARB();
#endif
    uint wp_counts[6]=uint[6](0u,0u,0u,0u,0u,0u);
    if(pg_s[2].w>.5){WP_COUNT(1);}else{WP_COUNT(0);}
#endif
#ifdef VPD_DIAGNOSTIC
    bool vpd_original_selected=vpd_sampled();uint vpd_original_nodes=0u,vpd_original_leaves=0u;
    uint vpd_classification=3u;
    if(pg_s[2].w<=.5)vpd_classification=vpd_begin(o,dir,interval,visibility);
#endif
    vec3 inv=1./dir, i_next=step(0.,inv),i_sign=i_next*2.-1.;
    vec2 e,i_pos;float lod;vec3 n,target;
    distance=interval.y;normal=vec3(0.);id=vec2(ID_AIR);att=1.;
#ifdef WP_CAPTURE
    int wpcap_index=wp_selected?wpcap_begin(o,dir,interval,visibility):-1;
#endif
    if(pg_s[2].w>.5){
        e=pg_s[0].xy;i_pos=pg_s[0].zw;lod=pg_s[1].x;
        n=pg_s[1].yzw;target=pg_s[2].xyz;
    }else{
        n=vec3(0.);e=ray_vs_aabb_ia(o,inv,interval,n,vol_bound[0],vol_bound[1]);
        target=mix(vol_bound[1],vol_bound[0],i_next);
        i_pos=clamp(floor(o+dir*e.x).xy,vec2(0.),mipmap_size.xy-1.);
        lod=mipmap_lod.y;
#ifdef PG_ANCESTOR_SEED
        // A positive transformed minimum certifies that every original ancestor
        // only executes lod--/continue at this unchanged e.x and i_pos.
        bool seed_transform=mipmap_size.z>=0.&&!isnan(mipmap_size.z)&&!isinf(mipmap_size.z);
#ifdef OPT_LEVEL
        seed_transform=seed_transform&&mipmap_step>0.&&!isnan(mipmap_step)&&!isinf(mipmap_step);
#endif
        float seed_z0=o.z+dir.z*e.x;
        if(seed_transform&&lod>=4.&&lod<=pg_seed_top&&e.x<e.y&&!isnan(seed_z0)&&!isinf(seed_z0)){
            vec2 seed_cell=floor(i_pos/16.)*16.;int seed_page=pg_cell_page(seed_cell-pg_logical_offset());
            int seed_slot=pg_page_slot(seed_page);
            if(seed_slot>=0){
                int seed_across=(pg_source_size.x+511)/512;
                ivec2 seed_core=ivec2(seed_page%seed_across,seed_page/seed_across)*512;
                ivec2 seed_index=ivec2((seed_cell-vec2(seed_core)+vec2(64.))/16.);
                ivec3 seed_extent=textureSize(pg_ancestors_seed,0);
                if(all(greaterThanEqual(seed_index,ivec2(0)))&&all(lessThan(seed_index,seed_extent.xy))&&seed_slot<seed_extent.z){
                    float seed_h=texelFetch(pg_ancestors_seed,ivec3(seed_index,seed_slot),0).x;seed_h=seed_h*mipmap_size.z;
#ifdef OPT_LEVEL
                    seed_h=ceil(seed_h/mipmap_step)*mipmap_step;
#endif
                    seed_h=seed_h+mipmap_size.w;
                    // 2^-17 relative guard is at least 64 f32 ULPs near normal
                    // values; absolute guard covers zero/subnormal magnitudes.
                    float seed_margin=max(1e-5,max(abs(seed_z0),abs(seed_h))*7.62939453125e-6);
                    if(seed_h>0.&&!isnan(seed_h)&&!isinf(seed_h)&&seed_z0<seed_h-seed_margin){
                        pg_brick_core=seed_core;pg_brick_layer=seed_slot;lod=min(lod,3.);
                    }
                }
            }
        }
#endif
    }
    // The page table and brick binding stay fixed during this walk invocation.
    // Restart this cache on every resumed call so newly uploaded pages are seen.
    int cached_page=-1;
    bool hit=false;
    for(int iteration=0;iteration<4096;++iteration){
        if(e.x>=e.y)break;
        lod=clamp(lod,mipmap_lod.x,mipmap_lod.y);
        float scale=exp2(lod);
        vec2 i_cell=floor(i_pos/scale)*scale;
        if(iteration>=pg_iteration_budget){
            WP_COUNT(5);
            pg_s[5]=vec4(-1.,2.,visibility?1.:0.,0.);pg_yield=true;break;
        }
        if(lod<4.5){
            int page=pg_cell_page(i_cell-pg_logical_offset());
            if(page!=cached_page){
                int slot=pg_page_slot(page);
                if(slot<0){
                    WP_COUNT(4);
                    pg_s[5]=vec4(float(page),1.,visibility?1.:0.,0.);pg_yield=true;break;
                }
                pg_brick_layer=slot;
                int across=(pg_source_size.x+511)/512;
                pg_brick_core=ivec2(page%across,page/across)*512;
                cached_page=page;
            }
        }
        WP_COUNT(2);
#ifdef VPD_DIAGNOSTIC
        if(vpd_original_selected)vpd_original_nodes++;
#endif
#if defined(PG_LAZY_BOUNDARY) && !defined(PG_TIGHT_BOUNDS) && !defined(PG_GEO_MERGED) && !defined(PRIM_CUBE)
        float z0=o.z+dir.z*e.x;
        float pg_lazy_height=0.;
        if(lod>mipmap_lod.x){
            pg_lazy_height=heightLod(i_pos,lod);
            if(pg_lazy_height>0.&&z0<=pg_lazy_height){lod-=1.;continue;}
        }
#endif
        vec2 i_end=i_cell+i_next.xy*scale;
        vec2 t=(i_end-o.xy)*inv.xy;
        float end=min(min2(t),e.y);
#if defined(PG_LAZY_BOUNDARY) && !defined(PG_TIGHT_BOUNDS) && !defined(PG_GEO_MERGED) && !defined(PRIM_CUBE)
        float z1=o.z+dir.z*end;
#else
        float z0=o.z+dir.z*e.x,z1=o.z+dir.z*end;
#endif
#ifdef PG_TIGHT_BOUNDS
        bool tight_empty=false;
        if(lod>=1.&&mipmap_size.z>=0.){
            float tight_h=pg_tight_raw(i_cell,lod)*mipmap_size.z;
#ifdef OPT_LEVEL
            tight_h=ceil(tight_h/mipmap_step)*mipmap_step;
#endif
            tight_h+=mipmap_size.w;
            // Extra rejection only. Preserve original descent/cursor arithmetic
            // whenever the segment is close to the mathematical upper bound.
            float margin=max(1e-5,max(max(abs(z0),abs(z1)),abs(tight_h))*7.62939453125e-6);
            tight_empty=min(z0,z1)>tight_h+margin;
        }
        if(!tight_empty){
#endif
#if !defined(PRIM_CUBE)
        if(lod<=mipmap_lod.x){
#ifdef PRIM_BICUBIC
            vec4 H=vec4(bc_bound(i_cell,lod));
#else
#ifdef PG_BILINEAR_LEAF_GATHER
            vec4 H;
            if(lod==0.)H=pg_leaf_heights(i_pos);
            else H=vec4(heightLod(i_pos,lod),heightLod(i_pos+vec2(scale,0.),lod),
                        heightLod(i_pos+vec2(0.,scale),lod),heightLod(i_pos+vec2(scale,scale),lod));
#else
            vec4 H=vec4(heightLod(i_pos,lod),heightLod(i_pos+vec2(scale,0.),lod),
                        heightLod(i_pos+vec2(0.,scale),lod),heightLod(i_pos+vec2(scale,scale),lod));
#endif
#endif
            float h=max4(H);
            if(h>0.&&min(z0,z1)<=h){
                vec2 local_e=vec2(e.x,clamp(end,e.x,e.y));
                WP_COUNT(3);
#ifdef VPD_DIAGNOSTIC
                if(vpd_original_selected)vpd_original_leaves++;
#endif
                if(visibility){
                    att=ray_vs_prim_shadow(o,dir,local_e,i_cell,H,scale);
                    if(att<=0.){hit=true;break;}
                }else if(ray_vs_prim(o,dir,local_e,n,i_cell,H,scale,target)){
                    e.x=local_e.x;hit=true;break;
                }
            }
        }else
#endif
        {
#ifdef PG_GEO_MERGED
            float h;
            if(pg_geo_merged_valid(i_pos,lod)){
            h=pg_geo_merged_raw(i_pos,lod);h=h*mipmap_size.z;
#ifdef OPT_LEVEL
            h=ceil(h/mipmap_step)*mipmap_step;
#endif
            h=h+mipmap_size.w;
            }else{h=heightLod(i_pos,lod);}
#else
#if defined(PG_LAZY_BOUNDARY) && !defined(PG_TIGHT_BOUNDS) && !defined(PRIM_CUBE)
            float h=pg_lazy_height;
#else
            float h=heightLod(i_pos,lod);
#endif
#endif
            if(h>0.){
                if(z0<=h){
#ifdef PRIM_CUBE
                    if(lod<=mipmap_lod.x){hit=true;att=0.;break;}
#endif
                    lod-=1.;continue;
                }
                if(z1<=h){
                    e.x=(h-o.z)*inv.z;n=vec3(0.,0.,1.);target=vec3(0.,0.,h);
                    i_pos=clamp(floor(o.xy+dir.xy*e.x),i_cell,i_cell+scale-1.);
#ifdef PRIM_CUBE
                    if(lod<=mipmap_lod.x){hit=true;att=0.;break;}
#endif
                    lod-=1.;continue;
                }
            }
        }
#ifdef PG_TIGHT_BOUNDS
        }
#endif
        e.x=end;n=-i_sign*vec3(minMask2(t,end),0.);target=vec3(i_end,0.);
        vec2 next_pos=clamp(floor(o.xy+dir.xy*e.x),i_cell,i_cell+scale-1.)-n.xy;
        lod+=(floor(i_pos/(scale+scale))!=floor(next_pos/(scale+scale))?1.:0.);
        i_pos=next_pos;
        // Even at the hard compiler loop limit, keep the exact cursor alive.
        if(iteration==4095){WP_COUNT(5);pg_s[5]=vec4(-1.,2.,visibility?1.:0.,0.);pg_yield=true;}
    }
    if(pg_yield){
        pg_s[0]=vec4(e,i_pos);pg_s[1]=vec4(lod,n);pg_s[2]=vec4(target,1.);
#ifdef VPD_DIAGNOSTIC
        vpd_end(vpd_classification,visibility,hit,true);
        if(vpd_original_selected)vpd_original_counts(visibility,vpd_original_nodes,vpd_original_leaves);
#endif
#ifdef WP_CAPTURE
        wpcap_end(wpcap_index,false,true,distance,normal,id,att,wp_counts[2],wp_counts[3]);
#endif
        WP_FLUSH();return false;
    }
    if(hit&&!visibility){
        id.y=ID_MAP;normal=n;distance=e.x;
        if(max3(abs(normal))==1.)plane_divide(dot(o,normal),dot(target,normal),dot(dir,normal),distance);
    }
#ifdef VPD_DIAGNOSTIC
    vpd_end(vpd_classification,visibility,hit,false);
    if(vpd_original_selected)vpd_original_counts(visibility,vpd_original_nodes,vpd_original_leaves);
#endif
#ifdef WP_CAPTURE
    wpcap_end(wpcap_index,true,false,distance,normal,id,att,wp_counts[2],wp_counts[3]);
#endif
    WP_FLUSH();return true;
}
#undef WP_COUNT
#undef WP_FLUSH

vec2 pg_world(vec3 o,vec3 dir,inout float d,inout vec3 n){
    if(pg_yield)return vec2(ID_AIR);
    int query=pg_ordinal++;
    if(query<pg_query_index){vec4 a=pg_memo(pg_memo_a,query);d=a.x;n=a.yzw;return pg_memo(pg_memo_b,query).xy;}
    if(query>pg_query_index){pg_yield=true;return vec2(ID_AIR);}
    // Only a geographic world query has a sample-invariant primary ray.
    // Shadow-matte queries never enter this function and remain stochastic.
    if(query==0&&pg_primary_cached!=0){
        pg_s[3]=pg_memo(pg_memo_a,0);pg_s[4]=pg_memo(pg_memo_b,0);
        pg_s[5]=vec4(-1.,3.,0.,0.);
        d=pg_s[3].x;n=pg_s[3].yzw;return pg_s[4].xy;
    }
    if(pg_s[4].w>.5){d=pg_s[3].x;n=pg_s[3].yzw;return pg_s[4].xy;}
    vec2 id;float att;
    if(!pg_walk(o,dir,vec2(OFFSET,d),false,d,n,id,att))return vec2(ID_AIR);
#ifdef OPT_GROUND
    ray_vs_ground(o,dir,vec2(OFFSET,d),d,n,id);
#endif
    pg_s[3]=vec4(d,n);pg_s[4]=vec4(id,1.,1.);pg_s[5]=vec4(-1.,3.,0.,0.);
    return id;
}
