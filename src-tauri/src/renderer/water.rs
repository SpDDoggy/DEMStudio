//! Horizontal visual inundation. No hydraulic connectivity or fluid simulation.
use super::RenderParams;
pub(crate) fn validate(p:&RenderParams)->Result<(),String>{
 for(value,min,max)in[(p.water_level,0.,100.),(p.water_opacity,0.,100.),(p.water_roughness,0.02,1.)]{
  if !value.is_finite()||value<min||value>max{return Err("无效的水面参数".into());}
 }
 let c=p.water_color.as_bytes();
 if c.len()!=7||c[0]!=b'#'||!c[1..].iter().all(u8::is_ascii_hexdigit){return Err("无效的水面颜色".into());}Ok(())
}
