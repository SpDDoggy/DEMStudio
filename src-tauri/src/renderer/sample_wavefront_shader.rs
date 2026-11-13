//! Isolated Fragment-stage generators; no runtime/export selection.
pub(super) struct Sources{pub vertex:String,pub fragment:String}
fn replace_once(s:&mut String,old:&str,new:&str)->Result<(),String>{
    if s.matches(old).count()!=1{return Err(format!("并发样本生成锚点数量变化: {old}"));}*s=s.replacen(old,new,1);Ok(())
}
pub(super) fn source(dense:&str,stage:u32)->Result<Sources,String>{
    adapt(super::raystate_shader::source_image_phase(dense,stage)?,stage)
}
/// Full original continuation state machine. Initialization remains the separate
/// triangle pass, so POINTS never reconstruct camera rays from its dummy varying.
pub(super) fn source_fused(dense:&str)->Result<Sources,String>{
    let mut s=super::raystate_shader::source_image_fragment(dense)?;
    let marker="if(!rs_batch_resume()){";
    if s.matches(marker).count()!=1{return Err("FUSED初始化锚点数量变化".into());}
    let start=s.find(marker).unwrap();let end=block_end(&s,start)?;
    s.replace_range(start..end,"");
    replace_once(&mut s,"bool resume=rs_batch_resume();","bool resume=true;")?;
    adapt(s,5)
}
fn adapt(mut s:String,stage:u32)->Result<Sources,String>{
    replace_once(&mut s,super::gpu_batch::FRAGMENT_HEADER,"")?;
    for old in["uniform vec4 dices[MAX_SEGMENTS];","uniform vec2 lobes[MAX_SEGMENTS];","uniform vec2 sun_r;","uniform float env_r;","uniform vec2 pixel_jitter;"]{replace_once(&mut s,old,"")?;}
    // Exact lexical array access adaptation, same lazy scalar table layout as
    // async_shader; reject nested indexes rather than guessing token boundaries.
    for(name,function)in[("dices","sw_dices"),("lobes","sw_lobes")]{
        let token=format!("{name}[");let mut result=String::new();let mut rest=s.as_str();
        while let Some(i)=rest.find(&token){result.push_str(&rest[..i]);let tail=&rest[i+token.len()..];let end=tail.find(']').ok_or("并发样本索引不完整")?;
            if tail[..end].contains('['){return Err("并发样本不支持嵌套采样数组索引".into());}
            result.push_str(&format!("{function}({})",&tail[..end]));rest=&tail[end+1..];}
        result.push_str(rest);s=result;
    }
    // Only continuation storage uses lane-expanded coordinates. All original
    // gl_FragCoord consumers, output_pixel and actual varying st stay physical.
    replace_once(&mut s,"ivec2 coord=ivec2(gl_FragCoord.xy);","ivec2 coord=ivec2(gl_FragCoord.xy)+ivec2(0,int(sw_lane)*sw_extent.y);")?;
    let producer="if(bounce==0){imageStore(rs_state_image,ivec3(ivec2(gl_FragCoord.xy),17),pg_s[3]);imageStore(rs_state_image,ivec3(ivec2(gl_FragCoord.xy),18),pg_s[4]);}";
    let world=stage==1||stage==5;
    if s.matches(producer).count()!=usize::from(world){return Err("并发样本primary producer数量变化".into());}
    s=s.replace(producer,"");
    // Shared immutable original sample-zero image. Never alias writable lane image.
    for anchor in ["pg_memo(pg_memo_a,0)","pg_memo(pg_memo_b,0)"]{if s.matches(anchor).count()!=usize::from(world){return Err("并发样本primary consumer数量变化".into());}}
    s=s.replace("pg_memo(pg_memo_a,0)","texelFetch(sw_primary_state,ivec3(ivec2(gl_FragCoord.xy),17),0)")
        .replace("pg_memo(pg_memo_b,0)","texelFetch(sw_primary_state,ivec3(ivec2(gl_FragCoord.xy),18),0)");
    if s.contains("imageStore(rs_state_image,ivec3(ivec2(gl_FragCoord.xy),17)")||s.contains("pg_memo(pg_memo_"){
        return Err("并发样本残留共享primary读写".into());}
    let header=format!(r#"
flat in uint sw_lane;
uniform ivec2 sw_extent;
uniform uint sw_base,sw_count,sw_stride,sw_target;
uniform sampler2DArray sw_primary_state;
layout(std430,binding=5) readonly buffer WaveSampleTable{{float sw_values[];}};
uint sw_sample(){{return sw_base+sw_lane;}}
float sw_value(uint offset){{return sw_values[sw_sample()*sw_stride+offset];}}
uint sw_segments(){{return (sw_stride-8u)/6u;}}
vec4 sw_dices(int i){{uint o=2u+4u*uint(i);return vec4(sw_value(o),sw_value(o+1u),sw_value(o+2u),sw_value(o+3u));}}
vec2 sw_lobes(int i){{uint o=2u+4u*sw_segments()+2u*uint(i);return vec2(sw_value(o),sw_value(o+1u));}}
vec2 sw_jitter(){{return vec2(sw_value(0u),sw_value(1u));}}
vec2 sw_sun(){{uint o=2u+6u*sw_segments();return vec2(sw_value(o),sw_value(o+1u));}}
#define pixel_jitter sw_jitter()
#define sun_r sw_sun()
#define env_r sw_value(4u+6u*sw_segments())
bool rs_batch_enabled(){{return sw_base>0u&&sw_count<=16u&&sw_lane<sw_count&&sw_target<=256u&&sw_sample()<sw_target;}}
bool rs_batch_resume(){{return {};}}
"#,if stage==0{"false"}else{"true"});
    let insertion=s.lines().take_while(|l|l.starts_with("#version")||l.starts_with("#extension")).map(|l|l.len()+1).sum::<usize>();s.insert_str(insertion,&header);
    let vertex=if stage==0{r#"#version 330 compatibility
#extension GL_ARB_shader_storage_buffer_object : require
layout(std430,binding=5) readonly buffer WaveSampleTable{float sw_values[];};
uniform uint sw_draw_lane,sw_base,sw_stride;
uniform vec2 aspect;
flat out uint sw_lane;
out vec2 st;
void main(){sw_lane=sw_draw_lane;uint offset=(sw_base+sw_lane)*sw_stride;
vec2 pixel_jitter=vec2(sw_values[offset],sw_values[offset+1u]);
vec2 p=vec2((gl_VertexID<<1)&2,gl_VertexID&2);gl_Position=vec4(p*2.-1.,0.,1.);
st=(p*2.-1.)*aspect+pixel_jitter;}
"#}else{r#"#version 330 compatibility
#extension GL_ARB_shader_storage_buffer_object : require
layout(std430,binding=1) readonly buffer WavePhaseQueue{uint pq_words[];};
uniform ivec2 sw_extent;
uniform uint pq_index_base;
flat out uint sw_lane;
out vec2 st;
void main(){uint path=pq_words[pq_index_base+uint(gl_VertexID)];uint total=uint(sw_extent.x)*uint(sw_extent.y);
sw_lane=path/total;uint pixel=path%total;
vec2 center=vec2(pixel%uint(sw_extent.x),pixel/uint(sw_extent.x))+.5;
vec2 ndc=center/vec2(sw_extent)*2.-1.;gl_Position=vec4(ndc,0.,1.);gl_PointSize=1.;st=vec2(0.);}
"#};
    Ok(Sources{vertex:vertex.into(),fragment:s})
}
/// Two-kernel candidate: stop BEFORE WORLD, keeping original OTHER arithmetic,
/// original transition cap and the common saved-jitter tail.
pub(super) fn source_other(dense:&str)->Result<Sources,String>{
    let mut s=super::raystate_shader::source_image_fragment(dense)?;
    let marker="if(phase==RS_WORLD){";
    if s.matches(marker).count()!=1{return Err("OTHER WORLD锚点数量改变".into());}
    let begin=s.find(marker).unwrap();let end=block_end(&s,begin)?;
    s.replace_range(begin..end,"if(phase==RS_WORLD)break;");
    replace_once(&mut s,"rs_run();","rs_run();if(pg_s[5].y==0.)pg_s[5].y=2.;")?;
    // Full mutable state is intentional here: shadow may yield with cursor and
    // conditional radiance unchanged; no phase-union optimization in this trial.
    let count=s.lines().find_map(|line|line.trim().strip_prefix("#define RS_MUTABLE_LAYERS ")?.parse::<u32>().ok()).filter(|n|[15,17].contains(n)).ok_or("OTHER状态层声明无效")?;
    for(load,expression)in[(true,"pg_s[layer]=resume?imageLoad(rs_state_image,ivec3(coord,layer)):vec4(0.);"),(false,"imageStore(rs_state_image,ivec3(coord,layer),pg_s[layer]);")]{
        let token="for(int layer=0;layer<RS_MUTABLE_LAYERS;++layer)";
        let mut found=None;
        for (index,_)in s.match_indices(token){let body_start=index+token.len();let rest=s[body_start..].trim_start();if rest.starts_with(expression){if found.is_some(){return Err("OTHER状态复制锚点重复".into());}found=Some((index,s.len()-rest.len()+expression.len()));}}
        if let Some((start,end))=found{let mut text=String::from("{\n");for layer in 0..count{text.push_str(&expression.replace("layer",&layer.to_string()));text.push('\n');}text.push('}');s.replace_range(start..end,&text);}
        else{for layer in 0..count{if !s.contains(&expression.replace("layer",&layer.to_string())){return Err(format!("OTHER{}状态复制缺失",if load{"load"}else{"store"}));}}}
    }
    adapt(s,2)
}
/// Lexical balanced block, ignoring braces in comments. No regex rewrite.
fn block_end(s:&str,start:usize)->Result<usize,String>{
    let bytes=s.as_bytes();let open=s[start..].find('{').map(|i|i+start).ok_or("OTHER块缺少括号")?;
    let mut i=open;let mut depth=0usize;
    while i<bytes.len(){
        if bytes[i..].starts_with(b"//"){while i<bytes.len()&&bytes[i]!=b'\n'{i+=1;}continue;}
        if bytes[i..].starts_with(b"/*"){i+=2;while i+1<bytes.len()&&!bytes[i..].starts_with(b"*/"){i+=1;}i+=2;continue;}
        if bytes[i]==b'{'{depth+=1;}else if bytes[i]==b'}'{depth-=1;if depth==0{return Ok(i+1);}}i+=1;
    }Err("OTHER块括号不完整".into())
}

