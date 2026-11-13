// One original horizontal conversion per logical cell X and clamped raw Y.
layout(local_size_x=8,local_size_y=8) in;
layout(rgba32f,binding=1) writeonly uniform image2D br_output;
void main(){
    ivec2 index=ivec2(gl_GlobalInvocationID.xy);
    if(any(greaterThanEqual(index,ivec2(514,516))))return;
    pg_brick_core=gm_core;pg_brick_layer=gm_slot;
#ifdef BR_CELL_BOUND
    vec2 cell=vec2(gm_core)+vec2(index);
    vec4 rows[4];float magnitude=0.;bool valid=true;
    for(int y=0;y<4;y++){
        vec4 row;
        for(int x=0;x<4;x++)row[x]=bc_raw(cell+vec2(x-1,y-1),false);
        magnitude=max(magnitude,max(max(abs(row.x),abs(row.y)),max(abs(row.z),abs(row.w))));
        valid=valid&&all(lessThanEqual(abs(row),vec4(1e25)));
        rows[y]=bc_to_bezier(row);
    }
    float high=-1e30;
    for(int x=0;x<4;x++){
        vec4 col=bc_to_bezier(vec4(rows[0][x],rows[1][x],rows[2][x],rows[3][x]));
        valid=valid&&all(lessThanEqual(abs(col),vec4(1e25)));
        high=max(high,max(max(col.x,col.y),max(col.z,col.w)));
    }
    imageStore(br_output,index,vec4(high,magnitude,valid?1.:0.,0.));
#else
    vec2 cell=vec2(gm_core)+vec2(index.x,index.y-1);
    vec4 row;
    for(int x=0;x<4;x++)row[x]=bc_raw(cell+vec2(x-1,1.),false);
#ifdef BR_BOUND_ONLY
    float magnitude=max(max(abs(row.x),abs(row.y)),max(abs(row.z),abs(row.w)));
    bool valid=all(lessThanEqual(abs(row),vec4(1e25)));
#endif
    row=bc_to_bezier(row);
#ifdef BR_BOUND_ONLY
    valid=valid&&all(lessThanEqual(abs(row),vec4(1e25)));
    row=vec4(max(max(row.x,row.y),max(row.z,row.w)),magnitude,valid?1.:0.,0.);
#endif
    imageStore(br_output,index,row);
#endif
}
