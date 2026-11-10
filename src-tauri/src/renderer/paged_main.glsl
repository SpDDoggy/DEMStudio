void main(){
    for(int layer=0;layer<7;layer++){
        pg_s[layer]=pg_resume!=0?texelFetch(pg_state_tex,ivec3(ivec2(gl_FragCoord.xy),layer),0):vec4(0.);
    }
    bool done=pg_s[5].y>3.5;
    if(pg_reset_query!=0&&!done){
        pg_s[2].w=0.;pg_s[4].w=0.;pg_s[5]=vec4(-1.,0.,0.,0.);
    }
    bool waiting=pg_s[5].y>.5&&pg_s[5].y<1.5;
    // A faulted pixel runs only with its requested page. Other pixels keep their
    // exact state, including a finished query waiting for the CPU memo copy.
    bool current_done=pg_s[4].w>.5;
    bool eligible=!done&&!current_done&&(!waiting||pg_page_slot(int(pg_s[5].x))>=0);
    if(eligible){
        pg_s[5]=vec4(-1.,0.,0.,0.);
        pg_original_main();
        if(!pg_yield){pg_s[5]=vec4(-1.,4.,0.,0.);pg_s[6]=pg_radiance;}
    }
#ifdef PG_WIDE_MRT
    pg_out0=pg_s[0];pg_out1=pg_s[1];pg_out2=pg_s[2];pg_out3=pg_s[3];
    pg_out4=pg_s[4];pg_out5=pg_s[5];pg_out6=pg_s[6];
#else
    int first=pg_bank*4;
    pg_out0=pg_s[first];pg_out1=pg_s[first+1];pg_out2=pg_s[first+2];
    pg_out3=first+3<7?pg_s[first+3]:vec4(0.);
#endif
}
