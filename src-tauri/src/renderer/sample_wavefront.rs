//! Isolated multi-sample resource foundation. No export entry point selects it.
use glow::HasContext;
pub(super) const SEGMENT_PATHS:u32=65536;
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(super) struct Layout{pub size:[u32;2],pub pixels:u32,pub lanes:u32,pub layers:u32,pub paths:u32,pub state_size:[u32;2]}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(super) struct Block{pub base:u32,pub count:u32}
impl Layout{
    pub fn new(size:[u32;2],lanes:u32,layers:u32)->Result<Self,String>{
        let fail=||"并发样本布局无效或溢出".to_string();
        if ![1,4,8,16].contains(&lanes)||![15,17,19].contains(&layers){return Err(fail());}
        let pixels=size[0].checked_mul(size[1]).filter(|&v|v>0&&v<=65536).ok_or_else(fail)?;
        let paths=pixels.checked_mul(lanes).ok_or_else(fail)?;
        let height=size[1].checked_mul(lanes).ok_or_else(fail)?;
        Ok(Self{size,pixels,lanes,layers,paths,state_size:[size[0],height]})
    }
    /// Sample zero is already accumulated. A final partial block has no fake lanes.
    pub fn block(self,base:u32,target:u32)->Option<Block>{
        if base==0||base>=target||target>256{return None;}Some(Block{base,count:(target-base).min(self.lanes)})
    }
    pub fn path(self,pixel:u32,lane:u32)->Option<u32>{if pixel>=self.pixels||lane>=self.lanes{None}else{Some(lane*self.pixels+pixel)}}
    pub fn decode(self,path:u32)->Option<(u32,u32)>{if path>=self.paths{None}else{Some((path%self.pixels,path/self.pixels))}}
    pub fn sample_record(self,block:Block,path:u32,stride:u32,table_len:usize)->Option<usize>{
        let (_,lane)=self.decode(path)?;if lane>=block.count||block.count>self.lanes||block.base==0{return None;}
        let sample=block.base.checked_add(lane)?;let begin=(sample as usize).checked_mul(stride as usize)?;
        if stride==0||begin.checked_add(stride as usize)?>table_len{None}else{Some(begin)}
    }
    pub fn segments(self)->impl Iterator<Item=(u32,u32)>{(0..self.paths).step_by(SEGMENT_PATHS as usize).map(move|base|(base,(self.paths-base).min(SEGMENT_PATHS)))}
    pub fn state_bytes(self)->Option<u64>{u64::from(self.paths).checked_mul(u64::from(self.layers))?.checked_mul(16)}
    /// Future four fixed queues plus optional caller metadata/table/pages/coarse.
    /// Only state_bytes is allocated by Resources; no hidden queue allocation.
    pub fn joint_bytes(self,metadata_per_path:u64,table:u64,other:u64)->Option<u64>{
        self.state_bytes()?.checked_add(u64::from(self.paths).checked_mul(16)?.checked_add(128)?)?
            .checked_add(u64::from(self.paths).checked_mul(metadata_per_path)?)?.checked_add(table)?.checked_add(other)
    }
}
const VERTEX:&str="#version 430 compatibility\nvoid main(){vec2 p=vec2((gl_VertexID<<1)&2,gl_VertexID&2);gl_Position=vec4(p*2.-1.,0.,1.);}";
const COMMIT:&str=r#"#version 430 compatibility
layout(rgba32f,binding=0) readonly uniform image2DArray sw_state;
uniform int sw_lane,sw_height,sw_reference;
uniform vec4 sw_color;
out vec4 sw_output;
void main(){ivec2 p=ivec2(gl_FragCoord.xy);p.y+=sw_lane*sw_height;
sw_output=sw_reference!=0?sw_color:imageLoad(sw_state,ivec3(p,6));}
"#;
pub(super) struct Resources{pub state:glow::Texture,program:glow::Program,pub layout:Layout}
unsafe fn program(gl:&glow::Context)->Result<glow::Program,String>{
    let p=gl.create_program()?;
    for(kind,src)in[(glow::VERTEX_SHADER,VERTEX),(glow::FRAGMENT_SHADER,COMMIT)]{
        let s=match gl.create_shader(kind){Ok(s)=>s,Err(e)=>{gl.delete_program(p);return Err(e)}};
        gl.shader_source(s,src);gl.compile_shader(s);
        if !gl.get_shader_compile_status(s){let e=gl.get_shader_info_log(s);gl.delete_shader(s);gl.delete_program(p);return Err(e);}
        gl.attach_shader(p,s);gl.delete_shader(s);
    }
    gl.link_program(p);if !gl.get_program_link_status(p){let e=gl.get_program_info_log(p);gl.delete_program(p);return Err(e);}Ok(p)
}
impl Resources{
    pub unsafe fn create(gl:&glow::Context,layout:Layout)->Result<Self,String>{
        let v=gl.version();if v.is_embedded||(v.major,v.minor)<(4,3){return Err("并发样本需要OpenGL4.3".into());}
        let max=gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE);let layers=gl.get_parameter_i32(glow::MAX_ARRAY_TEXTURE_LAYERS);
        if layout.state_size.iter().any(|&n|n>max as u32)||layout.layers>layers as u32{return Err("并发样本状态超过设备纹理限制".into());}
        let p=program(gl)?;let state=match gl.create_texture(){Ok(t)=>t,Err(e)=>{gl.delete_program(p);return Err(e)}};
        let old=gl.get_parameter_texture(glow::TEXTURE_BINDING_2D_ARRAY);
        gl.bind_texture(glow::TEXTURE_2D_ARRAY,Some(state));
        gl.tex_storage_3d(glow::TEXTURE_2D_ARRAY,1,glow::RGBA32F,layout.state_size[0]as i32,layout.state_size[1]as i32,layout.layers as i32);
        gl.bind_texture(glow::TEXTURE_2D_ARRAY,old);
        let error=gl.get_error();if error!=glow::NO_ERROR{gl.delete_texture(state);gl.delete_program(p);return Err(format!("并发样本状态分配失败GL{error:#x}"));}
        Ok(Self{state,program:p,layout})
    }
    /// Caller guarantees ALL block paths DONE, binds original accumulation FBO,
    /// original viewport/scissor and VAO. No status clear or sample reordering.
    /// Alpha slice contains the original CPU float for every global sample.
    pub unsafe fn commit_ordered(&self,gl:&glow::Context,block:Block,alphas:&[f32])->Result<(),String>{
        if block.base==0||block.count==0||block.count>self.layout.lanes||block.base.checked_add(block.count).map_or(true,|end|end as usize>alphas.len()){
            return Err("并发样本有序提交范围无效".into());}
        let end=block.base+block.count;
        if alphas[block.base as usize..end as usize].iter().any(|a|!a.is_finite()||*a<0.||*a>1.){return Err("并发样本混合alpha无效".into());}
        let previous=gl.get_parameter_program(glow::CURRENT_PROGRAM);
        gl.memory_barrier(glow::SHADER_IMAGE_ACCESS_BARRIER_BIT|glow::FRAMEBUFFER_BARRIER_BIT);
        gl.use_program(Some(self.program));gl.bind_image_texture(0,Some(self.state),0,true,0,glow::READ_ONLY,glow::RGBA32F);
        gl.uniform_1_i32(gl.get_uniform_location(self.program,"sw_height").as_ref(),self.layout.size[1]as i32);
        gl.uniform_1_i32(gl.get_uniform_location(self.program,"sw_reference").as_ref(),0);
        gl.enable(glow::BLEND);gl.blend_equation(glow::FUNC_ADD);gl.blend_func(glow::CONSTANT_ALPHA,glow::ONE_MINUS_CONSTANT_ALPHA);
        for lane in 0..block.count{
            gl.uniform_1_i32(gl.get_uniform_location(self.program,"sw_lane").as_ref(),lane as i32);
            gl.blend_color(0.,0.,0.,alphas[(block.base+lane)as usize]);gl.draw_arrays(glow::TRIANGLES,0,3);
        }
        gl.disable(glow::BLEND);gl.use_program(previous);gl.memory_barrier(glow::FRAMEBUFFER_BARRIER_BIT);
        let error=gl.get_error();if error!=glow::NO_ERROR{return Err(format!("并发样本提交失败GL{error:#x}"));}Ok(())
    }
    pub unsafe fn destroy(self,gl:&glow::Context){gl.delete_texture(self.state);gl.delete_program(self.program);}
}


// Feasibility oracle only: no production sample integration or blend change.

