//! One immutable classification snapshot, four mutually exclusive indirect queues.
//! Binding 1; state/page textures 6/9. No sample commit or batch cursor changes.
//! Initialize samples with the original fullscreen triangle before classification.
use glow::HasContext;
use super::sample_wavefront::{Layout,Block};
pub(super) use super::phase_queue::Phase;
const COMMAND_WORDS:u32=16;
const DIAGNOSTIC_WORDS:u32=8;
const SEGMENT_WORDS:u32=256;
fn allocation_bytes(capacity:u32)->Option<u32>{
    if capacity==0||capacity>16*65536{return None;}
    COMMAND_WORDS.checked_add(capacity.checked_mul(4)?)?.checked_add(DIAGNOSTIC_WORDS)?.checked_add(SEGMENT_WORDS)?.checked_mul(4)
}
fn index_word(phase:Phase,capacity:u32)->u32{COMMAND_WORDS+phase as u32*capacity}
fn segment_offset(phase:Phase,capacity:u32,segment:u32)->Option<i32>{
    if segment>=16{return None;}Some(((COMMAND_WORDS+4*capacity+DIAGNOSTIC_WORDS+(phase as u32*16+segment)*4)*4)as i32)
}
pub(super) struct WavefrontQueue{program:glow::Program,buffer:glow::Buffer,capacity:u32,layout:Layout}
impl WavefrontQueue{
    pub unsafe fn create(gl:&glow::Context,layout:Layout)->Result<Option<Self>,String>{
        if Layout::new(layout.size,layout.lanes,layout.layers)?!=layout{return Err("wavefront layout inconsistent".into());}
        let capacity=layout.paths;
        let v=gl.version();if v.is_embedded||(v.major,v.minor)<(4,3){return Ok(None);}
        let Some(bytes)=allocation_bytes(capacity)else{return Ok(None);};
        if gl.get_parameter_i64(glow::MAX_SHADER_STORAGE_BLOCK_SIZE)<i64::from(bytes){return Ok(None);}
        let shader=gl.create_shader(glow::COMPUTE_SHADER)?;
        gl.shader_source(shader,include_str!("wavefront_queue.glsl"));gl.compile_shader(shader);
        if !gl.get_shader_compile_status(shader){let error=gl.get_shader_info_log(shader);gl.delete_shader(shader);return Err(error);}
        let program=match gl.create_program(){Ok(p)=>p,Err(e)=>{gl.delete_shader(shader);return Err(e)}};
        gl.attach_shader(program,shader);gl.link_program(program);gl.detach_shader(program,shader);gl.delete_shader(shader);
        if !gl.get_program_link_status(program){let e=gl.get_program_info_log(program);gl.delete_program(program);return Err(e);}
        let buffer=match gl.create_buffer(){Ok(b)=>b,Err(e)=>{gl.delete_program(program);return Err(e)}};
        gl.bind_buffer(glow::SHADER_STORAGE_BUFFER,Some(buffer));
        gl.buffer_data_size(glow::SHADER_STORAGE_BUFFER,bytes as i32,glow::DYNAMIC_COPY);
        gl.bind_buffer(glow::SHADER_STORAGE_BUFFER,None);
        let error=gl.get_error();if error!=glow::NO_ERROR{gl.delete_buffer(buffer);gl.delete_program(program);return Err(format!("阶段队列分配失败: GL {error:#x}"));}
        Ok(Some(Self{program,buffer,capacity,layout}))
    }
    pub fn memory_bytes(&self)->u64{u64::from(allocation_bytes(self.capacity).unwrap())}
    pub fn layout(&self)->Layout{self.layout}
    /// Classify only real lanes; queues remain immutable until the next snapshot.
    pub unsafe fn dispatch(&self,gl:&glow::Context,state:glow::Texture,pages:glow::Texture,source_width:u32,block:Block)->Result<(),String>{
        self.dispatch_mode(gl,state,pages,source_width,block,0)
    }
    /// Explicit two-kernel experiment: WORLD in queue0, all phases2..6 in queue1.
    /// Queue2/3 remain empty; allocation, validation and progress are unchanged.
    pub unsafe fn dispatch_pair(&self,gl:&glow::Context,state:glow::Texture,pages:glow::Texture,source_width:u32,block:Block)->Result<(),String>{
        self.dispatch_mode(gl,state,pages,source_width,block,1)
    }
    pub unsafe fn dispatch_fused(&self,gl:&glow::Context,state:glow::Texture,pages:glow::Texture,source_width:u32,block:Block)->Result<(),String>{
        self.dispatch_mode(gl,state,pages,source_width,block,2)
    }
    unsafe fn dispatch_mode(&self,gl:&glow::Context,state:glow::Texture,pages:glow::Texture,source_width:u32,block:Block,mode:u32)->Result<(),String>{
        if block.count==0||block.count>self.layout.lanes||block.base==0||block.base.checked_add(block.count).is_none_or(|v|v>256)||source_width==0{return Err("wavefront block invalid".into());}
        gl.memory_barrier(glow::SHADER_IMAGE_ACCESS_BARRIER_BIT|glow::TEXTURE_FETCH_BARRIER_BIT);
        gl.use_program(Some(self.program));gl.bind_buffer_base(glow::SHADER_STORAGE_BUFFER,1,Some(self.buffer));
        let set=|name:&str,value:u32|gl.uniform_1_u32(gl.get_uniform_location(self.program,name).as_ref(),value);
        gl.uniform_1_i32(gl.get_uniform_location(self.program,"pq_state").as_ref(),6);
        gl.uniform_1_i32(gl.get_uniform_location(self.program,"pq_pages").as_ref(),9);
        set("pq_pair",mode);
        set("pq_capacity",self.capacity);set("pq_across",source_width.div_ceil(512));set("sw_count",block.count);
        gl.uniform_2_i32(gl.get_uniform_location(self.program,"pq_extent").as_ref(),self.layout.size[0]as i32,self.layout.size[1]as i32);
        set("pq_mode",0);gl.dispatch_compute(1,1,1);gl.memory_barrier(glow::SHADER_STORAGE_BARRIER_BIT);
        gl.active_texture(glow::TEXTURE6);gl.bind_texture(glow::TEXTURE_2D_ARRAY,Some(state));gl.uniform_1_i32(gl.get_uniform_location(self.program,"pq_state").as_ref(),6);
        gl.active_texture(glow::TEXTURE9);gl.bind_texture(glow::TEXTURE_2D,Some(pages));gl.uniform_1_i32(gl.get_uniform_location(self.program,"pq_pages").as_ref(),9);
        set("pq_mode",1);gl.dispatch_compute((self.layout.pixels*block.count).div_ceil(256),1,1);gl.memory_barrier(glow::SHADER_STORAGE_BARRIER_BIT);
        set("pq_mode",2);gl.dispatch_compute(1,1,1);gl.memory_barrier(glow::COMMAND_BARRIER_BIT|glow::SHADER_STORAGE_BARRIER_BIT);
        let error=gl.get_error();if error!=glow::NO_ERROR{return Err(format!("wavefront classify GL{error:#x}"));}Ok(())
    }
    /// Fixed 16 segments; empty commands have count zero, never a CPU count read.
    /// Caller owns original physical viewport/FBO, state images and phase uniforms.
    pub unsafe fn draw_segment(&self,gl:&glow::Context,program:glow::Program,phase:Phase,segment:u32)->Result<(),String>{
        let offset=segment_offset(phase,self.capacity,segment).ok_or("wavefront segment invalid")?;
        gl.use_program(Some(program));
        for name in ["pq_extent","sw_extent"]{gl.uniform_2_i32(gl.get_uniform_location(program,name).as_ref(),self.layout.size[0]as i32,self.layout.size[1]as i32);}
        gl.uniform_1_u32(gl.get_uniform_location(program,"pq_index_base").as_ref(),index_word(phase,self.capacity));
        gl.bind_buffer_base(glow::SHADER_STORAGE_BUFFER,1,Some(self.buffer));gl.bind_buffer(glow::DRAW_INDIRECT_BUFFER,Some(self.buffer));
        let enabled=gl.is_enabled(glow::PROGRAM_POINT_SIZE);gl.enable(glow::PROGRAM_POINT_SIZE);gl.draw_arrays_indirect_offset(glow::POINTS,offset);
        if !enabled{gl.disable(glow::PROGRAM_POINT_SIZE);}gl.bind_buffer(glow::DRAW_INDIRECT_BUFFER,None);
        let error=gl.get_error();if error!=glow::NO_ERROR{return Err(format!("wavefront draw GL{error:#x}"));}Ok(())
    }
    /// Single 32-byte feedback: invalid state/page, overflow, waiting, DONE,
    /// valid block paths, reserved, reserved. No per-path CPU read.
    pub unsafe fn read_progress(&self,gl:&glow::Context)->[u32;8]{
        gl.memory_barrier(glow::SHADER_STORAGE_BARRIER_BIT|glow::BUFFER_UPDATE_BARRIER_BIT);
        let prior=gl.get_parameter_buffer(glow::SHADER_STORAGE_BUFFER_BINDING);gl.bind_buffer(glow::SHADER_STORAGE_BUFFER,Some(self.buffer));
        let mut bytes=[0u8;32];gl.get_buffer_sub_data(glow::SHADER_STORAGE_BUFFER,((COMMAND_WORDS+4*self.capacity)*4)as i32,&mut bytes);gl.bind_buffer(glow::SHADER_STORAGE_BUFFER,prior);
        std::array::from_fn(|i|u32::from_ne_bytes(bytes[i*4..i*4+4].try_into().unwrap()))
    }
    /// Eight merged progress/diagnostic words.
    /// Append to an existing feedback buffer before its one normal readback.
    pub unsafe fn copy_diagnostics_to(&self,gl:&glow::Context,destination:glow::Buffer,offset:i32){
        gl.memory_barrier(glow::SHADER_STORAGE_BARRIER_BIT|glow::BUFFER_UPDATE_BARRIER_BIT);
        gl.bind_buffer(glow::COPY_READ_BUFFER,Some(self.buffer));gl.bind_buffer(glow::COPY_WRITE_BUFFER,Some(destination));
        gl.copy_buffer_sub_data(glow::COPY_READ_BUFFER,glow::COPY_WRITE_BUFFER,((COMMAND_WORDS+4*self.capacity)*4)as i32,offset,32);
        gl.bind_buffer(glow::COPY_READ_BUFFER,None);gl.bind_buffer(glow::COPY_WRITE_BUFFER,None);
        gl.memory_barrier(glow::BUFFER_UPDATE_BARRIER_BIT);
    }
    pub unsafe fn destroy(self,gl:&glow::Context){gl.bind_buffer_base(glow::SHADER_STORAGE_BUFFER,1,None);gl.delete_buffer(self.buffer);gl.delete_program(self.program);}
}









