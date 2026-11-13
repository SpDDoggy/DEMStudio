//! Optional GL 4.3 status reduction. The integration state stays on the GPU.
//! Uses texture unit 6 and SSBO binding 0; caller must bind its draw program
//! again after dispatch. No framebuffer, viewport, or draw-buffer state changes.
use glow::HasContext;

const HEADER: usize = 8;
const MAX_BYTES: usize = 16 * 1024 * 1024;
const SHADER: &str = include_str!("compact_feedback.glsl");

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Feedback {
    pub done: u32,
    /// Includes paths that are already completely done (status 4).
    pub current_done: u32,
    pub yielded: u32,
    pub invalid: u32,
    pub total: u32,
    /// Sample-boundary states from the existing reduction; not Draw activity.
    pub waiting: u32,
    pub active: u32,
    /// Descending demand count, then ascending page ID: inspect_status order.
    pub requests: Vec<(i32, u32)>,
}
impl Feedback {
    pub fn all_paths_done(&self) -> bool { self.done == self.total }
    pub fn query_complete(&self) -> bool { self.current_done == self.total }
}

pub(super) struct CompactFeedback {
    program: glow::Program,
    buffer: glow::Buffer,
    page_count: u32,
    bytes: Vec<u8>,
    pending: bool,
}

impl CompactFeedback {
    /// None means unsupported/capacity exceeded: use the GL 3.3 CPU inspector.
    /// Allocation/compile errors clean up all resources before returning Err.
    pub unsafe fn create(gl: &glow::Context, page_count: u32) -> Result<Option<Self>, String> {
        let v = gl.version();
        if v.is_embedded || (v.major, v.minor) < (4, 3) { return Ok(None); }
        let size = (page_count as usize).checked_add(HEADER).and_then(|n| n.checked_mul(4));
        let Some(size) = size.filter(|&n| page_count > 0 && n <= MAX_BYTES) else { return Ok(None); };
        if gl.get_parameter_i64(glow::MAX_SHADER_STORAGE_BLOCK_SIZE) < size as i64 {
            return Ok(None);
        }
        let shader = gl.create_shader(glow::COMPUTE_SHADER)?;
        gl.shader_source(shader, SHADER);
        gl.compile_shader(shader);
        if !gl.get_shader_compile_status(shader) {
            let error = gl.get_shader_info_log(shader);
            gl.delete_shader(shader);
            return Err(format!("分页紧凑反馈编译失败: {error}"));
        }
        let program = match gl.create_program() {
            Ok(p) => p,
            Err(e) => { gl.delete_shader(shader); return Err(e); }
        };
        gl.attach_shader(program, shader);
        gl.link_program(program);
        gl.detach_shader(program, shader);
        gl.delete_shader(shader);
        if !gl.get_program_link_status(program) {
            let error = gl.get_program_info_log(program);
            gl.delete_program(program);
            return Err(format!("分页紧凑反馈链接失败: {error}"));
        }
        let buffer = match gl.create_buffer() {
            Ok(b) => b,
            Err(e) => { gl.delete_program(program); return Err(e); }
        };
        gl.bind_buffer(glow::SHADER_STORAGE_BUFFER, Some(buffer));
        gl.buffer_data_size(glow::SHADER_STORAGE_BUFFER, (size+64) as i32, glow::DYNAMIC_READ);
        let error = gl.get_error();
        gl.bind_buffer(glow::SHADER_STORAGE_BUFFER, None);
        if error != glow::NO_ERROR {
            gl.delete_buffer(buffer);
            gl.delete_program(program);
            return Err(format!("分页紧凑反馈分配失败: GL {error:#x}"));
        }
        Ok(Some(Self { program, buffer, page_count, bytes: vec![0; size], pending: false }))
    }

    pub fn readback_bytes(&self) -> usize { self.bytes.len() }

    /// Device consumer has finished with this reduction; the next dispatch may
    /// overwrite it without a CPU read. Ordered SSBO barriers preserve access.
    pub fn consumed_on_gpu(&mut self) { self.pending=false; }

    /// Append the device cursor to the same readback. No separate controller
    /// get-buffer call or second CPU/GPU synchronization is required.
    pub unsafe fn read_batch(&mut self,gl:&glow::Context,batch:&crate::renderer::gpu_batch::GpuBatch)
        ->Result<(Feedback,crate::renderer::gpu_batch::BatchFeedback),String>{
        if !self.pending{return Err("分页紧凑反馈尚未执行".into());}
        batch.copy_to(gl,self.buffer,self.bytes.len() as i32);
        let mut bytes=vec![0u8;self.bytes.len()+64];
        gl.bind_buffer(glow::SHADER_STORAGE_BUFFER,Some(self.buffer));
        gl.get_buffer_sub_data(glow::SHADER_STORAGE_BUFFER,0,&mut bytes);
        gl.bind_buffer(glow::SHADER_STORAGE_BUFFER,None);self.pending=false;
        let error=gl.get_error();if error!=glow::NO_ERROR{return Err(format!("GPU批次反馈读取失败: GL {error:#x}"));}
        let words:Vec<u32>=bytes[..self.bytes.len()].chunks_exact(4).map(|b|u32::from_ne_bytes(b.try_into().unwrap())).collect();
        Ok((decode(&words),crate::renderer::gpu_batch::BatchFeedback::decode(&bytes[self.bytes.len()..])))
    }

    /// Reuse the existing bounded readback for wavefront queue diagnostics.
    pub unsafe fn read_wavefront(&mut self,gl:&glow::Context,queue:&crate::renderer::wavefront_queue::WavefrontQueue)
        ->Result<(Feedback,[u32;8]),String>{
        if !self.pending{return Err("wavefront compact feedback not dispatched".into());}
        queue.copy_diagnostics_to(gl,self.buffer,self.bytes.len()as i32);
        let mut bytes=vec![0u8;self.bytes.len()+32];gl.bind_buffer(glow::SHADER_STORAGE_BUFFER,Some(self.buffer));gl.get_buffer_sub_data(glow::SHADER_STORAGE_BUFFER,0,&mut bytes);gl.bind_buffer(glow::SHADER_STORAGE_BUFFER,None);self.pending=false;
        let error=gl.get_error();if error!=glow::NO_ERROR{return Err(format!("wavefront feedback GL{error:#x}"));}
        let words:Vec<u32>=bytes[..self.bytes.len()].chunks_exact(4).map(|b|u32::from_ne_bytes(b.try_into().unwrap())).collect();
        let mut diagnostic=[0u32;8];for(i,b)in bytes[self.bytes.len()..].chunks_exact(4).enumerate(){diagnostic[i]=u32::from_ne_bytes(b.try_into().unwrap());}Ok((decode(&words),diagnostic))
    }

    /// Clear counters and scan status layer 5. Dispatch itself does not wait.
    pub unsafe fn dispatch(&mut self, gl: &glow::Context, state: glow::Texture,
        size: [u32; 2]) -> Result<(), String> {
        let total = u64::from(size[0]) * u64::from(size[1]);
        if self.pending { return Err("分页紧凑反馈尚未读取".into()); }
        if total > u32::MAX as u64 || size.iter().any(|&v| v > i32::MAX as u32) {
            return Err("分页反馈工作块尺寸超限".into());
        }
        gl.use_program(Some(self.program));
        gl.bind_buffer_base(glow::SHADER_STORAGE_BUFFER, 0, Some(self.buffer));
        gl.uniform_1_u32(gl.get_uniform_location(self.program, "page_count").as_ref(), self.page_count);
        gl.uniform_1_u32(gl.get_uniform_location(self.program, "mode").as_ref(), 0);
        gl.dispatch_compute((self.page_count + HEADER as u32).div_ceil(256), 1, 1);
        gl.memory_barrier(glow::SHADER_STORAGE_BARRIER_BIT);
        gl.active_texture(glow::TEXTURE6);
        gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(state));
        gl.uniform_1_i32(gl.get_uniform_location(self.program, "state_image").as_ref(), 6);
        gl.uniform_2_i32(gl.get_uniform_location(self.program, "extent").as_ref(), size[0] as i32, size[1] as i32);
        gl.uniform_1_u32(gl.get_uniform_location(self.program, "mode").as_ref(), 1);
        // Linearize work to avoid requiring more than 65535 groups on either axis.
        let groups = (total as u32).div_ceil(256);
        let gx = groups.min(65535).max(1);
        gl.dispatch_compute(gx, groups.div_ceil(gx).max(1), 1);
        gl.memory_barrier(glow::BUFFER_UPDATE_BARRIER_BIT | glow::SHADER_STORAGE_BARRIER_BIT);
        let error = gl.get_error();
        if error != glow::NO_ERROR { return Err(format!("分页紧凑反馈执行失败: GL {error:#x}")); }
        self.pending = true;
        Ok(())
    }

    /// Blocking only at consumption; reads HEADER + page_count uints, no pixels.
    pub unsafe fn read(&mut self, gl: &glow::Context) -> Result<Feedback, String> {
        if !self.pending { return Err("分页紧凑反馈尚未执行".into()); }
        gl.bind_buffer(glow::SHADER_STORAGE_BUFFER, Some(self.buffer));
        gl.get_buffer_sub_data(glow::SHADER_STORAGE_BUFFER, 0, &mut self.bytes);
        gl.bind_buffer(glow::SHADER_STORAGE_BUFFER, None);
        self.pending = false;
        let error = gl.get_error();
        if error != glow::NO_ERROR { return Err(format!("分页紧凑反馈读取失败: GL {error:#x}")); }
        let words: Vec<u32> = self.bytes.chunks_exact(4)
            .map(|b| u32::from_ne_bytes(b.try_into().unwrap())).collect();
        Ok(decode(&words))
    }

    /// Must run while the owning GL context is current (no context-free Drop).
    pub unsafe fn destroy(self, gl: &glow::Context) {
        gl.bind_buffer_base(glow::SHADER_STORAGE_BUFFER, 0, None);
        gl.delete_buffer(self.buffer);
        gl.delete_program(self.program);
    }
}

fn decode(words: &[u32]) -> Feedback {
    let mut requests: Vec<_> = words[HEADER..].iter().enumerate()
        .filter(|(_, count)| **count != 0).map(|(page, &count)| (page as i32, count)).collect();
    requests.sort_by_key(|&(page, count)| (std::cmp::Reverse(count), page));
    Feedback { done: words[0], current_done: words[1], yielded: words[2],
        invalid: words[3], total: words[4], waiting: words[5], active: words[6], requests }
}




