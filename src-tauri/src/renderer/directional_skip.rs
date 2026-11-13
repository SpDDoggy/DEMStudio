//! Explicit DCSF microstep candidate. No runtime selection in this module.
//! Keep original traversal advance arithmetic; reuse an envelope only inside
//! its exact same-LOD directional rectangle. Does not manufacture page demand.
use glow::HasContext;
use super::geo_merged::{link,Saved};
pub(super) fn memory_bytes(padded:u32)->Option<u64>{
    if !(32..=32768).contains(&padded)||!padded.is_power_of_two(){return None;}
    let mut edge=u64::from(padded/32);let mut bytes=0;
    loop{bytes+=edge*edge*8*4;if edge==1{return Some(bytes);}edge/=2;}
}
const BUILD:&str=r#"#version 430 core
layout(local_size_x=8,local_size_y=8) in;
uniform sampler2D dcsf_source;
uniform int dcsf_lod;
uniform ivec2 dcsf_source_size;
layout(r32f,binding=1) writeonly uniform image2DArray dcsf_output;
ivec2 direction(int d){if(d<3)return ivec2(d-1,-1);if(d<5)return ivec2(d==3?-1:1,0);return ivec2(d-6,1);}
void main(){
 ivec3 p=ivec3(gl_GlobalInvocationID);ivec3 extent=imageSize(dcsf_output);
 if(any(greaterThanEqual(p,extent)))return;
 int scale=1<<dcsf_lod;ivec2 s=direction(p.z);
 ivec2 first=p.xy+min(s*3,ivec2(0));ivec2 last=p.xy+max(s*3,ivec2(0));
 // Union of the original Geo lo/hi footprints for EVERY node in the
 // directional rectangle, including unequal XY slopes and axis crossings.
 ivec2 lo=clamp(first*scale-ivec2(1),ivec2(0),dcsf_source_size-ivec2(1))/scale;
 ivec2 hi=clamp(last*scale+ivec2(scale-1),ivec2(0),dcsf_source_size-ivec2(1))/scale;
 ivec2 source_extent=textureSize(dcsf_source,dcsf_lod-5);
 lo=clamp(lo,ivec2(0),source_extent-ivec2(1));hi=clamp(hi,ivec2(0),source_extent-ivec2(1));
 float upper=uintBitsToFloat(0xff800000u);bool valid=true;
 for(int y=0;y<5;y++)for(int x=0;x<5;x++){
  ivec2 q=lo+ivec2(x,y);if(any(greaterThan(q,hi)))continue;
  float h=texelFetch(dcsf_source,q,dcsf_lod-5).x;
  if(isnan(h)||isinf(h))valid=false;upper=max(upper,h);
 }
 if(any(greaterThan(hi-lo,ivec2(4))))valid=false;
 imageStore(dcsf_output,p,vec4(valid?upper:uintBitsToFloat(0x7f800000u)));
}
"#;
/// Input must be the original Geo Bilinear shader. No hand-maintained walker.
pub(super) fn transform(source:&str)->Result<String,String>{
    let normalized=source.replace("\r\n","\n");let source=normalized.as_str();
    if !source.contains("#define PRIM_SURF")||!source.contains("uniform vec2 geo_source_size;")
        ||source.contains("#define PRIM_BICUBIC")||source.contains("#define PG_GEO_MERGED")
        ||source.contains("#define PG_TIGHT_BOUNDS")||source.contains("#define PG_LAZY_BOUNDARY") {return Err("DCSF requires original Geo Bilinear without merged/tight/lazy".into());}
    let anchor="bool pg_walk(vec3 o,vec3 dir,vec2 interval,bool visibility,";
    if source.matches(anchor).count()!=1{return Err("DCSF walker insertion boundary changed".into());}
    let sample="#else\n#if defined(PG_LAZY_BOUNDARY) && !defined(PG_TIGHT_BOUNDS) && !defined(PRIM_CUBE)\n            float h=pg_lazy_height;\n#else\n            float h=heightLod(i_pos,lod);\n#endif\n#endif\n            if(h>0.){";
    if source.matches(sample).count()!=1{return Err("DCSF original node sampling boundary changed".into());}
    let mut result=source.replacen(anchor,&format!("{}\n{anchor}",include_str!("directional_skip.glsl")),1);
    result=result.replacen("    vec3 inv=1./dir, i_next=step(0.,inv),i_sign=i_next*2.-1.;","    dcsf_cached=false;\n    vec3 inv=1./dir, i_next=step(0.,inv),i_sign=i_next*2.-1.;",1);
    result=result.replacen(sample,"#else\n            float h;\n            if(dcsf_empty(visibility,lod,i_cell,dir.xy,z0,z1))h=0.;\n            else h=heightLod(i_pos,lod);\n#endif\n            if(h>0.){",1);
    Ok(result)
}
pub(super) struct DirectionalSkip{texture:glow::Texture,program:glow::Program,padded:u32}
impl DirectionalSkip{
 pub unsafe fn create(gl:&glow::Context,padded:u32)->Result<Self,String>{
  memory_bytes(padded).ok_or("DCSF padded must be power-of-two 32..32768")?;
  let program=link(gl,&[(glow::COMPUTE_SHADER,BUILD)])?;
  let texture=match gl.create_texture(){Ok(t)=>t,Err(e)=>{gl.delete_program(program);return Err(e);}};
  let saved=Saved::capture(gl);gl.active_texture(glow::TEXTURE4);gl.bind_texture(glow::TEXTURE_2D_ARRAY,Some(texture));
  gl.tex_storage_3d(glow::TEXTURE_2D_ARRAY,(padded.ilog2()-4)as i32,glow::R32F,(padded/32)as i32,(padded/32)as i32,8);
  gl.tex_parameter_i32(glow::TEXTURE_2D_ARRAY,glow::TEXTURE_MIN_FILTER,glow::NEAREST_MIPMAP_NEAREST as i32);gl.tex_parameter_i32(glow::TEXTURE_2D_ARRAY,glow::TEXTURE_MAG_FILTER,glow::NEAREST as i32);
  let error=gl.get_error();saved.restore(gl);if error!=glow::NO_ERROR{gl.delete_texture(texture);gl.delete_program(program);return Err(format!("DCSF allocate GL{error:#x}"));}
  Ok(Self{texture,program,padded})
 }
 pub unsafe fn build(&self,gl:&glow::Context,coarse:glow::Texture,size:[u32;2])->Result<(),String>{
  if size.contains(&0)||size.iter().any(|&n|n>self.padded){return Err("DCSF source dimensions invalid".into());}
  let saved=Saved::capture(gl);gl.use_program(Some(self.program));gl.active_texture(glow::TEXTURE5);gl.bind_texture(glow::TEXTURE_2D,Some(coarse));
  gl.uniform_1_i32(gl.get_uniform_location(self.program,"dcsf_source").as_ref(),5);
  gl.uniform_2_i32(gl.get_uniform_location(self.program,"dcsf_source_size").as_ref(),size[0]as i32,size[1]as i32);
  for lod in 5..=self.padded.ilog2(){let edge=self.padded>>lod;gl.uniform_1_i32(gl.get_uniform_location(self.program,"dcsf_lod").as_ref(),lod as i32);gl.bind_image_texture(1,Some(self.texture),(lod-5)as i32,true,0,glow::WRITE_ONLY,glow::R32F);gl.dispatch_compute(edge.div_ceil(8),edge.div_ceil(8),8);}
  gl.memory_barrier(glow::SHADER_IMAGE_ACCESS_BARRIER_BIT|glow::TEXTURE_FETCH_BARRIER_BIT);let error=gl.get_error();saved.restore(gl);if error!=glow::NO_ERROR{return Err(format!("DCSF build GL{error:#x}"));}Ok(())
 }
 /// Texture unit 11 must be free. Caller owns normal shader/resource rebinding.
 pub unsafe fn bind(&self,gl:&glow::Context,program:glow::Program,enabled:bool){gl.active_texture(glow::TEXTURE11);gl.bind_texture(glow::TEXTURE_2D_ARRAY,Some(self.texture));gl.uniform_1_i32(gl.get_uniform_location(program,"dcsf_bounds").as_ref(),11);gl.uniform_1_i32(gl.get_uniform_location(program,"dcsf_top").as_ref(),self.padded.ilog2()as i32);gl.uniform_1_i32(gl.get_uniform_location(program,"dcsf_enabled").as_ref(),i32::from(enabled));}
 pub fn bytes(&self)->u64{memory_bytes(self.padded).unwrap()}
 pub unsafe fn destroy(self,gl:&glow::Context){gl.delete_texture(self.texture);gl.delete_program(self.program);}
}



