//! Smooth reconstruction used by the bicubic option; bilinear stays on the original grid.
use super::RenderParams;
pub(super) fn enabled()->bool {
 match std::env::var("DEMSTUDIO_REFINED_BILINEAR").as_deref(){Ok("on")=>true,Ok("off")=>false,_=>true}
}
pub(super) fn active(p:&RenderParams)->bool {enabled() && p.surface=="bicubic" && p.height_step==0.}
pub(super) fn cubic(v:[f32;4],t:f32)->f32 {
 (v[1]+0.5*t*(v[2]-v[0]+t*(2.*v[0]-5.*v[1]+4.*v[2]-v[3]+t*(3.*(v[1]-v[2])+v[3]-v[0])))).clamp(0.,1.)
}
pub(super) fn bspline_enabled()->bool {
 match std::env::var("DEMSTUDIO_BILINEAR_RECONSTRUCTION").as_deref() {
  Ok("bspline")=>true,Ok("catmull")=>false,
  _=>true,
 }
}
fn bspline(v:[f32;4],t:f32)->f32 {
 let s=1.-t;let t2=t*t;let t3=t2*t;
 (v[0]*s*s*s+v[1]*(3.*t3-6.*t2+4.)+v[2]*(-3.*t3+3.*t2+3.*t+1.)+v[3]*t3)/6.
}
pub(super) fn patch(v:[[f32;4];4],tx:f32,ty:f32)->f32 {patch_with_basis(v,tx,ty,false)}
pub(super) fn patch_with_basis(v:[[f32;4];4],tx:f32,ty:f32,smooth:bool)->f32 {
 if v.iter().flatten().any(|x|*x==0.) {return v[if ty<0.5{1}else{2}][if tx<0.5{1}else{2}];}
 if smooth {bspline(v.map(|r|bspline(r,tx)),ty).clamp(1e-6,1.)} else {cubic(v.map(|r|cubic(r,tx)),ty).max(1e-6)}
}
/// Input and output are normalized, curved, top-down samples. Pixel centers
/// preserve source extent; the hierarchy is rebuilt from actual fine heights.
pub(super) fn refine(base:&[f32],w:u32,h:u32)->Vec<f32> {
 let smooth=bspline_enabled();
 let fw=w*2;let fh=h*2;let mut out=vec![0.;(fw*fh)as usize];
 for y in 0..fh {let py=y as f32*0.5-0.25;let iy=py.floor()as i64;let ty=py-py.floor();
 for x in 0..fw {let px=x as f32*0.5-0.25;let ix=px.floor()as i64;let tx=px-px.floor();let mut v=[[0.;4];4];
 for j in 0..4 {for i in 0..4 {let sx=(ix+i as i64-1).clamp(0,w as i64-1)as u32;let sy=(iy+j as i64-1).clamp(0,h as i64-1)as u32;v[j][i]=base[(sy*w+sx)as usize];}}
 out[(y*fw+x)as usize]=patch_with_basis(v,tx,ty,smooth);
 }}out
}
