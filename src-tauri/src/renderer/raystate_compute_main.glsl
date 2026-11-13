void main(){
    uint ordinal=gl_GlobalInvocationID.x+gl_GlobalInvocationID.y*gl_NumWorkGroups.x*256u;
    uint total=uint(rs_extent.x)*uint(rs_extent.y);
    if(rs_use_active_queue!=0){if(ordinal>=rs_active_words[0])return;}
    else if(ordinal>=total)return;
    uint pixel=rs_use_active_queue!=0?rs_active_words[4u+ordinal]:ordinal;
    if(pixel>=total)return;
    ivec2 coord=ivec2(int(pixel%uint(rs_extent.x)),int(pixel/uint(rs_extent.x)));
    rs_fragcoord=vec2(coord)+vec2(.5);
    // The initial raster pass writes the original vertex varying here. Its
    // hardware interpolation need not round like an algebraic reconstruction.
    // Resumed paths already retain their ray and do not evaluate sample_ray.
    st=pg_resume==0?imageLoad(rs_state_image,ivec3(coord,18)).xy:vec2(0.);
    for(int layer=0;layer<19;++layer)pg_s[layer]=pg_resume!=0?imageLoad(rs_state_image,ivec3(coord,layer)):vec4(0.);
    bool done=pg_s[5].y>3.5;bool waiting=pg_s[5].y>.5&&pg_s[5].y<1.5;
    if(done||(waiting&&pg_page_slot(int(pg_s[5].x))<0))return;
    pg_s[5]=vec4(-1.,0.,0.,0.);rs_run();
    for(int layer=0;layer<19;++layer)imageStore(rs_state_image,ivec3(coord,layer),pg_s[layer]);
}
