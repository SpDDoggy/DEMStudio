//! Experimental exact ancestor descent certificate. No renderer entry point.
use glow::HasContext;
use super::geo_merged::{shared_source,link,Saved};
pub(super) const SLOT_BYTES:u64=40*40*4;
fn compute_source()->Result<String,String>{Ok(format!("#version 430 compatibility\n{}\n{}",shared_source()?,include_str!("ancestor_seed.glsl")))}

pub(super) struct AncestorSeed{texture:glow::Texture,program:glow::Program,slots:u32,padded:u32}
impl AncestorSeed{
 pub unsafe fn create(gl:&glow::Context,slots:u32,padded:u32)->Result<Self,String>{
  Self::create_with_source(gl,slots,padded,&compute_source()?)
 }
 
 unsafe fn create_with_source(gl:&glow::Context,slots:u32,padded:u32,source:&str)->Result<Self,String>{
  if slots==0||padded<32||!padded.is_power_of_two(){return Err("ancestor seed allocation invalid".into());}
  let program=link(gl,&[(glow::COMPUTE_SHADER,source)])?;
  let texture=match gl.create_texture(){Ok(t)=>t,Err(e)=>{gl.delete_program(program);return Err(e);}};
  let saved=Saved::capture(gl);gl.active_texture(glow::TEXTURE4);gl.bind_texture(glow::TEXTURE_2D_ARRAY,Some(texture));gl.tex_storage_3d(glow::TEXTURE_2D_ARRAY,1,glow::R32F,40,40,slots as i32);gl.tex_parameter_i32(glow::TEXTURE_2D_ARRAY,glow::TEXTURE_MIN_FILTER,glow::NEAREST as i32);gl.tex_parameter_i32(glow::TEXTURE_2D_ARRAY,glow::TEXTURE_MAG_FILTER,glow::NEAREST as i32);let error=gl.get_error();saved.restore(gl);if error!=glow::NO_ERROR{gl.delete_texture(texture);gl.delete_program(program);return Err(format!("ancestor seed allocation GL{error:#x}"));}Ok(Self{texture,program,slots,padded})
 }
 unsafe fn configure(&self,gl:&glow::Context,program:glow::Program,low:glow::Texture,coarse:glow::Texture,slot:u32,core:[i32;2],size:[u32;2],padded:u32){
  gl.use_program(Some(program));gl.active_texture(glow::TEXTURE4);gl.bind_texture(glow::TEXTURE_2D_ARRAY,Some(low));gl.active_texture(glow::TEXTURE5);gl.bind_texture(glow::TEXTURE_2D,Some(coarse));
  for(name,value)in[("pg_low_height",4),("pg_coarse_height",5),("gm_slot",slot as i32),("ancestor_top",padded.ilog2()as i32)]{gl.uniform_1_i32(gl.get_uniform_location(program,name).as_ref(),value);}
  gl.uniform_2_i32(gl.get_uniform_location(program,"gm_core").as_ref(),core[0],core[1]);gl.uniform_2_i32(gl.get_uniform_location(program,"gm_origin").as_ref(),core[0]-64,core[1]-64);gl.uniform_2_i32(gl.get_uniform_location(program,"pg_source_size").as_ref(),size[0]as i32,size[1]as i32);gl.uniform_2_f32(gl.get_uniform_location(program,"geo_source_size").as_ref(),size[0]as f32,size[1]as f32);gl.uniform_2_f32(gl.get_uniform_location(program,"mipmap_tex_inv_size").as_ref(),1./padded as f32,1./padded as f32);
 }
 /// core is the original pg_brick_core (page*512), NOT brick halo origin.
 pub unsafe fn prepare_low(&self,gl:&glow::Context,low:glow::Texture,coarse:glow::Texture,slot:u32,core:[i32;2],size:[u32;2],padded:u32)->Result<(),String>{
  if slot>=self.slots||size.contains(&0)||padded!=self.padded||core.iter().any(|v|v%512!=0){return Err("ancestor seed input invalid".into());}
  let saved=Saved::capture(gl);self.configure(gl,self.program,low,coarse,slot,core,size,padded);gl.bind_image_texture(1,Some(self.texture),0,false,slot as i32,glow::WRITE_ONLY,glow::R32F);gl.dispatch_compute(5,5,1);gl.memory_barrier(glow::SHADER_IMAGE_ACCESS_BARRIER_BIT|glow::TEXTURE_FETCH_BARRIER_BIT);let error=gl.get_error();saved.restore(gl);if error!=glow::NO_ERROR{return Err(format!("ancestor seed build GL{error:#x}"));}Ok(())
 }
 pub unsafe fn bind(&self,gl:&glow::Context,program:glow::Program,unit:u32){gl.active_texture(glow::TEXTURE0+unit);gl.bind_texture(glow::TEXTURE_2D_ARRAY,Some(self.texture));gl.uniform_1_i32(gl.get_uniform_location(program,"pg_ancestors_seed").as_ref(),unit as i32);gl.uniform_1_f32(gl.get_uniform_location(program,"pg_seed_top").as_ref(),self.padded.ilog2()as f32);}
 pub fn memory_bytes(&self)->u64{u64::from(self.slots)*SLOT_BYTES}
 pub unsafe fn destroy(self,gl:&glow::Context){gl.delete_texture(self.texture);gl.delete_program(self.program);}
}



