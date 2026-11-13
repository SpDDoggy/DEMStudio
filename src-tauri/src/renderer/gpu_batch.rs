//! Device-side sample cursor and per-draw activity accounting. Binding 2.
//! A candidate is one sample attempt: begin -> several integration draws ->
//! compact GPU reduction -> finish -> conditional commit draw -> advance. Calls
//! stay asynchronous until read(). The context must be current for destruction.
use glow::HasContext;

const WORDS: usize = 16;

/// Insert after shader extension directives. The fragment must call enabled()
/// before state reads and use resume() instead of pg_resume. No per-pixel global
/// atomic operations are added. Existing compact feedback supplies reduction.
pub(super) const FRAGMENT_HEADER: &str = r#"
layout(std430,binding=2) buffer RayBatchControl { uint rs_batch_words[]; };
uniform int rs_expected_sample;
uniform int rs_batch_mode;
bool rs_batch_enabled(){return rs_batch_mode==0||rs_batch_words[0]==uint(rs_expected_sample);}
bool rs_batch_resume(){return pg_resume!=0||(rs_batch_mode!=0&&rs_batch_words[2]!=0u);}
"#;

/// Commit shader must return/discard unless this predicate is true. Advance is
/// a separate dispatch after the commit draw, never called by every fragment.
pub(super) const COMMIT_HEADER: &str = r#"
layout(std430,binding=2) readonly buffer RayBatchControl { uint rs_batch_words[]; };
uniform int rs_expected_sample;
uniform int rs_batch_mode;
bool rs_batch_commit_ready(){return rs_batch_mode==0||(rs_batch_words[0]==uint(rs_expected_sample)&&rs_batch_words[3]!=0u);}
"#;

#[derive(Debug, Clone, Copy)]
pub(super) struct BatchFeedback {
    pub cursor: u32,
    pub done: u32,
    pub initialized: bool,
    pub commit_ready: bool,
    /// Completed paths summed over accepted candidate boundaries; formerly unused word 4.
    pub boundary_done: u32,
    pub boundary_waiting: u32,
    pub boundary_active: u32,
    pub boundary_capacity: u32,
    pub eligible_invocations: u32,
    pub zero_draws: u32,
    pub draws: u32,
    pub last_draw_active: u32,
    pub invalid: u32,
    pub yielded: u32,
    pub candidates: u32,
    pub eligible_capacity: u32,
}

pub(super) struct GpuBatch { program: glow::Program, buffer: glow::Buffer }
impl BatchFeedback {
    pub(super) fn decode(bytes:&[u8])->Self{
        let mut w=[0u32;WORDS];for (i,b) in bytes.chunks_exact(4).take(WORDS).enumerate(){w[i]=u32::from_ne_bytes(b.try_into().unwrap());}
        Self{cursor:w[0],done:w[1],initialized:w[2]!=0,commit_ready:w[3]!=0,
            boundary_done:w[4],boundary_waiting:w[13],boundary_active:w[14],boundary_capacity:w[15],eligible_invocations:w[5],zero_draws:w[6],draws:w[7],last_draw_active:w[8],invalid:w[9],yielded:w[10],candidates:w[11],eligible_capacity:w[12]}
    }
}
impl GpuBatch {
    pub unsafe fn copy_to(&self,gl:&glow::Context,destination:glow::Buffer,offset:i32){
        gl.memory_barrier(glow::SHADER_STORAGE_BARRIER_BIT|glow::BUFFER_UPDATE_BARRIER_BIT);
        gl.bind_buffer(glow::COPY_READ_BUFFER,Some(self.buffer));gl.bind_buffer(glow::COPY_WRITE_BUFFER,Some(destination));
        gl.copy_buffer_sub_data(glow::COPY_READ_BUFFER,glow::COPY_WRITE_BUFFER,0,offset,(WORDS*4) as i32);
        gl.bind_buffer(glow::COPY_READ_BUFFER,None);gl.bind_buffer(glow::COPY_WRITE_BUFFER,None);
        gl.memory_barrier(glow::BUFFER_UPDATE_BARRIER_BIT);
    }
    pub unsafe fn create(gl: &glow::Context) -> Result<Option<Self>, String> {
        let version=gl.version();
        if version.is_embedded||(version.major,version.minor)<(4,3){return Ok(None);}
        let shader=gl.create_shader(glow::COMPUTE_SHADER)?;
        gl.shader_source(shader,include_str!("gpu_batch.glsl"));gl.compile_shader(shader);
        if !gl.get_shader_compile_status(shader){let e=gl.get_shader_info_log(shader);gl.delete_shader(shader);return Err(e);}
        let program=match gl.create_program(){Ok(p)=>p,Err(e)=>{gl.delete_shader(shader);return Err(e)}};
        gl.attach_shader(program,shader);gl.link_program(program);gl.detach_shader(program,shader);gl.delete_shader(shader);
        if !gl.get_program_link_status(program){let e=gl.get_program_info_log(program);gl.delete_program(program);return Err(e);}
        let buffer=match gl.create_buffer(){Ok(b)=>b,Err(e)=>{gl.delete_program(program);return Err(e)}};
        gl.bind_buffer(glow::SHADER_STORAGE_BUFFER,Some(buffer));
        gl.buffer_data_size(glow::SHADER_STORAGE_BUFFER,(WORDS*4) as i32,glow::DYNAMIC_COPY);
        gl.bind_buffer(glow::SHADER_STORAGE_BUFFER,None);
        let error=gl.get_error();
        if error!=glow::NO_ERROR{gl.delete_buffer(buffer);gl.delete_program(program);return Err(format!("GPU 批次控制分配失败: GL {error:#x}"));}
        let batch=Self{program,buffer};batch.reset(gl,0);Ok(Some(batch))
    }
    /// Start a bounded CPU submission batch. Caller has consumed prior feedback;
    /// this clears telemetry, and must not be used to reset an unfinished sample.
    pub unsafe fn reset(&self,gl:&glow::Context,cursor:u32){
        self.reset_resume(gl,cursor,false);
    }
    pub unsafe fn reset_resume(&self,gl:&glow::Context,cursor:u32,resume:bool){
        let mut words=[0u32;WORDS];words[0]=cursor;
        words[2]=u32::from(resume);
        let bytes=std::slice::from_raw_parts(words.as_ptr().cast::<u8>(),WORDS*4);
        gl.bind_buffer(glow::SHADER_STORAGE_BUFFER,Some(self.buffer));
        gl.buffer_sub_data_u8_slice(glow::SHADER_STORAGE_BUFFER,0,bytes);
        gl.bind_buffer(glow::SHADER_STORAGE_BUFFER,None);self.bind(gl);
        gl.memory_barrier(glow::BUFFER_UPDATE_BARRIER_BIT|glow::SHADER_STORAGE_BARRIER_BIT);
    }
    pub unsafe fn bind(&self,gl:&glow::Context){gl.bind_buffer_base(glow::SHADER_STORAGE_BUFFER,2,Some(self.buffer));}
    unsafe fn phase(&self,gl:&glow::Context,mode:u32,expected:u32,total:u32){
        self.bind(gl);gl.use_program(Some(self.program));
        for (name,value) in [("gb_mode",mode),("gb_expected",expected),("gb_total",total)]{
            gl.uniform_1_u32(gl.get_uniform_location(self.program,name).as_ref(),value);
        }
        gl.memory_barrier(glow::SHADER_STORAGE_BARRIER_BIT);
        gl.dispatch_compute(1,1,1);
        gl.memory_barrier(glow::SHADER_STORAGE_BARRIER_BIT);
    }
    pub unsafe fn begin_candidate(&self,gl:&glow::Context,expected:u32){self.phase(gl,0,expected,0);}
    /// Existing compact_feedback must remain bound at binding 0 and must have
    /// completed dispatch(). Cursor mismatch never consumes its stale contents.
    pub unsafe fn finish_candidate(&self,gl:&glow::Context,expected:u32,total:u32){self.phase(gl,1,expected,total);}
    /// Optional activity telemetry: call only after a fresh active_queue dispatch
    /// bound at binding 1. No per-pixel atomics or additional CPU synchronization.
    /// eligible_capacity==0 means telemetry was not collected, not zero activity.
    pub unsafe fn record_queue_draw(&self,gl:&glow::Context,expected:u32,total:u32){self.phase(gl,3,expected,total);}
    /// All four phase POINTS Draws from one immutable classification snapshot.
    /// PhaseQueue must remain bound at binding 1. Counts include empty/gated
    /// commands; initialization triangles and the first unbatched sample are excluded.
    pub unsafe fn record_phase_queue(&self,gl:&glow::Context,expected:u32,total:u32){self.phase(gl,4,expected,total);}
    /// Call after the conditional commit. Incomplete samples retain state and
    /// their cursor; future sample candidates remain gated until it advances.
    pub unsafe fn advance(&self,gl:&glow::Context,expected:u32){self.phase(gl,2,expected,0);}
    /// Sole blocking operation; call once after the whole submission batch.
    pub unsafe fn read(&self,gl:&glow::Context)->Result<BatchFeedback,String>{
        gl.memory_barrier(glow::BUFFER_UPDATE_BARRIER_BIT|glow::SHADER_STORAGE_BARRIER_BIT);
        let mut bytes=[0u8;WORDS*4];
        gl.bind_buffer(glow::SHADER_STORAGE_BUFFER,Some(self.buffer));
        gl.get_buffer_sub_data(glow::SHADER_STORAGE_BUFFER,0,&mut bytes);
        gl.bind_buffer(glow::SHADER_STORAGE_BUFFER,None);
        let error=gl.get_error();if error!=glow::NO_ERROR{return Err(format!("GPU 批次控制读取失败: GL {error:#x}"));}
        let mut w=[0u32;WORDS];for (i,b) in bytes.chunks_exact(4).enumerate(){w[i]=u32::from_ne_bytes(b.try_into().unwrap());}
        Ok(BatchFeedback{cursor:w[0],done:w[1],initialized:w[2]!=0,commit_ready:w[3]!=0,
            boundary_done:w[4],boundary_waiting:w[13],boundary_active:w[14],boundary_capacity:w[15],eligible_invocations:w[5],zero_draws:w[6],draws:w[7],last_draw_active:w[8],invalid:w[9],yielded:w[10],candidates:w[11],eligible_capacity:w[12]})
    }
    pub unsafe fn destroy(self,gl:&glow::Context){gl.bind_buffer_base(glow::SHADER_STORAGE_BUFFER,2,None);gl.delete_buffer(self.buffer);gl.delete_program(self.program);}
}




