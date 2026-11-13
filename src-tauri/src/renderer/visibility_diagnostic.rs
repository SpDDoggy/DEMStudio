//! Sampled comparison only: never changes pg_s, query return values or RNG.
//! Binding6 is exclusive with the future visibility continuation stack.
use glow::HasContext;
use super::{visibility_bounds::BoundsLevel,visibility_probe::ProbeParams};
const SCOPE_FIELDS:usize=15;
pub(super) const COUNTER_BYTES:usize=2*SCOPE_FIELDS*8;
const DIAGNOSTIC_HEADER:&str=r#"
#define VPD_DIAGNOSTIC
layout(std430,binding=6) buffer VisibilityDiagnostic {uint vpd_words[];};
bool vpd_sampled(){
    uvec2 p=uvec2(output_pixel());uint h=p.x*0x9e3779b9u+p.y*0x85ebca6bu;
    h^=h>>16;h*=0x7feb352du;h^=h>>15;return (h&4095u)==0u;
}
void vpd_add(uint scope,uint field,uint value){
    if(value==0u)return;uint at=(scope*15u+field)*2u;
    uint old=atomicAdd(vpd_words[at],value);
    if(old>0xffffffffu-value)atomicAdd(vpd_words[at+1u],1u);
}
uint vpd_begin(vec3 o,vec3 dir,vec2 interval,bool visibility){
    if(!vpd_sampled())return 3u;
    uint nodes;uint classification=vp_classify(o,dir,interval,nodes);
    uint scope=visibility?1u:0u;
    vpd_add(scope,classification,1u);vpd_add(scope,3u,nodes);return classification;
}
void vpd_original_counts(bool visibility,uint visited,uint leaves){uint scope=visibility?1u:0u;vpd_add(scope,13u,visited);vpd_add(scope,14u,leaves);}
void vpd_end(uint classification,bool visibility,bool hit,bool interrupted){
    if(classification>=3u)return;uint scope=visibility?1u:0u;
    if(interrupted)vpd_add(scope,10u+classification,1u);
    else vpd_add(scope,4u+classification*2u+uint(hit),1u);
}
"#;
pub(super) fn shader_header()->String{format!("{}\n{}",super::visibility_probe::SHADER_HEADER,DIAGNOSTIC_HEADER)}
pub(super) fn coarse_memory_bytes(levels:&[BoundsLevel])->u64{levels.iter().map(|l|l.values.len()as u64*8).sum()}
fn validate(levels:&[BoundsLevel])->Result<(),String>{
    let first=levels.first().ok_or("可见性诊断缺少coarse层")?;
    if first.level!=5||!first.edge.is_power_of_two()||levels.len()!=first.edge.trailing_zeros()as usize+1||levels.last().is_none_or(|l|l.edge!=1)
        ||levels.iter().enumerate().any(|(i,l)|l.level!=5+i as u32||l.edge!=first.edge>>i||l.values.len()!=l.edge as usize*l.edge as usize){return Err("可见性诊断coarse布局无效".into());}Ok(())
}
pub(super) struct Diagnostics{texture:glow::Texture,buffer:glow::Buffer,bytes:u64}
impl Diagnostics{
    pub unsafe fn create(gl:&glow::Context,levels:&[BoundsLevel])->Result<Self,String>{
        validate(levels)?;let version=gl.version();
        if version.is_embedded||(version.major,version.minor)<(4,3)||gl.get_parameter_i32(glow::MAX_TEXTURE_IMAGE_UNITS)<13
            ||levels[0].edge>gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE).max(0)as u32{return Err("可见性诊断需要GL4.3及13个片元纹理单元".into());}
        let active=gl.get_parameter_i32(glow::ACTIVE_TEXTURE)as u32;gl.active_texture(glow::TEXTURE0+12);
        let prior=gl.get_parameter_texture(glow::TEXTURE_BINDING_2D);
        let texture=match gl.create_texture(){Ok(t)=>t,Err(e)=>{gl.active_texture(active);return Err(e)}};
        let result=(||{
            gl.bind_texture(glow::TEXTURE_2D,Some(texture));
            gl.tex_parameter_i32(glow::TEXTURE_2D,glow::TEXTURE_MIN_FILTER,glow::NEAREST_MIPMAP_NEAREST as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D,glow::TEXTURE_MAG_FILTER,glow::NEAREST as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D,glow::TEXTURE_MAX_LEVEL,levels.len()as i32-1);
            for level in levels{
                let bytes=std::slice::from_raw_parts(level.values.as_ptr()as *const u8,level.values.len()*8);
                gl.tex_image_2d(glow::TEXTURE_2D,(level.level-5)as i32,glow::RG32F as i32,level.edge as i32,level.edge as i32,0,glow::RG,glow::FLOAT,glow::PixelUnpackData::Slice(Some(bytes)));
            }
            if gl.get_error()!=glow::NO_ERROR{return Err("可见性诊断coarse上传失败".into());}Ok(())
        })();
        gl.bind_texture(glow::TEXTURE_2D,prior);gl.active_texture(active);
        if let Err(e)=result{gl.delete_texture(texture);return Err(e);}
        let buffer=match gl.create_buffer(){Ok(b)=>b,Err(e)=>{gl.delete_texture(texture);return Err(e)}};
        let prior_buffer=gl.get_parameter_buffer(glow::SHADER_STORAGE_BUFFER_BINDING);
        gl.bind_buffer(glow::SHADER_STORAGE_BUFFER,Some(buffer));
        gl.buffer_data_u8_slice(glow::SHADER_STORAGE_BUFFER,&[0;COUNTER_BYTES],glow::DYNAMIC_COPY);
        gl.bind_buffer(glow::SHADER_STORAGE_BUFFER,prior_buffer);
        let error=gl.get_error();if error!=glow::NO_ERROR{gl.delete_buffer(buffer);gl.delete_texture(texture);return Err(format!("可见性诊断计数分配失败 GL {error:#x}"));}
        Ok(Self{texture,buffer,bytes:coarse_memory_bytes(levels)+COUNTER_BYTES as u64})
    }
    /// Caller has already selected program. Changes unit12 binding and SSBO6 only.
    pub unsafe fn bind(&self,gl:&glow::Context,program:glow::Program,params:&ProbeParams){
        gl.bind_buffer_base(glow::SHADER_STORAGE_BUFFER,6,Some(self.buffer));
        super::visibility_probe::bind(gl,program,self.texture,params);
    }
    pub fn memory_bytes(&self)->u64{self.bytes}
    /// Exactly one 240-byte readback at export end, after all dispatches/draws.
    pub unsafe fn final_read(&self,gl:&glow::Context)->serde_json::Value{
        gl.memory_barrier(glow::SHADER_STORAGE_BARRIER_BIT|glow::BUFFER_UPDATE_BARRIER_BIT);
        let prior=gl.get_parameter_buffer(glow::SHADER_STORAGE_BUFFER_BINDING);
        gl.bind_buffer(glow::SHADER_STORAGE_BUFFER,Some(self.buffer));let mut bytes=[0u8;COUNTER_BYTES];
        gl.get_buffer_sub_data(glow::SHADER_STORAGE_BUFFER,0,&mut bytes);gl.bind_buffer(glow::SHADER_STORAGE_BUFFER,prior);
        let mut result=decode(&bytes);let error=gl.get_error();result["readback_gl_error"]=serde_json::json!(error);result["valid_readback"]=serde_json::json!(error==glow::NO_ERROR);result
    }
    pub unsafe fn destroy(self,gl:&glow::Context){gl.bind_buffer_base(glow::SHADER_STORAGE_BUFFER,6,None);gl.delete_buffer(self.buffer);gl.delete_texture(self.texture);}
}
fn decode(bytes:&[u8;COUNTER_BYTES])->serde_json::Value{
    let mut result=serde_json::json!({"enabled":true,"mode":"diagnostic_only_no_short_circuit","sampling_probability":1./4096.,
        "sampling_scope":"stable global output pixel hash; sampled invocation counts, not extrapolated queries",
        "classification_scope":"experimental margin classifier, not original bi_interval float-equivalence certificate",
        "matrix_columns":["original_miss","original_hit"],"matrix_rows":["clear","occluded","unknown"],
        "interrupted_scope":"excluded from original-result matrix; resumes do not rerun classifier",
        "original_work_scope":"all sampled pg_walk invocations including continuations; classifier matrix only new queries that finish without yielding","readback_bytes":COUNTER_BYTES,"counter_width_bits":64});
    for(scope,name)in ["world","shadow"].iter().enumerate(){
        let read=|field:usize|{let at=(scope*SCOPE_FIELDS+field)*8;let lo=u32::from_ne_bytes(bytes[at..at+4].try_into().unwrap());let hi=u32::from_ne_bytes(bytes[at+4..at+8].try_into().unwrap());u64::from(lo)|(u64::from(hi)<<32)};
        result[*name]=serde_json::json!({"classification":{"clear":read(0),"occluded":read(1),"unknown":read(2)},"nodes":read(3),"original_visited_nodes":read(13),"original_leaf_tests":read(14),
            "original_result_matrix":[[read(4),read(5)],[read(6),read(7)],[read(8),read(9)]],
            "interrupted":{"clear":read(10),"occluded":read(11),"unknown":read(12)}});
    }result
}

