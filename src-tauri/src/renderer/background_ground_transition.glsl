vec4 scene_transition_ground(vec4 value,vec3 p){
    float w=scene_background_physical_weight;
    if(scene_background_transition==0||w<=0.||w>=1.||scene_background_transparent!=0)return value;
    vec4 matte=scene_ground_shadow!=0?scene_ground_shadow_matte(p):vec4(0.);
#ifdef OPT_GRID
    vec4 grid=grid_line(vec2(ID_AIR,ID_GROUND),p,vec3(0.,0.,1.));matte.a=grid.w*.35+matte.a*(1.-grid.w*.35);
#endif
    return value*w+matte*(1.-w);
}
