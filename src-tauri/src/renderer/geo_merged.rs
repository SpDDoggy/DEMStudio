//! Opt-in original Geo four-fetch maximum premerge; no new rejection or bounds.
use glow::HasContext;
pub(super) const LOW_BYTES:u64=544_000;
pub(super) fn coarse_bytes(padded:u32)->Option<u64>{if padded<32||!padded.is_power_of_two(){return None;}let mut edge=u64::from(padded/32);let mut bytes=0;loop{bytes+=edge*edge*4;if edge==1{return Some(bytes);}edge/=2;}}
fn original_raw()->Result<&'static str,String>{
 let source=include_str!("paged_header.glsl");let start=source.find("float pg_raw(").ok_or("Geo merged lacks pg_raw")?;let open=start+source[start..].find('{').unwrap();let mut depth=0;
 for(i,c)in source[open..].char_indices(){if c=='{'{depth+=1;}else if c=='}'{depth-=1;if depth==0{return Ok(&source[start..open+i+1]);}}}Err("Geo merged pg_raw braces invalid".into())
}
pub(super) fn shared_source()->Result<String,String>{
 let template=include_str!("geo_merged.glsl");let shared=template.split("// GM_COMPUTE_MAIN").next().unwrap();
 // Keep the original function text. Its inverse-size uniform is hoisted to the
 // original wrapper declaration so GLSL can see it before pg_texture_mipmap.
 let geo=super::gl_renderer::GEO_HEIGHT_SAMPLER.replace("uniform vec2 mipmap_tex_inv_size;","");
 Ok(shared.replace("__PG_RAW__",original_raw()?).replace("__GEO_SAMPLER__",&geo))
}
fn compute_source()->Result<String,String>{let shared=shared_source()?;let main=include_str!("geo_merged.glsl").split("// GM_COMPUTE_MAIN").nth(1).unwrap();Ok(format!("#version 430 compatibility\n{shared}{main}"))}
pub(super) unsafe fn link(gl:&glow::Context,sources:&[(u32,&str)])->Result<glow::Program,String>{
 let program=gl.create_program()?;
 for(kind,source)in sources{let shader=match gl.create_shader(*kind){Ok(s)=>s,Err(e)=>{gl.delete_program(program);return Err(e);}};gl.shader_source(shader,source);gl.compile_shader(shader);if !gl.get_shader_compile_status(shader){let e=gl.get_shader_info_log(shader);gl.delete_shader(shader);gl.delete_program(program);return Err(e);}gl.attach_shader(program,shader);gl.delete_shader(shader);}
 gl.link_program(program);if !gl.get_program_link_status(program){let e=gl.get_program_info_log(program);gl.delete_program(program);return Err(e);}Ok(program)
}
pub(super) struct Saved{program:Option<glow::Program>,active:u32,low:Option<glow::Texture>,coarse:Option<glow::Texture>,image:[i32;6]}
impl Saved{
 pub(super) unsafe fn capture(gl:&glow::Context)->Self{let active=gl.get_parameter_i32(glow::ACTIVE_TEXTURE)as u32;gl.active_texture(glow::TEXTURE4);let low=gl.get_parameter_texture(glow::TEXTURE_BINDING_2D_ARRAY);gl.active_texture(glow::TEXTURE5);let coarse=gl.get_parameter_texture(glow::TEXTURE_BINDING_2D);let names=[glow::IMAGE_BINDING_NAME,glow::IMAGE_BINDING_LEVEL,glow::IMAGE_BINDING_LAYERED,glow::IMAGE_BINDING_LAYER,glow::IMAGE_BINDING_ACCESS,glow::IMAGE_BINDING_FORMAT];Self{program:gl.get_parameter_program(glow::CURRENT_PROGRAM),active,low,coarse,image:names.map(|name|gl.get_parameter_indexed_i32(name,1))}}
 pub(super) unsafe fn restore(self,gl:&glow::Context){gl.active_texture(glow::TEXTURE4);gl.bind_texture(glow::TEXTURE_2D_ARRAY,self.low);gl.active_texture(glow::TEXTURE5);gl.bind_texture(glow::TEXTURE_2D,self.coarse);gl.active_texture(self.active);gl.use_program(self.program);let texture=std::num::NonZeroU32::new(self.image[0]as u32).map(glow::NativeTexture);if texture.is_some(){gl.bind_image_texture(1,texture,self.image[1],self.image[2]!=0,self.image[3],self.image[4]as u32,self.image[5]as u32);}else{gl.bind_image_texture(1,None,0,false,0,glow::READ_ONLY,glow::R32F);}}
}
pub(super) struct GeoMerged{pub low:glow::Texture,pub coarse:glow::Texture,program:glow::Program,slots:u32,padded:u32}
impl GeoMerged{
 pub unsafe fn create(gl:&glow::Context,slots:u32,padded:u32)->Result<Self,String>{
  if slots==0||coarse_bytes(padded).is_none(){return Err("Geo merged allocation invalid".into());}let source=compute_source()?;let program=link(gl,&[(glow::COMPUTE_SHADER,&source)])?;
  let low=match gl.create_texture(){Ok(t)=>t,Err(e)=>{gl.delete_program(program);return Err(e)}};let coarse=match gl.create_texture(){Ok(t)=>t,Err(e)=>{gl.delete_texture(low);gl.delete_program(program);return Err(e)}};
  let saved=Saved::capture(gl);gl.active_texture(glow::TEXTURE4);gl.bind_texture(glow::TEXTURE_2D_ARRAY,Some(low));
  gl.tex_storage_3d(glow::TEXTURE_2D_ARRAY,4,glow::R32F,320,320,slots as i32);gl.tex_parameter_i32(glow::TEXTURE_2D_ARRAY,glow::TEXTURE_MIN_FILTER,glow::NEAREST_MIPMAP_NEAREST as i32);gl.tex_parameter_i32(glow::TEXTURE_2D_ARRAY,glow::TEXTURE_MAG_FILTER,glow::NEAREST as i32);
  gl.active_texture(glow::TEXTURE5);gl.bind_texture(glow::TEXTURE_2D,Some(coarse));let edge=padded/32;let levels=edge.ilog2()+1;gl.tex_storage_2d(glow::TEXTURE_2D,levels as i32,glow::R32F,edge as i32,edge as i32);gl.tex_parameter_i32(glow::TEXTURE_2D,glow::TEXTURE_MIN_FILTER,glow::NEAREST_MIPMAP_NEAREST as i32);gl.tex_parameter_i32(glow::TEXTURE_2D,glow::TEXTURE_MAG_FILTER,glow::NEAREST as i32);
  let error=gl.get_error();saved.restore(gl);if error!=glow::NO_ERROR{gl.delete_texture(low);gl.delete_texture(coarse);gl.delete_program(program);return Err(format!("Geo merged allocation GL{error:#x}"));}Ok(Self{low,coarse,program,slots,padded})
 }
 unsafe fn configure(&self,gl:&glow::Context,program:glow::Program,low:glow::Texture,coarse:glow::Texture,core:[i32;2],slot:u32,size:[u32;2],padded:u32){
  gl.use_program(Some(program));gl.active_texture(glow::TEXTURE4);gl.bind_texture(glow::TEXTURE_2D_ARRAY,Some(low));gl.active_texture(glow::TEXTURE5);gl.bind_texture(glow::TEXTURE_2D,Some(coarse));
  for(name,value)in[("pg_low_height",4),("pg_coarse_height",5),("gm_slot",slot as i32)]{gl.uniform_1_i32(gl.get_uniform_location(program,name).as_ref(),value);}
  gl.uniform_2_i32(gl.get_uniform_location(program,"gm_core").as_ref(),core[0],core[1]);gl.uniform_2_i32(gl.get_uniform_location(program,"pg_source_size").as_ref(),size[0]as i32,size[1]as i32);gl.uniform_2_f32(gl.get_uniform_location(program,"geo_source_size").as_ref(),size[0]as f32,size[1]as f32);gl.uniform_2_f32(gl.get_uniform_location(program,"mipmap_tex_inv_size").as_ref(),1./padded as f32,1./padded as f32);
 }
 pub unsafe fn prepare_low(&self,gl:&glow::Context,low:glow::Texture,coarse:glow::Texture,slot:u32,core:[i32;2],size:[u32;2],padded:u32)->Result<(),String>{
  if slot>=self.slots||size.contains(&0)||padded!=self.padded{return Err("Geo merged low arguments invalid".into());}let saved=Saved::capture(gl);self.configure(gl,self.program,low,coarse,core,slot,size,padded);gl.uniform_2_i32(gl.get_uniform_location(self.program,"gm_origin").as_ref(),core[0]-64,core[1]-64);
  for lod in 1..=4{let edge=640>>lod;gl.uniform_1_i32(gl.get_uniform_location(self.program,"gm_lod").as_ref(),lod);gl.uniform_2_i32(gl.get_uniform_location(self.program,"gm_edge").as_ref(),edge,edge);gl.bind_image_texture(1,Some(self.low),lod-1,false,slot as i32,glow::WRITE_ONLY,glow::R32F);gl.dispatch_compute((edge as u32).div_ceil(8),(edge as u32).div_ceil(8),1);}
  gl.memory_barrier(glow::TEXTURE_FETCH_BARRIER_BIT|glow::SHADER_IMAGE_ACCESS_BARRIER_BIT);let error=gl.get_error();saved.restore(gl);if error!=glow::NO_ERROR{return Err(format!("Geo merged low GL{error:#x}"));}Ok(())
 }
 pub unsafe fn prepare_coarse(&self,gl:&glow::Context,low:glow::Texture,coarse:glow::Texture,size:[u32;2],padded:u32)->Result<(),String>{
  if size.contains(&0)||padded!=self.padded{return Err("Geo merged coarse arguments invalid".into());}let saved=Saved::capture(gl);self.configure(gl,self.program,low,coarse,[0,0],0,size,padded);gl.uniform_2_i32(gl.get_uniform_location(self.program,"gm_origin").as_ref(),0,0);
  for lod in 5..=padded.ilog2(){let edge=padded>>lod;gl.uniform_1_i32(gl.get_uniform_location(self.program,"gm_lod").as_ref(),lod as i32);gl.uniform_2_i32(gl.get_uniform_location(self.program,"gm_edge").as_ref(),edge as i32,edge as i32);gl.bind_image_texture(1,Some(self.coarse),(lod-5)as i32,false,0,glow::WRITE_ONLY,glow::R32F);gl.dispatch_compute(edge.div_ceil(8),edge.div_ceil(8),1);}
  gl.memory_barrier(glow::TEXTURE_FETCH_BARRIER_BIT|glow::SHADER_IMAGE_ACCESS_BARRIER_BIT);let error=gl.get_error();saved.restore(gl);if error!=glow::NO_ERROR{return Err(format!("Geo merged coarse GL{error:#x}"));}Ok(())
 }
 pub unsafe fn bind(&self,gl:&glow::Context,program:glow::Program){gl.active_texture(glow::TEXTURE10);gl.bind_texture(glow::TEXTURE_2D_ARRAY,Some(self.low));gl.uniform_1_i32(gl.get_uniform_location(program,"pg_geo_merged_low").as_ref(),10);gl.active_texture(glow::TEXTURE11);gl.bind_texture(glow::TEXTURE_2D,Some(self.coarse));gl.uniform_1_i32(gl.get_uniform_location(program,"pg_geo_merged_coarse").as_ref(),11);}
 pub fn memory_bytes(&self)->u64{u64::from(self.slots)*LOW_BYTES+coarse_bytes(self.padded).unwrap()}
 pub unsafe fn destroy(self,gl:&glow::Context){gl.delete_texture(self.low);gl.delete_texture(self.coarse);gl.delete_program(self.program);}
}



