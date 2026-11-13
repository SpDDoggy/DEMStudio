//! Optional per-resident-slot exact raw LOD0 maxima; no source IO or CPU scan.
use glow::HasContext;
pub(super) const SLOT_BYTES:u64=640*640*4;
pub(super) unsafe fn create_program(gl:&glow::Context)->Result<glow::Program,String>{
    let s=gl.create_shader(glow::COMPUTE_SHADER)?;gl.shader_source(s,include_str!("bicubic_leaf.glsl"));gl.compile_shader(s);
    if !gl.get_shader_compile_status(s){let e=gl.get_shader_info_log(s);gl.delete_shader(s);return Err(e);}
    let p=match gl.create_program(){Ok(p)=>p,Err(e)=>{gl.delete_shader(s);return Err(e)}};
    gl.attach_shader(p,s);gl.link_program(p);gl.detach_shader(p,s);gl.delete_shader(s);
    if !gl.get_program_link_status(p){let e=gl.get_program_info_log(p);gl.delete_program(p);return Err(e);}Ok(p)
}
pub(super) unsafe fn allocate(gl:&glow::Context,tex:glow::Texture,slots:usize)->Result<(),String>{
    gl.bind_texture(glow::TEXTURE_2D_ARRAY,Some(tex));
    gl.tex_parameter_i32(glow::TEXTURE_2D_ARRAY,glow::TEXTURE_MIN_FILTER,glow::NEAREST as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D_ARRAY,glow::TEXTURE_MAG_FILTER,glow::NEAREST as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D_ARRAY,glow::TEXTURE_MAX_LEVEL,0);
    gl.tex_image_3d(glow::TEXTURE_2D_ARRAY,0,glow::R32F as i32,640,640,slots as i32,0,glow::RED,glow::FLOAT,glow::PixelUnpackData::Slice(None));
    let e=gl.get_error();if e!=glow::NO_ERROR{return Err(format!("Bicubic叶界分配失败 GL {e:#x}"));}Ok(())
}
pub(super) unsafe fn build(gl:&glow::Context,program:glow::Program,raw:glow::Texture,output:glow::Texture,slot:usize,origin:[i32;2],size:[u32;2],offset:u32)->Result<(),String>{
    let previous=gl.get_parameter_program(glow::CURRENT_PROGRAM);
    let active=gl.get_parameter_i32(glow::ACTIVE_TEXTURE)as u32;gl.active_texture(glow::TEXTURE0+4);
    let texture=gl.get_parameter_texture(glow::TEXTURE_BINDING_2D_ARRAY);
    gl.bind_texture(glow::TEXTURE_2D_ARRAY,Some(raw));gl.use_program(Some(program));
    gl.uniform_1_i32(gl.get_uniform_location(program,"bl_raw").as_ref(),4);
    gl.uniform_1_i32(gl.get_uniform_location(program,"bl_slot").as_ref(),slot as i32);
    gl.uniform_1_i32(gl.get_uniform_location(program,"bl_offset").as_ref(),offset as i32);
    gl.uniform_2_i32(gl.get_uniform_location(program,"bl_origin").as_ref(),origin[0],origin[1]);
    gl.uniform_2_i32(gl.get_uniform_location(program,"bl_size").as_ref(),size[0]as i32,size[1]as i32);
    gl.bind_image_texture(1,Some(output),0,true,0,glow::WRITE_ONLY,glow::R32F);
    gl.dispatch_compute(80,80,1);
    gl.memory_barrier(glow::TEXTURE_FETCH_BARRIER_BIT|glow::SHADER_IMAGE_ACCESS_BARRIER_BIT);
    gl.bind_image_texture(1,None,0,false,0,glow::WRITE_ONLY,glow::R32F);
    gl.bind_texture(glow::TEXTURE_2D_ARRAY,texture);gl.active_texture(active);gl.use_program(previous);
    let error=gl.get_error();if error!=glow::NO_ERROR{return Err(format!("Bicubic叶界计算失败 GL {error:#x}"));}Ok(())
}



