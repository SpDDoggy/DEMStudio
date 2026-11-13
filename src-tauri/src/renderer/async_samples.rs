//! Independent per-pixel sample cursors. Binding 4, sample table binding 5.
//! Start only after original sample zero and immutable-primary memo are complete.
use glow::HasContext;
pub(super) const SHADER_HEADER:&str=r#"
layout(std430,binding=4) buffer AsyncSamples { uint as_words[]; };
uniform ivec2 as_extent;
uniform uint as_target,as_stride,as_alpha_offset;
uint as_pixel_index(){return uint(gl_FragCoord.y)*uint(as_extent.x)+uint(gl_FragCoord.x);}
uint as_cursor(){return as_words[4u+2u*as_pixel_index()];}
bool as_initialized(){return as_words[5u+2u*as_pixel_index()]!=0u;}
void as_begin_initialized(){as_words[5u+2u*as_pixel_index()]=1u;}
"#;
pub(super) fn commit_source()->String{format!("#version 430 compatibility\n{}\n{}",SHADER_HEADER,r#"
layout(std430,binding=5) readonly buffer AsyncSampleTable { float as_values[]; };
layout(rgba32f,binding=0) uniform image2DArray as_state;
layout(location=0,index=0) out vec4 as_color;
layout(location=0,index=1) out vec4 as_factor;
void main(){
    uint cursor=as_cursor();ivec2 coord=ivec2(gl_FragCoord.xy);
    if(cursor>=as_target||!as_initialized())discard;
    vec4 status=imageLoad(as_state,ivec3(coord,5));
    if(status.y!=4.)discard;
    as_color=imageLoad(as_state,ivec3(coord,6));
    as_factor=vec4(0.,0.,0.,as_values[cursor*as_stride+as_alpha_offset]);
    uint next=cursor+1u;as_words[4u+2u*as_pixel_index()]=next;
    as_words[5u+2u*as_pixel_index()]=0u;
    // Finished pixels stay DONE for compact feedback; new samples must initialize.
    if(next<as_target)imageStore(as_state,ivec3(coord,5),vec4(-1.,2.,0.,0.));
}
"#)}

#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(super) struct AsyncProgress{pub completed_samples:u32,pub finished_pixels:u32,pub invalid:u32}
pub(super) struct AsyncSamples{program:glow::Program,commit:glow::Program,buffer:glow::Buffer,table:glow::Buffer,capacity:u32,target:u32,stride:u32,size:std::cell::Cell<[u32;2]>}
fn checked_total(capacity:u32,size:[u32;2],target:u32)->Result<u32,String>{
    let total=size[0].checked_mul(size[1]).filter(|&n|n>0&&n<=capacity).ok_or("独立采样工作块容量无效")?;
    if !(2..=256).contains(&target)||target.checked_mul(total).is_none(){return Err("独立采样首版仅支持2至256采样".into());}Ok(total)
}
pub(super) fn measure_activity(mode:&str,first:bool,hashed:bool)->bool{mode!="off"&&(mode=="on"||first||hashed)}
unsafe fn build_program(gl:&glow::Context,sources:&[(u32,&str)])->Result<glow::Program,String>{
    let p=gl.create_program()?;
    for &(kind,source)in sources{
        let shader=match gl.create_shader(kind){Ok(s)=>s,Err(e)=>{gl.delete_program(p);return Err(e)}};
        gl.shader_source(shader,source);gl.compile_shader(shader);
        if !gl.get_shader_compile_status(shader){let e=gl.get_shader_info_log(shader);gl.delete_shader(shader);gl.delete_program(p);return Err(e);}
        gl.attach_shader(p,shader);gl.delete_shader(shader);
    }
    gl.link_program(p);if !gl.get_program_link_status(p){let e=gl.get_program_info_log(p);gl.delete_program(p);return Err(e);}Ok(p)
}
impl AsyncSamples{
    pub unsafe fn create(gl:&glow::Context,capacity:u32,target:u32,values:&[f32],stride:u32)->Result<Option<Self>,String>{
        let v=gl.version();if v.is_embedded||(v.major,v.minor)<(4,3)||capacity==0||capacity>65536{return Ok(None);}
        checked_total(capacity,[capacity,1],target)?;
        if stride<14||(stride-8)%6!=0||values.len()!=target as usize*stride as usize{return Err("独立采样随机表布局无效".into());}
        if gl.get_parameter_i32(glow::MAX_DUAL_SOURCE_DRAW_BUFFERS)<1{return Ok(None);}
        let bytes=(4+2*capacity)*4;let table_bytes=values.len()*4;
        let limit=gl.get_parameter_i64(glow::MAX_SHADER_STORAGE_BLOCK_SIZE);
        if limit<i64::from(bytes)||limit<table_bytes as i64{return Ok(None);}
        let program=build_program(gl,&[(glow::COMPUTE_SHADER,include_str!("async_samples.glsl"))])?;
        // Both samplers remain active in the linked multi-mode compute program.
        // Distinct units are required even for reset/progress modes that do not fetch.
        let previous=gl.get_parameter_program(glow::CURRENT_PROGRAM);gl.use_program(Some(program));
        gl.uniform_1_i32(gl.get_uniform_location(program,"as_activity_state").as_ref(),6);
        gl.uniform_1_i32(gl.get_uniform_location(program,"as_activity_pages").as_ref(),9);gl.use_program(previous);
        let fragment=commit_source();
        let vertex="#version 430 compatibility\nvoid main(){vec2 p=vec2((gl_VertexID<<1)&2,gl_VertexID&2);gl_Position=vec4(p*2.-1.,0.,1.);}";
        let commit=match build_program(gl,&[(glow::VERTEX_SHADER,vertex),(glow::FRAGMENT_SHADER,&fragment)]){Ok(p)=>p,Err(e)=>{gl.delete_program(program);return Err(e)}};
        let buffer=match gl.create_buffer(){Ok(b)=>b,Err(e)=>{gl.delete_program(program);gl.delete_program(commit);return Err(e)}};
        let table=match gl.create_buffer(){Ok(b)=>b,Err(e)=>{gl.delete_program(program);gl.delete_program(commit);gl.delete_buffer(buffer);return Err(e)}};
        gl.bind_buffer(glow::SHADER_STORAGE_BUFFER,Some(buffer));gl.buffer_data_size(glow::SHADER_STORAGE_BUFFER,bytes as i32,glow::DYNAMIC_COPY);
        gl.bind_buffer(glow::SHADER_STORAGE_BUFFER,Some(table));let raw=std::slice::from_raw_parts(values.as_ptr().cast::<u8>(),table_bytes);
        gl.buffer_data_u8_slice(glow::SHADER_STORAGE_BUFFER,raw,glow::STATIC_DRAW);gl.bind_buffer(glow::SHADER_STORAGE_BUFFER,None);
        let error=gl.get_error();if error!=glow::NO_ERROR{gl.delete_buffer(buffer);gl.delete_buffer(table);gl.delete_program(program);gl.delete_program(commit);return Err(format!("独立采样分配失败: GL {error:#x}"));}
        Ok(Some(Self{program,commit,buffer,table,capacity,target,stride,size:std::cell::Cell::new([0,0])}))
    }
    /// Reuse compiled programs and metadata allocation across finished tiles.
    /// Caller has consumed the previous tile before replacing its random table.
    pub unsafe fn update_table(&self,gl:&glow::Context,values:&[f32],stride:u32,target:u32)->Result<(),String>{
        if stride!=self.stride||target!=self.target||values.len()!=self.target as usize*self.stride as usize{
            return Err("独立采样复用随机表布局不一致".into());
        }
        let raw=std::slice::from_raw_parts(values.as_ptr().cast::<u8>(),values.len()*4);
        gl.bind_buffer(glow::SHADER_STORAGE_BUFFER,Some(self.table));
        gl.buffer_sub_data_u8_slice(glow::SHADER_STORAGE_BUFFER,0,raw);
        gl.bind_buffer_base(glow::SHADER_STORAGE_BUFFER,5,Some(self.table));
        gl.bind_buffer(glow::SHADER_STORAGE_BUFFER,None);
        gl.memory_barrier(glow::BUFFER_UPDATE_BARRIER_BIT|glow::SHADER_STORAGE_BARRIER_BIT);
        let error=gl.get_error();if error!=glow::NO_ERROR{return Err(format!("独立采样随机表更新失败: GL {error:#x}"));}Ok(())
    }
    pub fn memory_bytes(&self)->u64{u64::from((4+2*self.capacity)*4)+u64::from(self.target)*u64::from(self.stride)*4}
    unsafe fn bind_buffers(&self,gl:&glow::Context){gl.bind_buffer_base(glow::SHADER_STORAGE_BUFFER,4,Some(self.buffer));gl.bind_buffer_base(glow::SHADER_STORAGE_BUFFER,5,Some(self.table));}
    pub unsafe fn bind(&self,gl:&glow::Context,program:glow::Program){
        self.bind_buffers(gl);gl.use_program(Some(program));let size=self.size.get();
        gl.uniform_2_i32(gl.get_uniform_location(program,"as_extent").as_ref(),size[0]as i32,size[1]as i32);
        for(name,value)in[("as_target",self.target),("as_stride",self.stride),("as_alpha_offset",self.stride-3)]{gl.uniform_1_u32(gl.get_uniform_location(program,name).as_ref(),value);}
    }
    unsafe fn mode(&self,gl:&glow::Context,mode:u32,total:u32,groups:u32){
        self.bind(gl,self.program);
        for(name,value)in[("as_mode",mode),("as_total",total)]{gl.uniform_1_u32(gl.get_uniform_location(self.program,name).as_ref(),value);}
        gl.dispatch_compute(groups,1,1);gl.memory_barrier(glow::SHADER_STORAGE_BARRIER_BIT);
    }
    pub unsafe fn reset(&self,gl:&glow::Context,size:[u32;2],target:u32)->Result<(),String>{
        if target!=self.target{return Err("独立采样目标与随机表不一致".into());}let total=checked_total(self.capacity,size,target)?;self.size.set(size);self.mode(gl,0,total,total.div_ceil(256));Ok(())
    }
    /// Caller binds a single RGBA32F accumulation color target and original viewport/VAO.
    pub unsafe fn commit(&self,gl:&glow::Context,state:glow::Texture,size:[u32;2])->Result<(),String>{
        if size!=self.size.get(){return Err("独立采样提交尺寸不一致".into());}self.bind(gl,self.commit);
        gl.bind_image_texture(0,Some(state),0,true,0,glow::READ_WRITE,glow::RGBA32F);
        gl.uniform_1_i32(gl.get_uniform_location(self.commit,"as_state").as_ref(),0);
        gl.enable(glow::BLEND);gl.blend_equation(glow::FUNC_ADD);gl.blend_func(glow::SRC1_ALPHA,glow::ONE_MINUS_SRC1_ALPHA);
        gl.draw_arrays(glow::TRIANGLES,0,3);gl.disable(glow::BLEND);
        gl.memory_barrier(glow::SHADER_IMAGE_ACCESS_BARRIER_BIT|glow::TEXTURE_FETCH_BARRIER_BIT|glow::SHADER_STORAGE_BARRIER_BIT);
        let e=gl.get_error();if e!=glow::NO_ERROR{return Err(format!("独立采样双源提交失败: GL {e:#x}"));}Ok(())
    }
    /// Two GPU reduction dispatches followed by one bounded batch consumption.
    /// Counts samples AFTER the preserved first sample, rather than sum(cursor).
    pub unsafe fn progress(&self,gl:&glow::Context)->Result<AsyncProgress,String>{
        self.dispatch_progress(gl)?;self.read(gl)
    }
    pub unsafe fn dispatch_progress(&self,gl:&glow::Context)->Result<(),String>{
        let total=checked_total(self.capacity,self.size.get(),self.target)?;
        self.mode(gl,1,total,1);self.mode(gl,2,total,total.div_ceil(256));Ok(())
    }
    /// Eligibility before one integration Draw. DONE pending commit is eligible;
    /// finished cursors and unresolved page waits are excluded. State image0 is untouched.
    pub unsafe fn record_activity(&self,gl:&glow::Context,batch:&crate::renderer::gpu_batch::GpuBatch,
        state:glow::Texture,pages:glow::Texture)->Result<(),String>{
        let total=checked_total(self.capacity,self.size.get(),self.target)?;
        let previous=gl.get_parameter_program(glow::CURRENT_PROGRAM);
        let active=gl.get_parameter_i32(glow::ACTIVE_TEXTURE)as u32;
        gl.active_texture(glow::TEXTURE0+6);let previous_state=gl.get_parameter_texture(glow::TEXTURE_BINDING_2D_ARRAY);
        gl.bind_texture(glow::TEXTURE_2D_ARRAY,Some(state));
        gl.active_texture(glow::TEXTURE0+9);let previous_pages=gl.get_parameter_texture(glow::TEXTURE_BINDING_2D);
        gl.bind_texture(glow::TEXTURE_2D,Some(pages));
        batch.bind(gl);self.bind(gl,self.program);
        gl.memory_barrier(glow::SHADER_IMAGE_ACCESS_BARRIER_BIT|glow::TEXTURE_FETCH_BARRIER_BIT|glow::SHADER_STORAGE_BARRIER_BIT);
        self.mode(gl,3,total,total.div_ceil(256));self.mode(gl,4,total,1);
        gl.active_texture(glow::TEXTURE0+6);gl.bind_texture(glow::TEXTURE_2D_ARRAY,previous_state);
        gl.active_texture(glow::TEXTURE0+9);gl.bind_texture(glow::TEXTURE_2D,previous_pages);
        gl.active_texture(active);gl.use_program(previous);
        let error=gl.get_error();if error!=glow::NO_ERROR{return Err(format!("独立采样活跃诊断失败: GL {error:#x}"));}Ok(())
    }
    pub unsafe fn read(&self,gl:&glow::Context)->Result<AsyncProgress,String>{
        gl.memory_barrier(glow::BUFFER_UPDATE_BARRIER_BIT|glow::SHADER_STORAGE_BARRIER_BIT);
        let mut bytes=[0u8;16];gl.bind_buffer(glow::SHADER_STORAGE_BUFFER,Some(self.buffer));gl.get_buffer_sub_data(glow::SHADER_STORAGE_BUFFER,0,&mut bytes);gl.bind_buffer(glow::SHADER_STORAGE_BUFFER,None);
        let e=gl.get_error();if e!=glow::NO_ERROR{return Err(format!("独立采样进度读取失败: GL {e:#x}"));}
        let w:Vec<u32>=bytes.chunks_exact(4).map(|b|u32::from_ne_bytes(b.try_into().unwrap())).collect();Ok(AsyncProgress{completed_samples:w[0],finished_pixels:w[1],invalid:w[2]})
    }
    pub unsafe fn destroy(self,gl:&glow::Context){gl.bind_buffer_base(glow::SHADER_STORAGE_BUFFER,4,None);gl.bind_buffer_base(glow::SHADER_STORAGE_BUFFER,5,None);gl.delete_buffer(self.buffer);gl.delete_buffer(self.table);gl.delete_program(self.program);gl.delete_program(self.commit);}
}






