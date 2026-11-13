//! One immutable classification snapshot, four mutually exclusive indirect queues.
//! Binding 1; state/page textures 6/9; optional batch cursor at binding 2.
//! Initialize samples with the original fullscreen triangle before classification.
use glow::HasContext;
const COMMAND_WORDS:u32=16;
const DIAGNOSTIC_WORDS:u32=4;
const COMPUTE_COMMAND_WORDS:u32=12;
const MAX_CAPACITY:u32=65536;
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
#[repr(u32)]
pub(super) enum Phase { World=0,Surface=1,Shadow=2,Scatter=3 }
impl Phase {
    pub const ALL:[Self;4]=[Self::World,Self::Surface,Self::Shadow,Self::Scatter];
    /// Byte offset into DRAW_INDIRECT_BUFFER, never a uint offset.
    pub const fn command_offset(self)->i32{self as i32*16}
    pub const fn index_word(self,capacity:u32)->u32{COMMAND_WORDS+self as u32*capacity}
    pub const fn compute_command_offset(self,capacity:u32)->i32{((COMMAND_WORDS+4*capacity+DIAGNOSTIC_WORDS+self as u32*3)*4)as i32}
}
fn allocation_bytes(capacity:u32)->Option<u32>{
    if capacity==0||capacity>MAX_CAPACITY{return None;}
    Some((COMMAND_WORDS+4*capacity+DIAGNOSTIC_WORDS+COMPUTE_COMMAND_WORDS)*4)
}
/// Queue-only vertex input. A resumed phase never reconstructs initial camera st.
/// Fragment programs must use explicit texture LOD for POINTS material sampling.
pub(super) const VERTEX_SOURCE:&str=r#"#version 430 compatibility
layout(std430,binding=1) readonly buffer PhaseQueue { uint pq_words[]; };
uniform ivec2 pq_extent;
uniform uint pq_index_base;
out vec2 st;
void main(){
    uint pixel=pq_words[pq_index_base+uint(gl_VertexID)];
    vec2 center=vec2(pixel%uint(pq_extent.x),pixel/uint(pq_extent.x))+.5;
    gl_Position=vec4(center/vec2(pq_extent)*2.-1.,0.,1.);gl_PointSize=1.;st=vec2(0.);
}"#;
pub(super) struct PhaseQueue{program:glow::Program,buffer:glow::Buffer,capacity:u32}
impl PhaseQueue{
    pub unsafe fn create(gl:&glow::Context,capacity:u32)->Result<Option<Self>,String>{
        let v=gl.version();if v.is_embedded||(v.major,v.minor)<(4,3){return Ok(None);}
        let Some(bytes)=allocation_bytes(capacity)else{return Ok(None);};
        if gl.get_parameter_i64(glow::MAX_SHADER_STORAGE_BLOCK_SIZE)<i64::from(bytes){return Ok(None);}
        let shader=gl.create_shader(glow::COMPUTE_SHADER)?;
        gl.shader_source(shader,include_str!("phase_queue.glsl"));gl.compile_shader(shader);
        if !gl.get_shader_compile_status(shader){let error=gl.get_shader_info_log(shader);gl.delete_shader(shader);return Err(error);}
        let program=match gl.create_program(){Ok(p)=>p,Err(e)=>{gl.delete_shader(shader);return Err(e)}};
        gl.attach_shader(program,shader);gl.link_program(program);gl.detach_shader(program,shader);gl.delete_shader(shader);
        if !gl.get_program_link_status(program){let e=gl.get_program_info_log(program);gl.delete_program(program);return Err(e);}
        let buffer=match gl.create_buffer(){Ok(b)=>b,Err(e)=>{gl.delete_program(program);return Err(e)}};
        gl.bind_buffer(glow::SHADER_STORAGE_BUFFER,Some(buffer));
        gl.buffer_data_size(glow::SHADER_STORAGE_BUFFER,bytes as i32,glow::DYNAMIC_COPY);
        gl.bind_buffer(glow::SHADER_STORAGE_BUFFER,None);
        let error=gl.get_error();if error!=glow::NO_ERROR{gl.delete_buffer(buffer);gl.delete_program(program);return Err(format!("阶段队列分配失败: GL {error:#x}"));}
        Ok(Some(Self{program,buffer,capacity}))
    }
    pub fn memory_bytes(&self)->u64{u64::from(allocation_bytes(self.capacity).unwrap())}
    fn check_size(&self,size:[u32;2])->Result<(),String>{
        if size.contains(&0)||u64::from(size[0])*u64::from(size[1])>u64::from(self.capacity){return Err("阶段队列工作块容量超限".into());}Ok(())
    }
    /// Snapshot only: no queue append is allowed during the four subsequent Draws.
    /// expected=Some requires GpuBatch at binding 2; mismatch emits empty queues.
    pub unsafe fn dispatch(&self,gl:&glow::Context,state:glow::Texture,pages:glow::Texture,
        size:[u32;2],source_width:u32,expected:Option<u32>)->Result<(),String>{
        self.check_size(size)?;if source_width==0{return Err("阶段队列源宽度无效".into());}
        gl.memory_barrier(glow::SHADER_IMAGE_ACCESS_BARRIER_BIT|glow::TEXTURE_FETCH_BARRIER_BIT);
        gl.use_program(Some(self.program));gl.bind_buffer_base(glow::SHADER_STORAGE_BUFFER,1,Some(self.buffer));
        let set=|name:&str,value:u32|gl.uniform_1_u32(gl.get_uniform_location(self.program,name).as_ref(),value);
        set("pq_capacity",self.capacity);set("pq_across",source_width.div_ceil(512));
        set("pq_batch_mode",u32::from(expected.is_some()));set("pq_expected",expected.unwrap_or(0));
        set("pq_mode",0);gl.dispatch_compute(1,1,1);gl.memory_barrier(glow::SHADER_STORAGE_BARRIER_BIT);
        gl.active_texture(glow::TEXTURE6);gl.bind_texture(glow::TEXTURE_2D_ARRAY,Some(state));
        gl.uniform_1_i32(gl.get_uniform_location(self.program,"pq_state").as_ref(),6);
        gl.active_texture(glow::TEXTURE9);gl.bind_texture(glow::TEXTURE_2D,Some(pages));
        gl.uniform_1_i32(gl.get_uniform_location(self.program,"pq_pages").as_ref(),9);
        gl.uniform_2_i32(gl.get_uniform_location(self.program,"pq_extent").as_ref(),size[0] as i32,size[1] as i32);
        set("pq_mode",1);gl.dispatch_compute((size[0]*size[1]).div_ceil(256),1,1);
        gl.memory_barrier(glow::COMMAND_BARRIER_BIT|glow::SHADER_STORAGE_BARRIER_BIT);
        let error=gl.get_error();if error!=glow::NO_ERROR{return Err(format!("阶段队列分类失败: GL {error:#x}"));}Ok(())
    }
    /// Caller sets viewport/FBO/state images and phase Fragment uniforms.
    pub unsafe fn draw(&self,gl:&glow::Context,program:glow::Program,size:[u32;2],phase:Phase)->Result<(),String>{
        self.check_size(size)?;gl.use_program(Some(program));
        gl.uniform_2_i32(gl.get_uniform_location(program,"pq_extent").as_ref(),size[0] as i32,size[1] as i32);
        gl.uniform_1_u32(gl.get_uniform_location(program,"pq_index_base").as_ref(),phase.index_word(self.capacity));
        gl.bind_buffer_base(glow::SHADER_STORAGE_BUFFER,1,Some(self.buffer));
        gl.bind_buffer(glow::DRAW_INDIRECT_BUFFER,Some(self.buffer));
        let enabled=gl.is_enabled(glow::PROGRAM_POINT_SIZE);gl.enable(glow::PROGRAM_POINT_SIZE);
        gl.draw_arrays_indirect_offset(glow::POINTS,phase.command_offset());
        if !enabled{gl.disable(glow::PROGRAM_POINT_SIZE);}gl.bind_buffer(glow::DRAW_INDIRECT_BUFFER,None);
        let error=gl.get_error();if error!=glow::NO_ERROR{return Err(format!("阶段队列绘制失败: GL {error:#x}"));}Ok(())
    }
    /// Dispatch a 64-invocation phase kernel from the same classification snapshot.
    /// Caller owns image/storage barriers after execution, matching the Draw path.
    pub unsafe fn dispatch_phase(&self,gl:&glow::Context,program:glow::Program,size:[u32;2],phase:Phase)->Result<(),String>{
        self.check_size(size)?;gl.use_program(Some(program));
        gl.uniform_2_i32(gl.get_uniform_location(program,"pq_extent").as_ref(),size[0] as i32,size[1] as i32);
        gl.uniform_1_u32(gl.get_uniform_location(program,"pq_index_base").as_ref(),phase.index_word(self.capacity));
        gl.uniform_1_u32(gl.get_uniform_location(program,"pq_phase").as_ref(),phase as u32);
        gl.bind_buffer_base(glow::SHADER_STORAGE_BUFFER,1,Some(self.buffer));
        gl.bind_buffer(glow::DISPATCH_INDIRECT_BUFFER,Some(self.buffer));
        gl.dispatch_compute_indirect(phase.compute_command_offset(self.capacity));
        gl.bind_buffer(glow::DISPATCH_INDIRECT_BUFFER,None);
        let error=gl.get_error();if error!=glow::NO_ERROR{return Err(format!("阶段队列计算失败: GL {error:#x}"));}Ok(())
    }
    /// Four uints: invalid state, invalid page, overflow, waiting-unready.
    /// Append to an existing feedback buffer before its one normal readback.
    pub unsafe fn copy_diagnostics_to(&self,gl:&glow::Context,destination:glow::Buffer,offset:i32){
        gl.memory_barrier(glow::SHADER_STORAGE_BARRIER_BIT|glow::BUFFER_UPDATE_BARRIER_BIT);
        gl.bind_buffer(glow::COPY_READ_BUFFER,Some(self.buffer));gl.bind_buffer(glow::COPY_WRITE_BUFFER,Some(destination));
        gl.copy_buffer_sub_data(glow::COPY_READ_BUFFER,glow::COPY_WRITE_BUFFER,((COMMAND_WORDS+4*self.capacity)*4)as i32,offset,16);
        gl.bind_buffer(glow::COPY_READ_BUFFER,None);gl.bind_buffer(glow::COPY_WRITE_BUFFER,None);
        gl.memory_barrier(glow::BUFFER_UPDATE_BARRIER_BIT);
    }
    pub unsafe fn destroy(self,gl:&glow::Context){gl.bind_buffer_base(glow::SHADER_STORAGE_BUFFER,1,None);gl.delete_buffer(self.buffer);gl.delete_program(self.program);}
}




