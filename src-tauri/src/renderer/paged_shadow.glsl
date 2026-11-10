float pg_shadow(vec3 o,vec3 dir,float d){
    if(pg_yield)return 1.;
    int query=pg_ordinal++;
    if(query<pg_query_index)return pg_memo(pg_memo_b,query).z;
    if(query>pg_query_index){pg_yield=true;return 1.;}
    if(pg_s[4].w>.5)return pg_s[4].z;
    if(d>0.)d=max(d-MIN,0.);
    float att=1.;
#ifdef OPT_GROUND
    att=ray_vs_ground_shadow(o,dir,vec2(OFFSET,d));
#endif
    if(att>0.){
        float distance;vec3 normal;vec2 id;
        if(!pg_walk(o,dir,vec2(OFFSET,d),true,distance,normal,id,att))return 1.;
    }
#ifdef OPT_FOG
    if(att>0.)att=expo_fog_att(o,dir,d);
#endif
    pg_s[3]=vec4(0.);pg_s[4]=vec4(0.,0.,att,1.);pg_s[5]=vec4(-1.,3.,1.,0.);
    return att;
}
