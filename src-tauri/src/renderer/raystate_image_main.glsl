void main(){
    if(!rs_batch_enabled())return;
    ivec2 coord=ivec2(gl_FragCoord.xy);
    bool resume=rs_batch_resume();
    if(resume){
        vec4 status=imageLoad(rs_state_image,ivec3(coord,5));
        bool done=status.y>3.5;
        bool waiting=status.y>.5&&status.y<1.5;
        if(done||(waiting&&pg_page_slot(int(status.x))<0))return;
    }
    for(int layer=0;layer<RS_MUTABLE_LAYERS;++layer)
        pg_s[layer]=resume?imageLoad(rs_state_image,ivec3(coord,layer)):vec4(0.);
    pg_s[5]=vec4(-1.,0.,0.,0.);rs_run();
#ifdef RS_DONE_MINIMAL_STORE
    // Completed paths are never resumed. Consumers need only completion and
    // radiance; primary memo layers 17/18 were written at their producer.
    if(pg_s[5].y>3.5){
        imageStore(rs_state_image,ivec3(coord,5),pg_s[5]);
        imageStore(rs_state_image,ivec3(coord,6),pg_s[6]);
        return;
    }
#endif
    for(int layer=0;layer<RS_MUTABLE_LAYERS;++layer)
        imageStore(rs_state_image,ivec3(coord,layer),pg_s[layer]);
}
