//! Independent shadow-contact arithmetic experiment; does not smooth geometry.
pub(super) fn enabled()->bool {
    match std::env::var("DEMSTUDIO_BILINEAR_SHADOW_PRECISION").as_deref(){
        Ok("on")=>true,Ok("off")=>false,
        _=>option_env!("DEMSTUDIO_BILINEAR_SHADOW_PRECISION_BUILD")==Some("on"),
    }
}
// u=2^-24. gamma(16) bounds the bilinear arithmetic chain. The bound
// includes construction of p.z and the convex bilinear interpolation, while
// gamma(16) guards the derivative sign. Only the t=0 contact is eligible:
// a later cell's slightly negative entry must remain an actual occluder.
const HELPER:&str=r#"
void bsp_interval(vec3 o,vec3 dir,inout vec2 e,out float s0,out float s1,vec2 cell,vec4 h,float inv){
    vec3 p=o+dir*e.x;vec2 uv=(p.xy-cell)*inv;
    float d0=bi_delta(p,h,cell,inv);
    float d1=bi_delta(o+dir*e.y,h,cell,inv);
    float d2=bi_delta(o+dir*(e.x+e.x-e.y),h,cell,inv);
    float A=d0,B=(d1-d2)*.5,C=d1-A-B;
    const float g16=9.536752259018191e-7;
    vec2 gradient=bilinear_grad(h,uv)*inv;
    float derivative=dir.z-dot(gradient,dir.xy);
    vec2 slope_bound=vec2(max(abs(h.y-h.x),abs(h.w-h.z)),max(abs(h.z-h.x),abs(h.w-h.y)));
    vec2 coordinate_magnitude=(abs(o.xy)+abs(dir.xy*e.x)+abs(cell))*inv;
    float magnitude=abs(o.z)+abs(dir.z*e.x)+bilinear(abs(h),clamp(uv,vec2(0.),vec2(1.)))
                   +dot(slope_bound,coordinate_magnitude);
    // Gradient subtraction and interpolation error, conservatively measured
    // against sample magnitudes instead of the potentially cancelled slope.
    float derivative_error=g16*(abs(dir.z)+2.*max4(abs(h))*inv*(abs(dir.x)+abs(dir.y)));
    if(e.x==0. && d0<0. && -d0<=g16*magnitude && derivative>derivative_error
       && all(greaterThanEqual(uv,vec2(0.))) && all(lessThanEqual(uv,vec2(1.)))){
        A=0.;d0=0.;d1=A+B+C;
    }
    s0=sign(d0);s1=sign(d1);
    if(s0<0.){s1=-1.;return;}
    if(B*(B+C+C)<0.){
        float mid=-B/(C+C);float sm=sign(quad(vec3(A,B,C),mid));
        if(sm<0.){e.y=mix(e.x,e.y,mid);s1=sm;}
        else{e.x=mix(e.x,e.y,mid);s0=sm;}
    }
}
"#;
pub(super) fn source(block:&str)->String {
    let old="float ray_vs_prim_shadow(vec3 o,vec3 dir,vec2 e,vec2 i_pos,vec4 h,float scale){float inv=1.0/scale;float s0,s1;bi_interval(o,dir,e,s0,s1,i_pos,h,inv);return step(0.0,min(s0,s1));}";
    assert!(block.contains(old),"bilinear shadow precision requires original shadow primitive");
    let new="float ray_vs_prim_shadow(vec3 o,vec3 dir,vec2 e,vec2 i_pos,vec4 h,float scale){float inv=1.0/scale;float s0,s1;bsp_interval(o,dir,e,s0,s1,i_pos,h,inv);return step(0.0,min(s0,s1));}";
    block.replace(old,&format!("{HELPER}\n{new}"))
}
