//! Device-only eligible-pixel compaction and indirect point dispatch.
//! SSBO binding 1, texture units 6/9; no CPU queue readback in production.
use glow::HasContext;
const MAX_BYTES: u64 = 64 * 1024 * 1024;
pub(super) const VERTEX_SOURCE: &str = r#"#version 430 compatibility
layout(std430,binding=1) readonly buffer ActiveQueue { uint words[]; };
uniform ivec2 aq_extent;
uniform vec2 aspect,pixel_jitter;
out vec2 st;
void main(){
    uint pixel=words[4+gl_VertexID];
    vec2 center=vec2(pixel%uint(aq_extent.x),pixel/uint(aq_extent.x))+.5;
    vec2 ndc=center/vec2(aq_extent)*2.-1.;
    gl_Position=vec4(ndc,0.,1.);gl_PointSize=1.;
    st=ndc*aspect+pixel_jitter;
}"#;

pub(super) struct ActiveQueue {
    program: glow::Program,
    buffer: glow::Buffer,
    capacity: u32,
}


impl ActiveQueue {
    pub unsafe fn create(gl:&glow::Context, capacity:u32)->Result<Option<Self>,String>{
        let v=gl.version();
        if v.is_embedded||(v.major,v.minor)<(4,3){return Ok(None);}
        let bytes=16+u64::from(capacity)*4;
        if capacity==0||bytes>MAX_BYTES||gl.get_parameter_i64(glow::MAX_SHADER_STORAGE_BLOCK_SIZE)<bytes as i64{return Ok(None);}
        let shader=gl.create_shader(glow::COMPUTE_SHADER)?;
        gl.shader_source(shader,include_str!("active_queue.glsl"));gl.compile_shader(shader);
        if !gl.get_shader_compile_status(shader){let error=gl.get_shader_info_log(shader);gl.delete_shader(shader);return Err(error);}
        let program=match gl.create_program(){Ok(p)=>p,Err(e)=>{gl.delete_shader(shader);return Err(e)}};
        gl.attach_shader(program,shader);gl.link_program(program);gl.detach_shader(program,shader);gl.delete_shader(shader);
        if !gl.get_program_link_status(program){let error=gl.get_program_info_log(program);gl.delete_program(program);return Err(error);}
        let buffer=match gl.create_buffer(){Ok(b)=>b,Err(e)=>{gl.delete_program(program);return Err(e)}};
        gl.bind_buffer(glow::SHADER_STORAGE_BUFFER,Some(buffer));
        gl.buffer_data_size(glow::SHADER_STORAGE_BUFFER,bytes as i32,glow::DYNAMIC_COPY);
        let error=gl.get_error();gl.bind_buffer(glow::SHADER_STORAGE_BUFFER,None);
        if error!=glow::NO_ERROR{gl.delete_buffer(buffer);gl.delete_program(program);return Err(format!("活动队列分配失败: GL {error:#x}"));}
        Ok(Some(Self{program,buffer,capacity}))
    }
    pub fn memory_bytes(&self)->u64{16+u64::from(self.capacity)*4}
    /// query_memo=false for full RayState. Legacy query memo passes true and
    /// its pg_reset_query value so a newly started query wakes completed pixels.
    pub unsafe fn dispatch(&self,gl:&glow::Context,state:glow::Texture,pages:glow::Texture,
        size:[u32;2],source_width:u32,resume:bool,query_memo:bool,reset_query:bool)->Result<(),String>{
        self.dispatch_inner(gl,state,pages,size,source_width,resume,query_memo,reset_query,None)
    }
    /// Binding 2 must hold GpuBatch control. Gated candidates emit an empty
    /// command; resumed partial samples use device initialized, not CPU guesses.
    pub unsafe fn dispatch_batch(&self,gl:&glow::Context,state:glow::Texture,pages:glow::Texture,
        size:[u32;2],source_width:u32,resume:bool,query_memo:bool,reset_query:bool,expected:u32)->Result<(),String>{
        self.dispatch_inner(gl,state,pages,size,source_width,resume,query_memo,reset_query,Some(expected))
    }
    unsafe fn dispatch_inner(&self,gl:&glow::Context,state:glow::Texture,pages:glow::Texture,
        size:[u32;2],source_width:u32,resume:bool,query_memo:bool,reset_query:bool,expected:Option<u32>)->Result<(),String>{
        self.check_size(size)?;
        if source_width==0||source_width>i32::MAX as u32-511{return Err("活动队列源宽度无效".into());}
        gl.use_program(Some(self.program));
        gl.bind_buffer_base(glow::SHADER_STORAGE_BUFFER,1,Some(self.buffer));
        let set=|name:&str,value:i32|gl.uniform_1_i32(gl.get_uniform_location(self.program,name).as_ref(),value);
        set("aq_batch_mode",i32::from(expected.is_some()));set("aq_expected_sample",expected.unwrap_or(0) as i32);
        set("aq_mode",0);gl.dispatch_compute(1,1,1);gl.memory_barrier(glow::SHADER_STORAGE_BARRIER_BIT);
        gl.active_texture(glow::TEXTURE6);gl.bind_texture(glow::TEXTURE_2D_ARRAY,Some(state));set("aq_state",6);
        gl.active_texture(glow::TEXTURE9);gl.bind_texture(glow::TEXTURE_2D,Some(pages));set("aq_pages",9);
        gl.uniform_2_i32(gl.get_uniform_location(self.program,"aq_extent").as_ref(),size[0] as i32,size[1] as i32);
        set("aq_across",source_width.div_ceil(512) as i32);set("aq_resume",i32::from(resume));
        set("aq_query_memo",i32::from(query_memo));set("aq_reset_query",i32::from(reset_query));set("aq_mode",1);
        let groups=(size[0]*size[1]).div_ceil(256);let gx=groups.min(65535).max(1);
        gl.dispatch_compute(gx,groups.div_ceil(gx).max(1),1);
        gl.memory_barrier(glow::COMMAND_BARRIER_BIT|glow::SHADER_STORAGE_BARRIER_BIT);
        let error=gl.get_error();
        if error!=glow::NO_ERROR{return Err(format!("活动队列构建失败: GL {error:#x}"));}Ok(())
    }
    fn check_size(&self,size:[u32;2])->Result<(),String>{
        let total=u64::from(size[0])*u64::from(size[1]);
        if size.contains(&0)||size.iter().any(|&v|v>i32::MAX as u32)||total>u64::from(self.capacity){return Err("活动队列工作块容量超限".into());}Ok(())
    }
    /// Copy ALL layers before sparse drawing; skipped pixels retain exact bits.
    /// Input/output must be distinct compatible allocated RGBA32F array images.
    pub unsafe fn copy_state(&self,gl:&glow::Context,input:glow::Texture,output:glow::Texture,
        size:[u32;2],layers:u32)->Result<(),String>{
        self.check_size(size)?;
        if input==output||layers==0||layers>i32::MAX as u32{return Err("活动队列状态复制参数无效".into());}
        gl.copy_image_sub_data(input,glow::TEXTURE_2D_ARRAY,0,0,0,0,
            output,glow::TEXTURE_2D_ARRAY,0,0,0,0,size[0] as i32,size[1] as i32,layers as i32);
        let error=gl.get_error();if error!=glow::NO_ERROR{return Err(format!("活动队列状态复制失败: GL {error:#x}"));}Ok(())
    }
    /// Draw program must have VERTEX_SOURCE linked and normal integration
    /// uniforms set. Caller sets viewport/FBO and all fragment texture bindings.
    pub unsafe fn draw(&self,gl:&glow::Context,program:glow::Program,size:[u32;2])->Result<(),String>{
        self.check_size(size)?;gl.use_program(Some(program));
        gl.uniform_2_i32(gl.get_uniform_location(program,"aq_extent").as_ref(),size[0] as i32,size[1] as i32);
        gl.bind_buffer_base(glow::SHADER_STORAGE_BUFFER,1,Some(self.buffer));
        gl.bind_buffer(glow::DRAW_INDIRECT_BUFFER,Some(self.buffer));
        let enabled=gl.is_enabled(glow::PROGRAM_POINT_SIZE);gl.enable(glow::PROGRAM_POINT_SIZE);
        gl.draw_arrays_indirect_offset(glow::POINTS,0);
        if !enabled{gl.disable(glow::PROGRAM_POINT_SIZE);}
        gl.bind_buffer(glow::DRAW_INDIRECT_BUFFER,None);
        let error=gl.get_error();if error!=glow::NO_ERROR{return Err(format!("活动队列绘制失败: GL {error:#x}"));}Ok(())
    }
    pub unsafe fn destroy(self,gl:&glow::Context){
        gl.bind_buffer_base(glow::SHADER_STORAGE_BUFFER,1,None);gl.delete_buffer(self.buffer);gl.delete_program(self.program);
    }
}
