//! Explicit source-export stage/layout over the already selected image
//! integrator. This module does not regenerate traversal, state IO or commit.
//! Production routing is frozen by SourceExportPolicy; diagnostic overrides
//! and their environment guards exist only in test builds.
use glow::HasContext;

pub(super) const ST_UNIT:u32=13;
pub(super) const MAX_PIXELS:u32=65_536;


#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(super) enum Mode{Line32,Tile8x4,Tile4x8,Tile8x8,Tile16x4,Tile16x8,Tile16x16,Tile8x2,Tile4x4,Tile16x2}
impl Mode{
    pub fn parse(value:&str)->Result<Self,String>{match value{
        "line32"=>Ok(Self::Line32),"8x4"=>Ok(Self::Tile8x4),"4x8"=>Ok(Self::Tile4x8),
        "8x8"=>Ok(Self::Tile8x8),"16x4"=>Ok(Self::Tile16x4),"16x8"=>Ok(Self::Tile16x8),"16x16"=>Ok(Self::Tile16x16),
        "8x2"=>Ok(Self::Tile8x2),"4x4"=>Ok(Self::Tile4x4),
        "16x2"=>Ok(Self::Tile16x2),
        _=>Err("ComputeImageLayout模式必须为line32、8x4、4x8、8x8、16x4、16x8、16x16、8x2、4x4或16x2".into()),
    }}
    pub fn name(self)->&'static str{match self{Self::Line32=>"line32",Self::Tile8x4=>"8x4",Self::Tile4x8=>"4x8",Self::Tile8x8=>"8x8",Self::Tile16x4=>"16x4",Self::Tile16x8=>"16x8",Self::Tile16x16=>"16x16",Self::Tile8x2=>"8x2",Self::Tile4x4=>"4x4",Self::Tile16x2=>"16x2"}}
    pub fn local(self)->[u32;3]{match self{Self::Line32=>[32,1,1],Self::Tile8x4=>[8,4,1],Self::Tile4x8=>[4,8,1],Self::Tile8x8=>[8,8,1],Self::Tile16x4=>[16,4,1],Self::Tile16x8=>[16,8,1],Self::Tile16x16=>[16,16,1],Self::Tile8x2=>[8,2,1],Self::Tile4x4=>[4,4,1],Self::Tile16x2=>[16,2,1]}}
    pub fn shape(self,size:[u32;2])->Result<[u32;3],String>{
        let total=checked_total(size)?;let local=self.local();
        Ok(if self==Self::Line32{[total.div_ceil(32),1,1]}else{[size[0].div_ceil(local[0]),size[1].div_ceil(local[1]),1]})
    }
}
fn checked_total(size:[u32;2])->Result<u32,String>{size[0].checked_mul(size[1])
    .filter(|&n|n>0&&n<=MAX_PIXELS&&size.iter().all(|&v|v<=i32::MAX as u32))
    .ok_or_else(||"ComputeImageLayout工作块必须为1..65536像素".into())}

/// Existing D120 policy flags are allowed; alternate stage/scheduler/query
/// experiments must be rejected by the caller before allocating resources.
pub(super) fn validate_scope(geo:bool,bilinear:bool,source:bool,image:bool,other_experiment:bool)->Result<(),String>{
    if geo&&bilinear&&source&&image&&!other_experiment{Ok(())}
    else{Err("ComputeImageLayout仅支持源Geo/Bilinear/Image，不能叠加其它阶段、异步或查询试验".into())}
}

/// Preserve all selected image source operations. Only the stage interface,
/// logical fragment coordinate and entry-point wrapper change.
pub(super) fn transform(image_source:&str,mode:Mode)->Result<String,String>{
    let mut shader=image_source.replace("\r\n","\n");
    if !shader.starts_with("#version 330 compatibility\n")||shader.matches("void main(){").count()!=1
        ||shader.matches("varying vec2 st;").count()!=1
        ||!shader.contains("uniform image2DArray rs_state_image;")
        ||!shader.contains("#define RS_GEO")||!shader.contains("#define PRIM_SURF\n")
        ||!shader.contains("void rs_run(){")||!shader.contains("RS_MUTABLE_LAYERS"){
        return Err("ComputeImageLayout需要已生成的Geo/Bilinear image片元源码".into());
    }
    for forbidden in ["gl_FragDepth","gl_FrontFacing","gl_PointCoord","discard;","dFdx(","dFdy(","fwidth(","as_sample_cursor","rs_phase_filter"]{
        if shader.contains(forbidden){return Err(format!("ComputeImageLayout不支持阶段或调度符号{forbidden}"));}
    }
    shader=shader.replacen("#version 330 compatibility","#version 430 compatibility",1);
    shader=shader.replacen("varying vec2 st;","vec2 st;",1);
    shader=shader.replacen("void main(){","void cil_integrate_main(){",1);
    shader=shader.replace("gl_FragCoord","cil_fragcoord");
    // Global declarations precede every transformed reference. Retain the
    // original leading version/extension block and later directives verbatim.
    let insertion=shader.lines().take_while(|line|line.starts_with("#version")||line.starts_with("#extension"))
        .map(|line|line.len()+1).sum::<usize>();
    let local=mode.local();
    shader.insert_str(insertion,&format!("layout(local_size_x={},local_size_y={},local_size_z=1) in;\nvec4 cil_fragcoord;\nuniform ivec2 cil_extent;\nuniform sampler2D cil_interpolated_st;\n",local[0],local[1]));
    shader.push_str("\nvoid main(){\n");
    if mode==Mode::Line32{
        shader.push_str("    uint index=gl_GlobalInvocationID.x;\n    if(index>=uint(cil_extent.x)*uint(cil_extent.y))return;\n    ivec2 pixel=ivec2(index%uint(cil_extent.x),index/uint(cil_extent.x));\n");
    }else{
        shader.push_str("    ivec2 pixel=ivec2(gl_GlobalInvocationID.xy);\n    if(any(greaterThanEqual(pixel,cil_extent)))return;\n");
    }
    // Original VERT has clip z=0,w=1 and the renderer uses default depth range:
    // actual fragment zw are exactly .5 and 1. All current consumers use xy.
    shader.push_str("    cil_fragcoord=vec4(vec2(pixel)+vec2(.5),.5,1.);\n    st=texelFetch(cil_interpolated_st,pixel,0).xy;\n    cil_integrate_main();\n}\n");
    Ok(shader)
}
pub(super) unsafe fn build(gl:&glow::Context,image_source:&str,mode:Mode)->Result<glow::Program,String>{
    let source=transform(image_source,mode)?;
    super::geo_merged::link(gl,&[(glow::COMPUTE_SHADER,&source)])
}

pub(super) struct StCapture{program:glow::Program,texture:glow::Texture,fbo:glow::Framebuffer,size:[u32;2]}
impl StCapture{
    pub unsafe fn create(gl:&glow::Context,size:[u32;2])->Result<Self,String>{
        checked_total(size)?;
        let version=gl.version();
        if version.is_embedded||(version.major,version.minor)<(4,3)
            ||gl.get_parameter_i32(glow::MAX_COMPUTE_TEXTURE_IMAGE_UNITS)<=ST_UNIT as i32
            ||size.iter().any(|&n|n>gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE)as u32){
            return Err("ComputeImageLayout需要GL4.3、计算采样单元13及足够纹理尺寸".into());
        }
        let program=super::geo_merged::link(gl,&[(glow::VERTEX_SHADER,super::gl_renderer::VERT),
            (glow::FRAGMENT_SHADER,super::raystate_shader::INTERPOLATED_ST_FRAGMENT)])?;
        let texture=match gl.create_texture(){Ok(t)=>t,Err(e)=>{gl.delete_program(program);return Err(e)}};
        let fbo=match gl.create_framebuffer(){Ok(f)=>f,Err(e)=>{gl.delete_texture(texture);gl.delete_program(program);return Err(e)}};
        let saved=CaptureState::save(gl);
        gl.active_texture(glow::TEXTURE0+ST_UNIT);gl.bind_texture(glow::TEXTURE_2D,Some(texture));
        gl.tex_parameter_i32(glow::TEXTURE_2D,glow::TEXTURE_MIN_FILTER,glow::NEAREST as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D,glow::TEXTURE_MAG_FILTER,glow::NEAREST as i32);
        gl.tex_image_2d(glow::TEXTURE_2D,0,glow::RG32F as i32,size[0]as i32,size[1]as i32,0,glow::RG,glow::FLOAT,glow::PixelUnpackData::Slice(None));
        let error=gl.get_error();saved.restore(gl);
        if error!=glow::NO_ERROR{gl.delete_framebuffer(fbo);gl.delete_texture(texture);gl.delete_program(program);return Err(format!("ComputeImageLayout插值纹理分配失败GL{error:#x}"));}
        Ok(Self{program,texture,fbo,size})
    }
    pub fn memory_bytes(&self)->u64{u64::from(self.size[0])*u64::from(self.size[1])*8}
    pub fn size(&self)->[u32;2]{self.size}
    /// Read uniforms from the actual original raster program, not from a
    /// compute program in which VERT-only uniforms may have been optimized out.
    pub unsafe fn capture(&self,gl:&glow::Context,original:glow::Program)->Result<(),String>{
        let mut aspect=[0f32;2];let mut jitter=[0f32;2];
        for(name,value)in[("aspect",&mut aspect),("pixel_jitter",&mut jitter)]{
            let location=gl.get_uniform_location(original,name).ok_or_else(||format!("ComputeImageLayout原光栅程序缺少{name}"))?;
            gl.get_uniform_f32(original,&location,value);
        }
        self.capture_values(gl,aspect,jitter)
    }
    /// Host values must be the identical f32 values supplied to original VERT.
    /// ST is still produced by rasterization, never reconstructed on the CPU.
    pub unsafe fn capture_values(&self,gl:&glow::Context,aspect:[f32;2],jitter:[f32;2])->Result<(),String>{
        if aspect.iter().any(|v|!v.is_finite())||jitter.iter().any(|&v|v!=0.){
            return Err("ComputeImageLayout仅捕获有限aspect及Geo零抖动".into());
        }
        let saved=CaptureState::save(gl);
        let result=(||{
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER,Some(self.fbo));
            gl.framebuffer_texture_2d(glow::DRAW_FRAMEBUFFER,glow::COLOR_ATTACHMENT0,glow::TEXTURE_2D,Some(self.texture),0);
            gl.draw_buffers(&[glow::COLOR_ATTACHMENT0]);
            if gl.check_framebuffer_status(glow::DRAW_FRAMEBUFFER)!=glow::FRAMEBUFFER_COMPLETE{return Err("ComputeImageLayout插值FBO不完整".into());}
            gl.viewport(0,0,self.size[0]as i32,self.size[1]as i32);
            for capability in CAPABILITIES{gl.disable(capability);}
            gl.color_mask(true,true,true,true);gl.use_program(Some(self.program));
            gl.uniform_2_f32(gl.get_uniform_location(self.program,"aspect").as_ref(),aspect[0],aspect[1]);
            gl.uniform_2_f32(gl.get_uniform_location(self.program,"pixel_jitter").as_ref(),jitter[0],jitter[1]);
            gl.draw_arrays(glow::TRIANGLES,0,3);
            gl.memory_barrier(glow::FRAMEBUFFER_BARRIER_BIT|glow::TEXTURE_FETCH_BARRIER_BIT);
            let error=gl.get_error();if error!=glow::NO_ERROR{return Err(format!("ComputeImageLayout捕获失败GL{error:#x}"));}Ok(())
        })();
        saved.restore(gl);result
    }
    /// Leaves unit13 bound for the following dispatch; restores active unit.
    /// Caller must have made `compute` current, as for other sampler binds.
    pub unsafe fn bind(&self,gl:&glow::Context,compute:glow::Program){
        let active=gl.get_parameter_i32(glow::ACTIVE_TEXTURE)as u32;
        gl.active_texture(glow::TEXTURE0+ST_UNIT);gl.bind_texture(glow::TEXTURE_2D,Some(self.texture));
        gl.uniform_1_i32(gl.get_uniform_location(compute,"cil_interpolated_st").as_ref(),ST_UNIT as i32);
        gl.uniform_2_i32(gl.get_uniform_location(compute,"cil_extent").as_ref(),self.size[0]as i32,self.size[1]as i32);
        gl.active_texture(active);
    }
    pub unsafe fn destroy(self,gl:&glow::Context){gl.delete_framebuffer(self.fbo);gl.delete_texture(self.texture);gl.delete_program(self.program);}
}
const CAPABILITIES:[u32;6]=[glow::BLEND,glow::SCISSOR_TEST,glow::DEPTH_TEST,glow::STENCIL_TEST,glow::CULL_FACE,glow::RASTERIZER_DISCARD];
struct CaptureState{program:Option<glow::Program>,fbo:Option<glow::Framebuffer>,viewport:[i32;4],active:u32,texture:Option<glow::Texture>,enabled:[bool;6],mask:[i32;4]}
impl CaptureState{
    unsafe fn save(gl:&glow::Context)->Self{
        let active=gl.get_parameter_i32(glow::ACTIVE_TEXTURE)as u32;
        gl.active_texture(glow::TEXTURE0+ST_UNIT);let texture=gl.get_parameter_texture(glow::TEXTURE_BINDING_2D);gl.active_texture(active);
        let mut viewport=[0;4];gl.get_parameter_i32_slice(glow::VIEWPORT,&mut viewport);
        let mut mask=[0;4];gl.get_parameter_i32_slice(glow::COLOR_WRITEMASK,&mut mask);
        Self{program:gl.get_parameter_program(glow::CURRENT_PROGRAM),fbo:gl.get_parameter_framebuffer(glow::DRAW_FRAMEBUFFER_BINDING),viewport,active,texture,enabled:CAPABILITIES.map(|cap|gl.is_enabled(cap)),mask}
    }
    unsafe fn restore(self,gl:&glow::Context){
        gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER,self.fbo);gl.viewport(self.viewport[0],self.viewport[1],self.viewport[2],self.viewport[3]);gl.use_program(self.program);
        gl.active_texture(glow::TEXTURE0+ST_UNIT);gl.bind_texture(glow::TEXTURE_2D,self.texture);gl.active_texture(self.active);
        for(cap,enabled)in CAPABILITIES.into_iter().zip(self.enabled){if enabled{gl.enable(cap)}else{gl.disable(cap)}}
        gl.color_mask(self.mask[0]!=0,self.mask[1]!=0,self.mask[2]!=0,self.mask[3]!=0);
    }
}
