//! One device-only eligible-pixel list for the unchanged async fused Fragment.
//! Command + pixel indices share binding1. No per-Draw CPU readback.
use glow::HasContext;
pub(super) const CAPACITY:u32=65536;
pub(super) const QUEUE_BYTES:u64=16+CAPACITY as u64*4;
pub(super) const ST_BYTES:u64=CAPACITY as u64*8;
pub(super) const BYTES:u64=QUEUE_BYTES+ST_BYTES;
pub(super) struct AsyncActiveQueue{program:glow::Program,buffer:glow::Buffer,capture:StCapture}
pub(super) fn validate_path(geo:bool,image:bool,asynchronous:bool,fused:bool,pair:bool,wavefront:bool)->Result<(),String>{
    validate_mode(geo,image,asynchronous,fused,pair,wavefront,false)
}
pub(super) fn validate_mode(geo:bool,image:bool,asynchronous:bool,fused:bool,pair:bool,wavefront:bool,packed:bool)->Result<(),String>{
    if !geo||!image||!asynchronous||fused==packed||pair||wavefront{
        return Err("AsyncActiveQueue仅支持Geo image async，POINTS须fused、Packed须nonfused，且与PhasePair/4lane互斥".into());
    }Ok(())
}
/// Geo exports do NOT use output_tiled: preserve original raster interpolation.
/// This is the only change to the integrator source, before GLSL compilation.
pub(super) fn fragment_source(dense:&str)->Result<String,String>{
    let mut source=super::raystate_shader::source_async_samples(dense)?;
    let old="sample_ray(output_st(st),o,dir,A);";
    let anchor="void rs_run(){";
    if source.matches(anchor).count()!=1{return Err("AsyncActiveQueue缺少rs_run".into());}
    // The retained unreachable original main also contains sample_ray; modify
    // only the actual continuation integrator, never the original reference.
    let run=source.find(anchor).unwrap();
    if source[run..].matches(old).count()!=1{return Err("AsyncActiveQueue相机初始化锚点变化".into());}
    let call=run+source[run..].find(old).unwrap();
    source.replace_range(call..call+old.len(),"sample_ray(texelFetch(aaq_interpolated_st,ivec2(gl_FragCoord.xy),0).xy,o,dir,A);");
    Ok(source.replacen(anchor,"uniform sampler2D aaq_interpolated_st;\nvoid rs_run(){",1))
}
/// Continuous physical fragment prefix, with logical coordinates restored before
/// all original per-pixel operations. The independent fixed-function commit is
/// deliberately NOT transformed and still rasterizes the original pixel grid.
pub(super) fn packed_fragment_source(dense:&str)->Result<String,String>{
    if std::env::var("DEMSTUDIO_ASYNC_FUSED_COMMIT").as_deref()==Ok("on"){return Err("PackedAsyncQueue需要FusedCommit=off".into());}
    pack_fragment(fragment_source(dense)?)
}
fn pack_fragment(mut source:String)->Result<String,String>{
    let main=source.rfind("void main(){").ok_or("PackedAsyncQueue缺少main")?;
    source.replace_range(main..main+"void main(){".len(),"void aaq_integrate_main(){");
    // Rewrite the complete original source, including async metadata helpers,
    // RNG/output_pixel/memo/diagnostics and primary producers. Append the physical
    // wrapper only afterwards, so its gl_FragCoord remains the actual raster one.
    source=source.replace("gl_FragCoord","aaq_logical_fragcoord");
    let insertion=source.lines().take_while(|line|line.starts_with("#version")||line.starts_with("#extension")).map(|line|line.len()+1).sum::<usize>();
    source.insert_str(insertion,"vec4 aaq_logical_fragcoord;\nlayout(std430,binding=1) readonly buffer AsyncPackedQueue { uint aaq_words[]; };\n");
    source.push_str(r#"
void main(){
    uvec2 physical=uvec2(gl_FragCoord.xy);
    uint index=physical.y*uint(as_extent.x)+physical.x;
    if(index>=aaq_words[0])discard;
    uint pixel=aaq_words[4u+index];
    aaq_logical_fragcoord=vec4(vec2(pixel%uint(as_extent.x),pixel/uint(as_extent.x))+vec2(.5),gl_FragCoord.zw);
    aaq_integrate_main();
}
"#);
    Ok(source)
}
fn checked_total(size:[u32;2])->Result<u32,String>{size[0].checked_mul(size[1]).filter(|&n|n>0&&n<=CAPACITY&&size.iter().all(|&v|v<=i32::MAX as u32)).ok_or_else(||"AsyncActiveQueue尺寸超出65536像素".into())}
impl AsyncActiveQueue{
    pub unsafe fn create(gl:&glow::Context)->Result<Self,String>{
        let version=gl.version();
        if version.is_embedded||(version.major,version.minor)<(4,3)||gl.get_parameter_i64(glow::MAX_SHADER_STORAGE_BLOCK_SIZE)<QUEUE_BYTES as i64||gl.get_parameter_i32(glow::MAX_TEXTURE_IMAGE_UNITS)<14{return Err("AsyncActiveQueue需要OpenGL4.3、262160字节SSBO与纹理单元13".into());}
        let shader=gl.create_shader(glow::COMPUTE_SHADER)?;
        gl.shader_source(shader,include_str!("async_active_queue.glsl"));gl.compile_shader(shader);
        if !gl.get_shader_compile_status(shader){let e=gl.get_shader_info_log(shader);gl.delete_shader(shader);return Err(e);}
        let program=match gl.create_program(){Ok(p)=>p,Err(e)=>{gl.delete_shader(shader);return Err(e)}};
        gl.attach_shader(program,shader);gl.link_program(program);gl.detach_shader(program,shader);gl.delete_shader(shader);
        if !gl.get_program_link_status(program){let e=gl.get_program_info_log(program);gl.delete_program(program);return Err(e);}
        let buffer=match gl.create_buffer(){Ok(b)=>b,Err(e)=>{gl.delete_program(program);return Err(e)}};
        gl.bind_buffer(glow::SHADER_STORAGE_BUFFER,Some(buffer));gl.buffer_data_size(glow::SHADER_STORAGE_BUFFER,QUEUE_BYTES as i32,glow::DYNAMIC_COPY);
        gl.bind_buffer(glow::SHADER_STORAGE_BUFFER,None);
        let e=gl.get_error();if e!=glow::NO_ERROR{gl.delete_buffer(buffer);gl.delete_program(program);return Err(format!("AsyncActiveQueue分配失败: GL {e:#x}"));}
        let capture=match StCapture::create(gl){Ok(c)=>c,Err(e)=>{gl.delete_buffer(buffer);gl.delete_program(program);return Err(e)}};
        Ok(Self{program,buffer,capture})
    }
    pub fn memory_bytes(&self)->u64{BYTES}
    pub unsafe fn capture_st(&self,gl:&glow::Context,original:glow::Program,size:[u32;2])->Result<(),String>{self.capture.draw(gl,original,size)}
    pub unsafe fn bind_st(&self,gl:&glow::Context,program:glow::Program){let active=gl.get_parameter_i32(glow::ACTIVE_TEXTURE)as u32;gl.active_texture(glow::TEXTURE13);gl.bind_texture(glow::TEXTURE_2D,Some(self.capture.texture));gl.uniform_1_i32(gl.get_uniform_location(program,"aaq_interpolated_st").as_ref(),13);gl.active_texture(active);}
    /// AsyncSamples::bind must have bound metadata at binding4. Leaves that binding unchanged.
    pub unsafe fn dispatch(&self,gl:&glow::Context,state:glow::Texture,pages:glow::Texture,size:[u32;2],target:u32)->Result<(),String>{
        let total=checked_total(size)?;
        let previous=gl.get_parameter_program(glow::CURRENT_PROGRAM);let active=gl.get_parameter_i32(glow::ACTIVE_TEXTURE)as u32;
        gl.active_texture(glow::TEXTURE6);let previous_state=gl.get_parameter_texture(glow::TEXTURE_BINDING_2D_ARRAY);gl.bind_texture(glow::TEXTURE_2D_ARRAY,Some(state));
        gl.active_texture(glow::TEXTURE9);let previous_pages=gl.get_parameter_texture(glow::TEXTURE_BINDING_2D);gl.bind_texture(glow::TEXTURE_2D,Some(pages));
        gl.use_program(Some(self.program));gl.bind_buffer_base(glow::SHADER_STORAGE_BUFFER,1,Some(self.buffer));
        for(name,value)in[("aaq_state",6),("aaq_pages",9)]{gl.uniform_1_i32(gl.get_uniform_location(self.program,name).as_ref(),value);}
        gl.uniform_2_i32(gl.get_uniform_location(self.program,"aaq_extent").as_ref(),size[0]as i32,size[1]as i32);
        gl.uniform_1_u32(gl.get_uniform_location(self.program,"aaq_target").as_ref(),target);
        gl.memory_barrier(glow::SHADER_IMAGE_ACCESS_BARRIER_BIT|glow::TEXTURE_FETCH_BARRIER_BIT|glow::SHADER_STORAGE_BARRIER_BIT);
        gl.uniform_1_u32(gl.get_uniform_location(self.program,"aaq_mode").as_ref(),0);gl.dispatch_compute(1,1,1);gl.memory_barrier(glow::SHADER_STORAGE_BARRIER_BIT);
        gl.uniform_1_u32(gl.get_uniform_location(self.program,"aaq_mode").as_ref(),1);gl.dispatch_compute(total.div_ceil(256),1,1);
        gl.memory_barrier(glow::COMMAND_BARRIER_BIT|glow::SHADER_STORAGE_BARRIER_BIT);
        gl.active_texture(glow::TEXTURE6);gl.bind_texture(glow::TEXTURE_2D_ARRAY,previous_state);
        gl.active_texture(glow::TEXTURE9);gl.bind_texture(glow::TEXTURE_2D,previous_pages);gl.active_texture(active);gl.use_program(previous);
        Ok(())
    }
    /// Program is the original async fused Fragment + IMAGE_QUEUE_VERTEX.
    pub unsafe fn draw(&self,gl:&glow::Context,program:glow::Program,size:[u32;2])->Result<(),String>{
        checked_total(size)?;gl.use_program(Some(program));
        gl.uniform_1_i32(gl.get_uniform_location(program,"rs_use_point_queue").as_ref(),1);
        gl.uniform_2_i32(gl.get_uniform_location(program,"aq_extent").as_ref(),size[0]as i32,size[1]as i32);
        gl.bind_buffer_base(glow::SHADER_STORAGE_BUFFER,1,Some(self.buffer));
        let prior_indirect=gl.get_parameter_buffer(glow::DRAW_INDIRECT_BUFFER_BINDING);gl.bind_buffer(glow::DRAW_INDIRECT_BUFFER,Some(self.buffer));
        let enabled=gl.is_enabled(glow::PROGRAM_POINT_SIZE);gl.enable(glow::PROGRAM_POINT_SIZE);
        gl.draw_arrays_indirect_offset(glow::POINTS,0);
        if !enabled{gl.disable(glow::PROGRAM_POINT_SIZE);}gl.bind_buffer(glow::DRAW_INDIRECT_BUFFER,prior_indirect);
        Ok(())
    }
    pub unsafe fn destroy(self,gl:&glow::Context){gl.bind_buffer_base(glow::SHADER_STORAGE_BUFFER,1,None);gl.delete_buffer(self.buffer);gl.delete_program(self.program);self.capture.destroy(gl);}
}
struct StCapture{program:glow::Program,texture:glow::Texture,fbo:glow::Framebuffer}
impl StCapture{
    unsafe fn create(gl:&glow::Context)->Result<Self,String>{
        let program=super::geo_merged::link(gl,&[(glow::VERTEX_SHADER,super::gl_renderer::VERT),(glow::FRAGMENT_SHADER,super::raystate_shader::INTERPOLATED_ST_FRAGMENT)])?;
        let texture=match gl.create_texture(){Ok(t)=>t,Err(e)=>{gl.delete_program(program);return Err(e)}};
        let fbo=match gl.create_framebuffer(){Ok(f)=>f,Err(e)=>{gl.delete_texture(texture);gl.delete_program(program);return Err(e)}};
        let previous=gl.get_parameter_texture(glow::TEXTURE_BINDING_2D);gl.bind_texture(glow::TEXTURE_2D,Some(texture));
        gl.tex_parameter_i32(glow::TEXTURE_2D,glow::TEXTURE_MIN_FILTER,glow::NEAREST as i32);gl.tex_parameter_i32(glow::TEXTURE_2D,glow::TEXTURE_MAG_FILTER,glow::NEAREST as i32);
        gl.tex_image_2d(glow::TEXTURE_2D,0,glow::RG32F as i32,512,128,0,glow::RG,glow::FLOAT,glow::PixelUnpackData::Slice(None));gl.bind_texture(glow::TEXTURE_2D,previous);
        let e=gl.get_error();if e!=glow::NO_ERROR{gl.delete_framebuffer(fbo);gl.delete_texture(texture);gl.delete_program(program);return Err(format!("AsyncActiveQueue插值分配失败: GL {e:#x}"));}
        Ok(Self{program,texture,fbo})
    }
    unsafe fn draw(&self,gl:&glow::Context,original:glow::Program,size:[u32;2])->Result<(),String>{
        if size[0]==0||size[1]==0||size[0]>512||size[1]>128{return Err("AsyncActiveQueue插值仅支持512×128以内Geo工作块".into());}
        let mut aspect=[0f32;2];let location=gl.get_uniform_location(original,"aspect").ok_or("AsyncActiveQueue缺少原aspect")?;gl.get_uniform_f32(original,&location,&mut aspect);
        let program=gl.get_parameter_program(glow::CURRENT_PROGRAM);let fbo=gl.get_parameter_framebuffer(glow::DRAW_FRAMEBUFFER_BINDING);let mut viewport=[0i32;4];gl.get_parameter_i32_slice(glow::VIEWPORT,&mut viewport);
        let blend=gl.is_enabled(glow::BLEND);let scissor=gl.is_enabled(glow::SCISSOR_TEST);
        gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER,Some(self.fbo));gl.framebuffer_texture_2d(glow::DRAW_FRAMEBUFFER,glow::COLOR_ATTACHMENT0,glow::TEXTURE_2D,Some(self.texture),0);gl.draw_buffers(&[glow::COLOR_ATTACHMENT0]);
        if gl.check_framebuffer_status(glow::DRAW_FRAMEBUFFER)!=glow::FRAMEBUFFER_COMPLETE{gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER,fbo);return Err("AsyncActiveQueue插值FBO不完整".into());}
        gl.viewport(0,0,size[0]as i32,size[1]as i32);gl.disable(glow::BLEND);gl.disable(glow::SCISSOR_TEST);gl.use_program(Some(self.program));
        gl.uniform_2_f32(gl.get_uniform_location(self.program,"aspect").as_ref(),aspect[0],aspect[1]);
        // Original sample_uniform_record fixes Geo jitter to exactly zero.
        gl.uniform_2_f32(gl.get_uniform_location(self.program,"pixel_jitter").as_ref(),0.,0.);
        gl.draw_arrays(glow::TRIANGLES,0,3);gl.memory_barrier(glow::FRAMEBUFFER_BARRIER_BIT|glow::TEXTURE_FETCH_BARRIER_BIT);
        gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER,fbo);gl.viewport(viewport[0],viewport[1],viewport[2],viewport[3]);gl.use_program(program);if blend{gl.enable(glow::BLEND);}if scissor{gl.enable(glow::SCISSOR_TEST);}
        let e=gl.get_error();if e!=glow::NO_ERROR{return Err(format!("AsyncActiveQueue捕获插值失败: GL {e:#x}"));}Ok(())
    }
    unsafe fn destroy(self,gl:&glow::Context){gl.delete_framebuffer(self.fbo);gl.delete_texture(self.texture);gl.delete_program(self.program);}
}

