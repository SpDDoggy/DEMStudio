//! Camera-visible local cloud volume shared by interactive and export paths.
use super::RenderParams;
pub(super) const HEADER:&str=include_str!("cloud.glsl");
pub(crate) fn validate(p:&RenderParams)->Result<(),String>{
 if !p.cloud_shadow_strength.is_finite()||!(0. ..=100.).contains(&p.cloud_shadow_strength){return Err("无效的云影强度".into());}
 for (value,min,max) in [(p.cloud_coverage,0.,100.),(p.cloud_density,0.,100.),(p.cloud_height,-100.,1000.),(p.cloud_thickness,0.1,200.),(p.cloud_scale,1.,200.),(p.cloud_range,10.,500.),(p.cloud_offset_x,-200.,200.),(p.cloud_offset_y,-200.,200.)] {if !value.is_finite()||value<min||value>max{return Err("无效的云参数".into());}}
 if p.cloud_seed>9999{return Err("云种子应在0至9999之间".into());}
 if !p.transition_cloud_seeds.is_empty() {
  let components=&p.transition_cloud_seeds;
  if components.len()>32||components.iter().any(|c|c.seed>9999||!c.weight.is_finite()||!(0. ..=1.).contains(&c.weight))||(components.iter().map(|c|c.weight).sum::<f32>()-1.).abs()>0.001{return Err("无效的云种子过渡".into());}
 }
 let c=p.cloud_color.as_bytes();if c.len()!=7||c[0]!=b'#'||!c[1..].iter().all(u8::is_ascii_hexdigit){return Err("无效的云颜色".into());}Ok(())
}
