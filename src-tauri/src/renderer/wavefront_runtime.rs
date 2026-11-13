//! Experimental runtime; ordered samples share the existing source-page cache.
use glow::HasContext;
use super::sample_wavefront::{Resources,Layout,Block};
use super::wavefront_queue::WavefrontQueue;
pub(super) struct Runtime{
 pub programs:[glow::Program;5],pub resources:Option<Resources>,pub queue:Option<WavefrontQueue>,
 table:glow::Buffer,pub stride:u32,pub target:u32,table_bytes:u64,
 pub integration_draws:u64,pub blocks:u64,pub committed_samples:u64,
}
impl Runtime{
 /// Takes ownership of all five programs, including on failure.
 pub unsafe fn create(gl:&glow::Context,programs:[glow::Program;5],values:&[f32],stride:u32,target:u32)->Result<Self,String>{
  let result=(||{
   if gl.get_parameter_i32(glow::MAX_TEXTURE_IMAGE_UNITS)<14{return Err("wavefront requires texture unit13".into());}
   if target<2||target>256||stride<14||(stride-8)%6!=0||values.len()!=stride as usize*target as usize{return Err("wavefront sample table invalid".into());}
   let table=gl.create_buffer()?;gl.bind_buffer(glow::SHADER_STORAGE_BUFFER,Some(table));
   let bytes=std::slice::from_raw_parts(values.as_ptr().cast::<u8>(),values.len()*4);
   gl.buffer_data_u8_slice(glow::SHADER_STORAGE_BUFFER,bytes,glow::DYNAMIC_DRAW);gl.bind_buffer(glow::SHADER_STORAGE_BUFFER,None);
   let error=gl.get_error();if error!=glow::NO_ERROR{gl.delete_buffer(table);return Err(format!("wavefront table GL{error:#x}"));}
   Ok(Self{programs,resources:None,queue:None,table,stride,target,table_bytes:bytes.len()as u64,integration_draws:0,blocks:0,committed_samples:0})
  })();if result.is_err(){for p in programs{gl.delete_program(p);}}result
 }
 pub unsafe fn update_table(&self,gl:&glow::Context,values:&[f32],stride:u32,target:u32)->Result<(),String>{
  if stride!=self.stride||target!=self.target||values.len()!=stride as usize*target as usize{return Err("wavefront sample table changed shape".into());}
  gl.bind_buffer(glow::SHADER_STORAGE_BUFFER,Some(self.table));let bytes=std::slice::from_raw_parts(values.as_ptr().cast::<u8>(),values.len()*4);gl.buffer_sub_data_u8_slice(glow::SHADER_STORAGE_BUFFER,0,bytes);gl.bind_buffer(glow::SHADER_STORAGE_BUFFER,None);gl.memory_barrier(glow::BUFFER_UPDATE_BARRIER_BIT|glow::SHADER_STORAGE_BARRIER_BIT);Ok(())
 }
 pub unsafe fn ensure_layout(&mut self,gl:&glow::Context,size:[u32;2])->Result<(),String>{
  let layout=Layout::new(size,4,19)?;
  if self.resources.as_ref().is_some_and(|r|r.layout==layout)&&self.queue.as_ref().is_some_and(|q|q.layout()==layout){return Ok(());}
  // Release old state/queue BEFORE allocating replacement: no hidden double budget.
  if let Some(q)=self.queue.take(){q.destroy(gl);}if let Some(r)=self.resources.take(){r.destroy(gl);}
  let resources=Resources::create(gl,layout)?;
  let queue=match WavefrontQueue::create(gl,layout){Ok(Some(q))=>q,Ok(None)=>{resources.destroy(gl);return Err("wavefront queue unsupported".into());},Err(e)=>{resources.destroy(gl);return Err(e);}};
  self.resources=Some(resources);self.queue=Some(queue);Ok(())
 }
 pub unsafe fn bind(&self,gl:&glow::Context,program:glow::Program,block:Block,primary:glow::Texture){
  let size=self.resources.as_ref().unwrap().layout.size;gl.use_program(Some(program));gl.bind_buffer_base(glow::SHADER_STORAGE_BUFFER,5,Some(self.table));
  gl.uniform_2_i32(gl.get_uniform_location(program,"sw_extent").as_ref(),size[0]as i32,size[1]as i32);
  for(name,value)in[("sw_base",block.base),("sw_count",block.count),("sw_stride",self.stride),("sw_target",self.target)]{gl.uniform_1_u32(gl.get_uniform_location(program,name).as_ref(),value);}
  gl.active_texture(glow::TEXTURE13);gl.bind_texture(glow::TEXTURE_2D_ARRAY,Some(primary));gl.uniform_1_i32(gl.get_uniform_location(program,"sw_primary_state").as_ref(),13);
 }
 pub fn memory_bytes(&self)->u64{self.table_bytes+self.resources.as_ref().and_then(|r|r.layout.state_bytes()).unwrap_or(0)+self.queue.as_ref().map(|q|q.memory_bytes()).unwrap_or(0)}
 pub unsafe fn destroy(mut self,gl:&glow::Context){if let Some(q)=self.queue.take(){q.destroy(gl);}if let Some(r)=self.resources.take(){r.destroy(gl);}gl.bind_buffer_base(glow::SHADER_STORAGE_BUFFER,5,None);gl.delete_buffer(self.table);for p in self.programs{gl.delete_program(p);}}
}
