//! Camera-visible distance fog shared by every integration/export shader.
use super::RenderParams;
pub(super) const HEADER:&str=include_str!("fog.glsl");
pub(crate) fn validate(p:&RenderParams)->Result<(),String>{
    if !p.fog_density.is_finite()||!(0. ..=100.).contains(&p.fog_density){return Err("雾浓度应在0至100之间".into());}
    if !p.fog_start.is_finite() || !(0. ..=10000.).contains(&p.fog_start) || !p.fog_end.is_finite() || !(0.1 ..=10000.).contains(&p.fog_end) || p.fog_end<=p.fog_start {return Err("雾结束距离必须大于起始距离".into());}
    if !p.fog_height_base.is_finite() || !(-100. ..=1000.).contains(&p.fog_height_base) || !p.fog_height_falloff.is_finite() || !(0.1 ..=1000.).contains(&p.fog_height_falloff) {return Err("无效的高度雾参数".into());}
    let c=p.fog_color.as_bytes();
    if c.len()!=7||c[0]!=b'#'||!c[1..].iter().all(u8::is_ascii_hexdigit){return Err("无效的雾颜色".into());}
    Ok(())
}
pub(super) fn transform_block(block:&str)->String{
    if !block.contains("vec4 pt(vec3 o") {return block.to_owned();}
    block.replace("vec4 pt(vec3 o,vec3 dir,vec3 A){", "vec4 pt(vec3 o,vec3 dir,vec3 A){float scene_fog_distance=-1.;float scene_fog_origin_z=o.z;float scene_fog_direction_z=dir.z;bool scene_first_ray=true;")
        .replace("ref=radiance(o,dir,d,n,path_id,A,Lo,inv_pdf_o_brdf);", "if(scene_first_ray){if(is_solid(path_id.y)&&!is_ground(path_id.y))scene_fog_distance=d;scene_first_ray=false;}ref=radiance(o,dir,d,n,path_id,A,Lo,inv_pdf_o_brdf);")
        .replace("return vec4(Lo*path_alpha,path_alpha);", "Lo=scene_apply_fog(Lo,scene_fog_distance,scene_fog_origin_z,scene_fog_direction_z);return vec4(Lo*path_alpha,path_alpha);")
}
