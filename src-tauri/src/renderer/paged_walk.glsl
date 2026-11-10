// Pause only at a complete traversal-iteration boundary. The current brick's
// aligned halo contains every low-LOD and bicubic footprint of this cell.
bool pg_walk(vec3 o,vec3 dir,vec2 interval,bool visibility,
             out float distance,out vec3 normal,out vec2 id,out float att){
    vec3 inv=1./dir, i_next=step(0.,inv),i_sign=i_next*2.-1.;
    vec2 e,i_pos;float lod;vec3 n,target;
    distance=interval.y;normal=vec3(0.);id=vec2(ID_AIR);att=1.;
    if(pg_s[2].w>.5){
        e=pg_s[0].xy;i_pos=pg_s[0].zw;lod=pg_s[1].x;
        n=pg_s[1].yzw;target=pg_s[2].xyz;
    }else{
        n=vec3(0.);e=ray_vs_aabb_ia(o,inv,interval,n,vol_bound[0],vol_bound[1]);
        target=mix(vol_bound[1],vol_bound[0],i_next);
        i_pos=clamp(floor(o+dir*e.x).xy,vec2(0.),mipmap_size.xy-1.);
        lod=mipmap_lod.y;
    }
    bool hit=false;
    for(int iteration=0;iteration<4096;++iteration){
        if(e.x>=e.y)break;
        lod=clamp(lod,mipmap_lod.x,mipmap_lod.y);
        float scale=exp2(lod);
        vec2 i_cell=floor(i_pos/scale)*scale;
        if(iteration>=pg_iteration_budget){
            pg_s[5]=vec4(-1.,2.,visibility?1.:0.,0.);pg_yield=true;break;
        }
        if(lod<4.5){
            int page=pg_cell_page(i_cell-pg_logical_offset());
            int slot=pg_page_slot(page);
            if(slot<0){
                pg_s[5]=vec4(float(page),1.,visibility?1.:0.,0.);pg_yield=true;break;
            }
            pg_brick_layer=slot;
            int across=(pg_source_size.x+511)/512;
            pg_brick_core=ivec2(page%across,page/across)*512;
        }
        vec2 i_end=i_cell+i_next.xy*scale;
        vec2 t=(i_end-o.xy)*inv.xy;
        float end=min(min2(t),e.y);
        float z0=o.z+dir.z*e.x,z1=o.z+dir.z*end;
#if !defined(PRIM_CUBE)
        if(lod<=mipmap_lod.x){
#ifdef PRIM_BICUBIC
            vec4 H=vec4(bc_bound(i_cell,lod));
#else
            vec4 H=vec4(heightLod(i_pos,lod),heightLod(i_pos+vec2(scale,0.),lod),
                        heightLod(i_pos+vec2(0.,scale),lod),heightLod(i_pos+vec2(scale,scale),lod));
#endif
            float h=max4(H);
            if(h>0.&&min(z0,z1)<=h){
                vec2 local_e=vec2(e.x,clamp(end,e.x,e.y));
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
            float h=heightLod(i_pos,lod);
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
        e.x=end;n=-i_sign*vec3(minMask2(t,end),0.);target=vec3(i_end,0.);
        vec2 next_pos=clamp(floor(o.xy+dir.xy*e.x),i_cell,i_cell+scale-1.)-n.xy;
        lod+=(floor(i_pos/(scale+scale))!=floor(next_pos/(scale+scale))?1.:0.);
        i_pos=next_pos;
        // Even at the hard compiler loop limit, keep the exact cursor alive.
        if(iteration==4095){pg_s[5]=vec4(-1.,2.,visibility?1.:0.,0.);pg_yield=true;}
    }
    if(pg_yield){
        pg_s[0]=vec4(e,i_pos);pg_s[1]=vec4(lod,n);pg_s[2]=vec4(target,1.);return false;
    }
    if(hit&&!visibility){
        id.y=ID_MAP;normal=n;distance=e.x;
        if(max3(abs(normal))==1.)plane_divide(dot(o,normal),dot(target,normal),dot(dir,normal),distance);
    }
    return true;
}

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
