//! Explicit opt-in horizontal Bezier row cache. Original raw sampling and transform order.
use glow::HasContext;
use super::geo_merged::{shared_source,link,Saved};
pub(super) const SLOT_BYTES:u64=514*516*16;
fn original_function(name:&str)->Result<&'static str,String>{
 let s=include_str!("bicubic.glsl");let start=s.find(name).ok_or("row cache original function missing")?;let open=start+s[start..].find('{').unwrap();let mut depth=0;
 for(i,c)in s[open..].char_indices(){if c=='{'{depth+=1;}else if c=='}'{depth-=1;if depth==0{return Ok(&s[start..open+i+1]);}}}Err("row cache function braces".into())
}
fn shared(quantized:bool)->Result<String,String>{Ok(format!("{}\n#define BC_LEAF_BOUND_TEX\n{}\nuniform vec2 bc_source_size;uniform vec2 bc_source_offset;uniform vec4 mipmap_size;uniform float mipmap_step;\n{}\n{}\n",shared_source()?,if quantized{"#define OPT_LEVEL"}else{""},original_function("float bc_raw(")?,original_function("vec4 bc_to_bezier(")?))}
pub(super) unsafe fn create_program(gl:&glow::Context,quantized:bool)->Result<glow::Program,String>{let bound=if cfg!(test)&&std::env::var("DEMSTUDIO_BICUBIC_ROW_BOUND").as_deref()==Ok("on"){if std::env::var("DEMSTUDIO_BICUBIC_CELL_BOUND").as_deref()==Ok("on"){"#define BR_BOUND_ONLY\n#define BR_CELL_BOUND\n"}else{"#define BR_BOUND_ONLY\n"}}else{""};let source=format!("#version 430 compatibility\n{bound}{}\n{}",shared(quantized)?,include_str!("bicubic_rows.glsl"));link(gl,&[(glow::COMPUTE_SHADER,&source)])}
pub(super) unsafe fn allocate(gl:&glow::Context,texture:glow::Texture,slots:usize)->Result<(),String>{
 gl.bind_texture(glow::TEXTURE_2D_ARRAY,Some(texture));gl.tex_parameter_i32(glow::TEXTURE_2D_ARRAY,glow::TEXTURE_MIN_FILTER,glow::NEAREST as i32);gl.tex_parameter_i32(glow::TEXTURE_2D_ARRAY,glow::TEXTURE_MAG_FILTER,glow::NEAREST as i32);gl.tex_parameter_i32(glow::TEXTURE_2D_ARRAY,glow::TEXTURE_MAX_LEVEL,0);
 gl.tex_image_3d(glow::TEXTURE_2D_ARRAY,0,glow::RGBA32F as i32,514,516,slots as i32,0,glow::RGBA,glow::FLOAT,glow::PixelUnpackData::Slice(None));let error=gl.get_error();if error!=glow::NO_ERROR{return Err(format!("Bicubic row allocation GL{error:#x}"));}Ok(())
}
unsafe fn configure(gl:&glow::Context,program:glow::Program,raw:glow::Texture,slot:usize,core:[i32;2],size:[u32;2],padded:u32,transform:[f32;3]){
 gl.use_program(Some(program));gl.active_texture(glow::TEXTURE4);gl.bind_texture(glow::TEXTURE_2D_ARRAY,Some(raw));gl.uniform_1_i32(gl.get_uniform_location(program,"pg_low_height").as_ref(),4);gl.uniform_1_i32(gl.get_uniform_location(program,"gm_slot").as_ref(),slot as i32);gl.uniform_2_i32(gl.get_uniform_location(program,"gm_core").as_ref(),core[0],core[1]);gl.uniform_2_i32(gl.get_uniform_location(program,"pg_source_size").as_ref(),size[0]as i32,size[1]as i32);gl.uniform_2_f32(gl.get_uniform_location(program,"bc_source_size").as_ref(),size[0]as f32,size[1]as f32);gl.uniform_2_f32(gl.get_uniform_location(program,"bc_source_offset").as_ref(),1.,1.);gl.uniform_2_f32(gl.get_uniform_location(program,"mipmap_tex_inv_size").as_ref(),1./padded as f32,1./padded as f32);gl.uniform_4_f32(gl.get_uniform_location(program,"mipmap_size").as_ref(),0.,0.,transform[0],transform[1]);gl.uniform_1_f32(gl.get_uniform_location(program,"mipmap_step").as_ref(),transform[2]);
}
fn valid_input(core:[i32;2],size:[u32;2],padded:u32,transform:[f32;3])->bool{
 transform.iter().all(|v|v.is_finite())&&transform[2]>0.&&!size.contains(&0)&&padded.is_power_of_two()&&core.iter().all(|v|*v>=0&&v%512==0)
}
pub(super) unsafe fn build(gl:&glow::Context,program:glow::Program,raw:glow::Texture,output:glow::Texture,slot:usize,core:[i32;2],size:[u32;2],padded:u32,transform:[f32;3])->Result<(),String>{
 if !valid_input(core,size,padded,transform){return Err("Bicubic row input invalid".into());}
 let saved=Saved::capture(gl);configure(gl,program,raw,slot,core,size,padded,transform);gl.bind_image_texture(1,Some(output),0,false,slot as i32,glow::WRITE_ONLY,glow::RGBA32F);gl.dispatch_compute(514u32.div_ceil(8),516u32.div_ceil(8),1);gl.memory_barrier(glow::SHADER_IMAGE_ACCESS_BARRIER_BIT|glow::TEXTURE_FETCH_BARRIER_BIT);let error=gl.get_error();saved.restore(gl);if error!=glow::NO_ERROR{return Err(format!("Bicubic row build GL{error:#x}"));}Ok(())
}


