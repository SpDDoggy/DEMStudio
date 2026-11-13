//! Opt-in traversal counters, sampled by stable global pixel hash. Binding 7.
use glow::HasContext;
const FIELDS:[&str;6]=["new_queries","continuations","visited_nodes","leaf_tests","page_fault_yields","budget_yields"];
const BASE_BYTES:usize=2*6*8;
const BYTES:usize=BASE_BYTES+4*2*8;
const CAPTURE_CAPACITY:usize=2048;
const CAPTURE_WORDS:usize=52;
const CAPTURE_HEADER:usize=8;
const WORK_FIELDS:[&str;11]=["initializations","world_phase_entries","surface_phase_entries","shadow_phase_entries","scatter_phase_entries","matte_sun_entries","matte_sky_entries","primary_memo_hits","fused_committed_samples","fragment_invocations","rs_run_calls"];
const WORK_BYTES:usize=11*8;
pub(super) fn work_enabled()->bool{enabled()&&std::env::var("DEMSTUDIO_INTEGRATION_WORK").as_deref()==Ok("on")}
fn work_offset(capture:bool)->usize{BYTES+if capture{(CAPTURE_HEADER+CAPTURE_CAPACITY*CAPTURE_WORDS)*4}else{0}}

pub(super) fn capture_enabled()->bool{enabled()&&std::env::var("DEMSTUDIO_WALK_CAPTURE").as_deref()==Ok("on")}
pub(super) fn diagnostic_bytes()->u64{(work_offset(capture_enabled())+if work_enabled(){WORK_BYTES}else{0})as u64}
pub(super) fn clock_enabled()->bool{std::env::var("DEMSTUDIO_WALK_CLOCK").as_deref()==Ok("on")}
pub(super) fn enabled()->bool{std::env::var("DEMSTUDIO_WALK_PROFILE").as_deref()==Ok("on")}
pub(super) const SHADER_HEADER:&str=r#"
#define WP_PROFILE
layout(std430,binding=7) buffer WalkProfile {uint wp_words[];};
bool wp_sampled(){uvec2 p=uvec2(output_pixel());uint h=p.x*0x9e3779b9u+p.y*0x85ebca6bu;h^=h>>16;h*=0x7feb352du;h^=h>>15;return (h&255u)==0u;}
void wp_flush(bool visibility,inout uint counts[6]){
    uint base=visibility?12u:0u;
    for(uint i=0u;i<6u;i++){uint value=counts[i];if(value!=0u){uint at=base+i*2u;uint old=atomicAdd(wp_words[at],value);if(old>0xffffffffu-value)atomicAdd(wp_words[at+1u],1u);}counts[i]=0u;}
}
"#;
pub(super) fn inject_header(shader:&mut String)->Result<(),String>{
    let at=shader.find("#ifdef WP_PROFILE").ok_or("遍历诊断缺少遍历入口")?;
    shader.insert_str(at,SHADER_HEADER);
    if clock_enabled(){inject_clock(shader)?;}
    if capture_enabled(){let at=shader.find("#ifdef WP_PROFILE").ok_or("查询捕获缺少遍历入口")?;shader.insert_str(at,CAPTURE_SHADER);}
    
    if work_enabled(){let at=shader.find("#ifdef WP_PROFILE").ok_or("积分计数缺少遍历入口")?;let header=WORK_SHADER.replace("IW_BASE",&(work_offset(capture_enabled())/4).to_string());shader.insert_str(at,&header);}
    Ok(())
}
const CLOCK_HEADER:&str=r#"
#define WP_CLOCK
bool wpc_sampled(){uvec2 p=uvec2(output_pixel());uint h=p.x*0x9e3779b9u+p.y*0x85ebca6bu;h^=h>>16;h*=0x7feb352du;h^=h>>15;return (h&4095u)==0u;}
uvec2 wpc_delta(uvec2 end,uvec2 start){return uvec2(end.x-start.x,end.y-start.y-uint(end.x<start.x));}
void wpc_add64(uint at,uvec2 value){uint old=atomicAdd(wp_words[at],value.x);atomicAdd(wp_words[at+1u],value.y+uint(old>0xffffffffu-value.x));}
void wpc_record(uint category,uvec2 start,uvec2 end){uint at=24u+category*4u;wpc_add64(at,wpc_delta(end,start));wpc_add64(at+2u,uvec2(1u,0u));}
"#;
pub(super) fn inject_clock(shader:&mut String)->Result<(),String>{
    if !shader.contains("#define WP_PROFILE"){return Err("shader clock需要WalkProfile".into());}
    *shader=shader.replacen("#version 330 compatibility","#version 330 compatibility\n#extension GL_ARB_shader_clock : require",1);
    let at=shader.find("#ifdef WP_PROFILE").ok_or("shader clock缺少遍历入口")?;
    shader.insert_str(at,CLOCK_HEADER);
    let original="init_rand();gen_light_samples();";
    let run=shader.find("void rs_run(){").ok_or("shader clock缺少rs_run入口")?;
    let init=run+shader[run..].find(original).ok_or("shader clock缺少rs_run光源初始化入口")?;
    let replacement=r#"
#ifdef WP_CLOCK
    bool wpc_light_selected=wpc_sampled();uvec2 wpc_light_start=uvec2(0u);
    if(wpc_light_selected)wpc_light_start=clock2x32ARB();
#endif
    init_rand();gen_light_samples();
#ifdef WP_CLOCK
    if(wpc_light_selected){uvec2 wpc_light_end=clock2x32ARB();wpc_record(0u,wpc_light_start,wpc_light_end);
        uvec2 wpc_empty_start=clock2x32ARB();uvec2 wpc_empty_end=clock2x32ARB();wpc_record(3u,wpc_empty_start,wpc_empty_end);}
#endif
"#;
    shader.replace_range(init..init+original.len(),replacement);Ok(())
}
const CAPTURE_SHADER:&str=r#"
#define WP_CAPTURE
// Header at word40: accepted(saturating), reserved, attempts u64, overflow u64.
void wpcap_add(uint at){uint old=atomicAdd(wp_words[at],1u);if(old==0xffffffffu)atomicAdd(wp_words[at+1u],1u);}
void wpcap_float(uint at,float v){wp_words[at]=floatBitsToUint(v);}
int wpcap_begin(vec3 o,vec3 dir,vec2 interval,bool visibility){
    wpcap_add(42u);uint index=atomicAdd(wp_words[40],0u);
    for(;;){if(index>=2048u){wpcap_add(44u);return -1;}
        uint old=atomicCompSwap(wp_words[40],index,index+1u);if(old==index)break;index=old;}
    uint base=48u+index*52u;
    for(uint i=0u;i<3u;i++){wpcap_float(base+i,o[i]);wpcap_float(base+3u+i,dir[i]);}
    wpcap_float(base+6u,interval.x);wpcap_float(base+7u,interval.y);
    wp_words[base+8u]=visibility?1u:0u;wp_words[base+9u]=pg_s[2].w>.5?1u:0u;
    uvec2 pixel=uvec2(output_pixel());wp_words[base+23u]=pixel.x;wp_words[base+24u]=pixel.y;
    for(uint layer=0u;layer<3u;layer++)for(uint c=0u;c<4u;c++)wpcap_float(base+25u+layer*4u+c,pg_s[layer][c]);
    return int(index);
}
void wpcap_end(int index,bool complete,bool yielded,float distance,vec3 normal,vec2 id,float att,uint nodes,uint leaves){
    if(index<0)return;uint base=48u+uint(index)*52u;
    wp_words[base+10u]=complete?1u:0u;wp_words[base+11u]=yielded?1u:0u;
    wpcap_float(base+12u,distance);for(uint i=0u;i<3u;i++)wpcap_float(base+13u+i,normal[i]);
    wpcap_float(base+16u,id.x);wpcap_float(base+17u,id.y);wpcap_float(base+18u,att);
    wp_words[base+19u]=nodes;wp_words[base+20u]=leaves;
    wpcap_float(base+21u,pg_s[5].x);wpcap_float(base+22u,pg_s[5].y);
    for(uint layer=0u;layer<3u;layer++)for(uint c=0u;c<4u;c++)wpcap_float(base+37u+layer*4u+c,pg_s[layer][c]);
}
"#;
const WORK_SHADER:&str=r#"
#define IW_WORK
bool iw_selected=false;
uint iw_counts[11];
void iw_begin(){iw_selected=wp_sampled();for(int i=0;i<11;i++)iw_counts[i]=0u;if(iw_selected)iw_counts[9]=1u;}
void iw_flush(){if(!iw_selected)return;for(uint i=0u;i<11u;i++){uint value=iw_counts[i];if(value>0u){uint at=IW_BASEu+i*2u;uint old=atomicAdd(wp_words[at],value);if(old>0xffffffffu-value)atomicAdd(wp_words[at+1u],1u);}iw_counts[i]=0u;}}
#define IW_COUNT(i) if(iw_selected)iw_counts[i]++
#define IW_BEGIN() iw_begin()
#define IW_FLUSH() iw_flush()
"#;
pub(super) struct WalkProfile{buffer:glow::Buffer,clock:bool,capture:bool,work:bool}
impl WalkProfile{
 pub unsafe fn create(gl:&glow::Context)->Result<Self,String>{
    if gl.get_parameter_i32(glow::MAX_SHADER_STORAGE_BUFFER_BINDINGS)<8{return Err("遍历诊断需要SSBO binding7".into());}
    let buffer=gl.create_buffer()?;gl.bind_buffer(glow::SHADER_STORAGE_BUFFER,Some(buffer));
    gl.buffer_data_u8_slice(glow::SHADER_STORAGE_BUFFER,&vec![0u8;diagnostic_bytes()as usize],glow::DYNAMIC_READ);
    let error=gl.get_error();gl.bind_buffer(glow::SHADER_STORAGE_BUFFER,None);
    if error!=glow::NO_ERROR{gl.delete_buffer(buffer);return Err(format!("遍历诊断分配失败: GL {error:#x}"));}
    let value=Self{buffer,clock:clock_enabled(),capture:capture_enabled(),work:work_enabled()};value.bind(gl);Ok(value)
 }
 pub unsafe fn bind(&self,gl:&glow::Context){gl.bind_buffer_base(glow::SHADER_STORAGE_BUFFER,7,Some(self.buffer));}
 pub unsafe fn final_read(&self,gl:&glow::Context)->serde_json::Value{
    gl.memory_barrier(glow::SHADER_STORAGE_BARRIER_BIT|glow::BUFFER_UPDATE_BARRIER_BIT);
    let mut bytes=vec![0u8;work_offset(self.capture)+if self.work{WORK_BYTES}else{0}];gl.bind_buffer(glow::SHADER_STORAGE_BUFFER,Some(self.buffer));gl.get_buffer_sub_data(glow::SHADER_STORAGE_BUFFER,0,&mut bytes);gl.bind_buffer(glow::SHADER_STORAGE_BUFFER,None);
    let mut result=decode(bytes[..BYTES].try_into().unwrap(),self.clock);
    if self.capture{result["capture"]=decode_capture(&bytes[..work_offset(true)]);}
    if self.work{result["integration_work"]=decode_work(&bytes[work_offset(self.capture)..]);}
    result["readback_bytes"]=serde_json::json!(bytes.len());result
 }
 pub unsafe fn destroy(self,gl:&glow::Context){gl.bind_buffer_base(glow::SHADER_STORAGE_BUFFER,7,None);gl.delete_buffer(self.buffer);}
}
fn decode(bytes:&[u8;BYTES],clock:bool)->serde_json::Value{
 let mut result=serde_json::json!({"enabled":true,"diagnostic_only":true,"sampling_probability":1./256.,"sampling_scope":"stable hash of global output pixel; all samples and continuations of selected pixels","counter_scope":"sampled raw counts, not extrapolated totals","visited_nodes_scope":"page-ready evaluated nodes after iteration-budget check","leaf_tests_scope":"actual primitive calls after height gate","readback_bytes":BYTES});
 for(category,name)in ["world","shadow"].iter().enumerate(){let mut counters=serde_json::Map::new();let mut values=[0u64;6];for(i,field)in FIELDS.iter().enumerate(){let at=(category*6+i)*8;values[i]=u64::from_ne_bytes(bytes[at..at+8].try_into().unwrap());counters.insert((*field).into(),serde_json::json!(values[i]));}
 counters.insert("selected_pixel_query_invocations".into(),serde_json::json!(values[0]+values[1]));result[*name]=counters.into();}
 result["clock"]=serde_json::json!({"enabled":clock,"sampling_probability":1./4096.,"sampling_scope":"independent stable global pixel hash; all samples and continuation invocations of selected pixels","units":"shader-local latency ticks, not GPU seconds","parallel_intervals_are_not_additive":true,"frequency_is_not_guaranteed_constant":true,"empty_pair_is_diagnostic_not_subtracted":true});
 if clock{for(i,name)in ["gen_light","world","shadow","empty_pair"].iter().enumerate(){let at=BASE_BYTES+i*16;let ticks=u64::from_ne_bytes(bytes[at..at+8].try_into().unwrap());let count=u64::from_ne_bytes(bytes[at+8..at+16].try_into().unwrap());result["clock"][*name]=serde_json::json!({"latency_ticks":ticks,"measured_invocations":count,"mean_latency_ticks":if count>0{Some(ticks as f64/count as f64)}else{None}});}}
 result
}
fn decode_capture(bytes:&[u8])->serde_json::Value{
    let expected=BYTES+(CAPTURE_HEADER+CAPTURE_CAPACITY*CAPTURE_WORDS)*4;
    if bytes.len()!=expected{return serde_json::json!({"enabled":true,"valid":false,"error":"capture payload size mismatch"});}
    let word=|i:usize|u32::from_ne_bytes(bytes[i*4..i*4+4].try_into().unwrap());
    let number=|i:usize|{let v=f32::from_bits(word(i));if v.is_finite(){serde_json::json!(v)}else{serde_json::Value::Null}};
    let count=(word(40)as usize).min(CAPTURE_CAPACITY);let mut records=Vec::with_capacity(count);
    for index in 0..count{let b=48+index*CAPTURE_WORDS;let floats=|start:usize,n:usize|(0..n).map(|i|number(b+start+i)).collect::<Vec<_>>();
        records.push(serde_json::json!({"origin":floats(0,3),"direction":floats(3,3),"interval":floats(6,2),"visibility":word(b+8)!=0,"start_resume":word(b+9)!=0,"completed":word(b+10)!=0,"yielded":word(b+11)!=0,"distance":number(b+12),"normal":floats(13,3),"id":floats(16,2),"att":number(b+18),"visited_nodes":word(b+19),"leaf_tests":word(b+20),"end_page":number(b+21),"end_status":number(b+22),"output_pixel":[word(b+23),word(b+24)],"start_cursor_float_bits":(25..37).map(|i|word(b+i)).collect::<Vec<_>>(),"end_cursor_float_bits":(37..49).map(|i|word(b+i)).collect::<Vec<_>>(),"output_float_bits":(12..19).map(|i|word(b+i)).collect::<Vec<_>>(),"input_float_bits":(0..8).map(|i|word(b+i)).collect::<Vec<_>>()}));
    }
    serde_json::json!({"enabled":true,"valid":word(40)as usize<=CAPTURE_CAPACITY,"capacity":CAPTURE_CAPACITY,"record_bytes":CAPTURE_WORDS*4,"additional_buffer_bytes":expected-BYTES,"attempted_invocations":u64::from(word(42))+(u64::from(word(43))<<32),"overflow":u64::from(word(44))+(u64::from(word(45))<<32),"scope":"one pg_walk invocation; resumed calls are separate records, not full queries; only wp_sampled pixels","selection_bias":"first 2048 accepted selected invocations in GPU atomic order; truncated and not representative of full export","test_pixel_override":std::env::var("DEMSTUDIO_WALK_CAPTURE_PIXEL").ok(),"nonfinite_float_json":"null; exact input/cursor bit arrays preserve replay data","records":records})
}
fn decode_work(bytes:&[u8])->serde_json::Value{
    if bytes.len()!=WORK_BYTES{return serde_json::json!({"enabled":true,"valid":false});}
    let mut result=serde_json::json!({"enabled":true,"valid":true,"sampling_probability":1./256.,"scope":"async image Fragment invocations after preserved sample zero; stable global pixel hash, raw sampled totals, not extrapolated; phase entries include retries/yields; commit counter covers fused path only","additional_buffer_bytes":WORK_BYTES});
    for(i,name)in WORK_FIELDS.iter().enumerate(){result[*name]=serde_json::json!(u64::from_ne_bytes(bytes[i*8..i*8+8].try_into().unwrap()));}result
}



