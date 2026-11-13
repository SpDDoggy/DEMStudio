// Experimental independent pixel sampling; the default generator is unchanged.
pub(super) fn source_async_samples(dense:&str)->Result<String,String>{
    let fused=std::env::var("DEMSTUDIO_ASYNC_FUSED_COMMIT").as_deref()==Ok("on");
    let mut shader=source_async_samples_flags(dense,fused,
        std::env::var("DEMSTUDIO_ASYNC_STATIC_STATE").as_deref()==Ok("on"))?;
    if std::env::var("DEMSTUDIO_ASYNC_MATTE_STATE_ELISION").as_deref()==Ok("on"){
        if !fused||["DEMSTUDIO_STATE_DUMP","DEMSTUDIO_TRACE_INITIAL_RAY","DEMSTUDIO_TRACE_CAMERA_INPUT"].iter().any(|n|std::env::var_os(n).is_some()){
            return Err("接影状态省略要求融合独立采样且不能启用完整状态转储".into());
        }
        shader=elide_matte_state(expand_async_state_loops(&shader)?)?;
    }
    if std::env::var("DEMSTUDIO_ASYNC_ELIDE_COMPLETED_STORE").as_deref()==Ok("on") {
        if !fused||std::env::var("DEMSTUDIO_ASYNC_PHASE_PAIR").as_deref()==Ok("on") {
            return Err("完成样本写回消除仅支持融合独立采样，不能与阶段对同时启用".into());
        }
        elide_completed_stores(shader)
    }else{Ok(shader)}
}
// Ordinary paths never enter MATTE after initialization. Keep its private layers
// in the image, rather than carrying unused values through every traversal.
// Fresh MATTE initialization still writes them before a yield; resumed MATTE
// loads both. DONE consumers use completion/radiance, not these retired layers.
fn elide_matte_state(mut shader:String)->Result<String,String>{
    if !shader.contains("#define RS_MUTABLE_LAYERS 17"){return Ok(shader);}
    let entry=shader.rfind("void main(){").ok_or("接影状态省略缺少入口")?;
    let mut main=shader[entry..].to_owned();
    for layer in [15,16]{
        let load=format!("pg_s[{layer}]=resume?imageLoad(rs_state_image,ivec3(coord,{layer})):vec4(0.);");
        let store=format!("imageStore(rs_state_image,ivec3(coord,{layer}),pg_s[{layer}]);");
        if main.matches(&load).count()!=1||main.matches(&store).count()!=1{return Err("接影状态省略复制锚点变化".into());}
        main=main.replace(&load,&format!("pg_s[{layer}]=(resume&&pg_s[7].w>=float(RS_MATTE_SUN)&&pg_s[7].w<=float(RS_MATTE_SKY))?imageLoad(rs_state_image,ivec3(coord,{layer})):vec4(0.);"));
        main=main.replace(&store,&format!("if(pg_s[7].w>=float(RS_MATTE_SUN)&&pg_s[7].w<=float(RS_MATTE_SKY)){{{store}}}"));
    }
    shader.truncate(entry);shader.push_str(&main);Ok(shader)
}
// Runtime owns the explicit default-off selection and draws Fresh before Resume.
pub(super) fn source_async_fresh_split(dense:&str)->Result<[String;2],String>{
    if std::env::var("DEMSTUDIO_ASYNC_FUSED_COMMIT").as_deref()!=Ok("on")
        ||["DEMSTUDIO_ASYNC_PHASE_PAIR","DEMSTUDIO_ASYNC_ACTIVE_QUEUE","DEMSTUDIO_PHASE_QUEUE","DEMSTUDIO_PHASE_COMPUTE"].iter().any(|name|std::env::var(name).as_deref()==Ok("on"))
        ||std::env::var("DEMSTUDIO_SAMPLE_WAVEFRONT").as_deref()==Ok("4"){
        return Err("FreshSplit要求融合独立采样且不能与阶段对、队列或多lane组合".into());
    }
    split_fused_fresh_source(&source_async_samples(dense)?)
}
fn split_fused_fresh_source(source:&str)->Result<[String;2],String>{
    let init="as_was_initialized=as_initialized();as_begin_initialized();";
    let entry=source.rfind("void main(){").ok_or("FreshSplit缺少main")?;
    if source[entry..].matches(init).count()!=1{return Err("FreshSplit缺少唯一初始化门控".into());}
    let mut fresh=source.to_owned();
    let resume_fn="bool rs_batch_resume(){return as_was_initialized;}";
    if fresh.matches(resume_fn).count()!=1{return Err("FreshSplit缺少唯一resume函数".into());}
    fresh=fresh.replace(resume_fn,"bool rs_batch_resume(){return false;}");
    fresh=fresh.replace(init,"as_was_initialized=as_initialized();if(as_was_initialized){IW_FLUSH();discard;}as_begin_initialized();");
    let fresh_entry=fresh.rfind("void main(){").unwrap();
    let gate=fresh[fresh_entry..].find("if(resume){").map(|i|fresh_entry+i).ok_or("FreshSplit缺少resume状态分支")?;
    let end={let mut depth=0;let mut end=None;for(i,b)in fresh.as_bytes().iter().enumerate().skip(gate+"if(resume)".len()){if *b==b'{'{depth+=1;}else if *b==b'}'{depth-=1;if depth==0{end=Some(i+1);break;}}}end.ok_or("FreshSplit状态分支未闭合")?};
    fresh.replace_range(gate..end,"");
    // The only remaining image loads in main are mutable-state initialization.
    // ElideCompletedStore may already have changed layer 5 to literal zero.
    let main=fresh.rfind("void main(){").unwrap();let mut body=fresh[main..].to_owned();
    let dynamic="resume?imageLoad(rs_state_image,ivec3(coord,layer)):vec4(0.)";
    body=body.replace(dynamic,"vec4(0.)");
    for layer in 0..17{body=body.replace(&format!("resume?imageLoad(rs_state_image,ivec3(coord,{layer})):vec4(0.)"),"vec4(0.)");}
    if body.contains("imageLoad("){return Err("FreshSplit存在未识别的fresh状态载入".into());}
    fresh.truncate(main);fresh.push_str(&body);
    let mut resume=source.to_owned();resume=resume.replace(init,"as_was_initialized=as_initialized();if(!as_was_initialized){IW_FLUSH();discard;}as_begin_initialized();");
    Ok([fresh,resume])
}
// Apply after static expansion so both generated state-copy forms are supported.
fn elide_completed_stores(mut shader:String)->Result<String,String>{
    if !shader.lines().any(|line|line.trim()=="#define RS_DONE_MINIMAL_STORE") {
        return Err("完成样本写回消除要求最小 DONE 写回；完整状态诊断不能启用".into());
    }
    let done=r#"    if(pg_s[5].y>3.5){
        imageStore(rs_state_image,ivec3(coord,5),pg_s[5]);
        imageStore(rs_state_image,ivec3(coord,6),pg_s[6]);
    }else"#;
    if shader.matches(done).count()!=1{return Err("完成样本写回消除缺少唯一 DONE 分支".into());}
    shader=shader.replace(done,r#"    if(pg_s[5].y>3.5){
        // Non-final radiance is consumed locally by as_emit_sample; the next
        // sample initializes all mutable state. Final state remains observable.
        if(rs_sample_index+1u>=as_target){
            imageStore(rs_state_image,ivec3(coord,5),pg_s[5]);
            imageStore(rs_state_image,ivec3(coord,6),pg_s[6]);
        }
    }else"#);
    let dynamic="pg_s[layer]=resume?imageLoad(rs_state_image,ivec3(coord,layer)):vec4(0.);";
    let constant="pg_s[5]=resume?imageLoad(rs_state_image,ivec3(coord,5)):vec4(0.);";
    if shader.matches(dynamic).count()==1{
        shader=shader.replace(dynamic,"pg_s[layer]=layer==5?vec4(0.):(resume?imageLoad(rs_state_image,ivec3(coord,layer)):vec4(0.));");
    }else if shader.matches(constant).count()==1{
        shader=shader.replace(constant,"pg_s[5]=vec4(0.);");
    }else{return Err("完成样本写回消除缺少唯一状态载入".into());}
    Ok(shader)
}

fn source_async_samples_flags(dense:&str,fused:bool,static_state:bool)->Result<String,String>{
    let mut shader=source_image_fragment(dense)?;
    if !shader.contains(crate::renderer::gpu_batch::FRAGMENT_HEADER){return Err("独立采样缺少批控制入口".into());}
    shader=shader.replace(crate::renderer::gpu_batch::FRAGMENT_HEADER,"");
    shader=shader.replace("uniform vec4 dices[MAX_SEGMENTS];","")
        .replace("uniform vec2 lobes[MAX_SEGMENTS];","")
        .replace("uniform vec2 sun_r;","")
        .replace("uniform float env_r;","")
        .replace("uniform vec2 pixel_jitter;","");
    // Indexed reads remain lazy; no per-invocation array copy is introduced.
    for(name,fun)in[("dices","as_dices"),("lobes","as_lobes")]{
        let mut out=String::new();let mut rest=shader.as_str();let prefix=format!("{name}[");
        while let Some(start)=rest.find(&prefix){
            out.push_str(&rest[..start]);let tail=&rest[start+prefix.len()..];
            let end=tail.find(']').ok_or("独立采样数组索引缺少结束括号")?;
            out.push_str(&format!("{fun}({})",&tail[..end]));rest=&tail[end+1..];
        }
        out.push_str(rest);shader=out;
    }
    let header=format!("{}\n{}",crate::renderer::async_samples::SHADER_HEADER,r#"
layout(std430,binding=5) readonly buffer AsyncSampleTable{float as_values[];};
uint rs_sample_index;bool as_was_initialized;
uint as_segments(){return (as_stride-8u)/6u;}
float as_value(uint offset){return as_values[rs_sample_index*as_stride+offset];}
vec4 as_dices(int index){uint o=2u+uint(index)*4u;return vec4(as_value(o),as_value(o+1u),as_value(o+2u),as_value(o+3u));}
vec2 as_lobes(int index){uint o=2u+4u*as_segments()+uint(index)*2u;return vec2(as_value(o),as_value(o+1u));}
vec2 as_jitter(){return vec2(as_value(0u),as_value(1u));}
vec2 as_sun(){uint o=2u+6u*as_segments();return vec2(as_value(o),as_value(o+1u));}
#define pixel_jitter as_jitter()
#define sun_r as_sun()
#define env_r as_value(4u+6u*as_segments())
bool rs_batch_enabled(){return rs_sample_index<as_target;}
bool rs_batch_resume(){return as_was_initialized;}
"#);
    // Extensions must precede SSBO declarations; header precedes every consumer.
    let insertion=shader.lines().take_while(|l|l.starts_with("#version")||l.starts_with("#extension")).map(|l|l.len()+1).sum::<usize>();
    shader.insert_str(insertion,&header);
    
    let entry=shader.rfind("void main(){").ok_or("独立采样缺少片元入口")?;
    if fused {
        // Only the image entry changes. rs_run and all primitive expressions keep
        // the original phase/iteration boundaries and sample RNG accessors.
        shader.truncate(entry);
        shader.push_str(r#"
layout(location=0,index=0) out vec4 as_color;
layout(location=0,index=1) out vec4 as_factor;
void as_emit_sample(vec4 value){
    as_color=value;
    as_factor=vec4(0.,0.,0.,as_value(as_alpha_offset));
    uint next=rs_sample_index+1u;
    as_words[4u+2u*as_pixel_index()]=next;
    as_words[5u+2u*as_pixel_index()]=0u;
    if(next<as_target)imageStore(rs_state_image,ivec3(ivec2(gl_FragCoord.xy),5),vec4(-1.,2.,0.,0.));
}
void main(){
    rs_sample_index=as_cursor();if(rs_sample_index>=as_target)discard;
    as_was_initialized=as_initialized();as_begin_initialized();
    ivec2 coord=ivec2(gl_FragCoord.xy);
    bool resume=rs_batch_resume();
    if(resume){
        vec4 status=imageLoad(rs_state_image,ivec3(coord,5));
        if(status.y>3.5){as_emit_sample(imageLoad(rs_state_image,ivec3(coord,6)));return;}
        bool waiting=status.y>.5&&status.y<1.5;
        if(waiting&&pg_page_slot(int(status.x))<0)discard;
    }
    for(int layer=0;layer<RS_MUTABLE_LAYERS;++layer)
        pg_s[layer]=resume?imageLoad(rs_state_image,ivec3(coord,layer)):vec4(0.);
    pg_s[5]=vec4(-1.,0.,0.,0.);rs_run();
#ifdef BC_PROFILE_COUNTERS
    bc_profile_flush();
#endif
#ifdef RS_DONE_MINIMAL_STORE
    if(pg_s[5].y>3.5){
        imageStore(rs_state_image,ivec3(coord,5),pg_s[5]);
        imageStore(rs_state_image,ivec3(coord,6),pg_s[6]);
    }else
#endif
    {
        for(int layer=0;layer<RS_MUTABLE_LAYERS;++layer)
            imageStore(rs_state_image,ivec3(coord,layer),pg_s[layer]);
    }
    if(pg_s[5].y>3.5){as_emit_sample(pg_s[6]);return;}
    // Discard suppresses framebuffer output, not preceding image/SSBO stores.
    discard;
}
"#);
    }else{
        shader.insert_str(entry+"void main(){".len(),"\nrs_sample_index=as_cursor();if(rs_sample_index>=as_target)return;as_was_initialized=as_initialized();as_begin_initialized();\n");
    }
    // One local counter set per invocation. Flush on every terminal return or
    // discard, including already-DONE, waiting, and completed fused commits.
    if crate::renderer::walk_profile::work_enabled(){
    inject_integration_work_main(&mut shader)?;
    }
    if static_state || std::env::var("DEMSTUDIO_IMAGE_STATIC_STATE").as_deref()==Ok("on") {shader=expand_async_state_loops(&shader)?;}
    Ok(shader)
}

fn inject_integration_work_main(shader:&mut String)->Result<(),String>{
    *shader=shader.replace("    as_color=value;","    IW_COUNT(8);as_color=value;");
    let entry=shader.rfind("void main(){").ok_or("积分计数缺少async入口")?;
    let mut main=shader[entry..].replace("void main(){","void main(){IW_BEGIN();");
    main=main.replace("rs_run();","IW_COUNT(10);rs_run();").replace("return;","{IW_FLUSH();return;}").replace("discard;","{IW_FLUSH();discard;}");
    shader.truncate(entry);shader.push_str(&main);
    Ok(())
}

/// Continuous one-path-per-pixel WORLD/OTHER pair. Original sample zero is
/// external; OTHER alone emits one original dual-source blend per completion.
pub(super) fn source_async_phase_pair(dense:&str)->Result<[String;2],String>{
    if std::env::var("DEMSTUDIO_ASYNC_ELIDE_COMPLETED_STORE").as_deref()==Ok("on"){return Err("阶段对不能启用完成样本写回消除".into());}
    let mut world=source_async_samples_flags(dense,false,false)?;
    let run_range=phase_named_block(&world,"void rs_run(){")?;
    let run=&world[run_range.0..run_range.1];
    let loop_start=run.find("for(int transitions=0;").ok_or("AsyncPair缺少阶段循环")?;
    let loop_range=phase_named_block(&run[loop_start..],"for(int transitions=0;")?;
    let loop_end=loop_start+loop_range.1;let phase_loop=&run[loop_start..loop_end];
    let selected=phase_named_block(phase_loop,"if(phase==RS_WORLD){")?;
    let loop_open=phase_loop.find('{').ok_or("AsyncPair阶段括号缺失")?;
    let body=format!("{}for(int transitions=0;transitions<1;++transitions){{{}{}\n}}{}",&run[..loop_start],&phase_loop[loop_open+1..selected.0],&phase_loop[selected.0..selected.1],&run[loop_end..]);
    world.replace_range(run_range.0..run_range.1,&body);
    let producer="if(bounce==0){imageStore(rs_state_image,ivec3(ivec2(gl_FragCoord.xy),17),pg_s[3]);imageStore(rs_state_image,ivec3(ivec2(gl_FragCoord.xy),18),pg_s[4]);}";
    if world.matches(producer).count()!=1{return Err("AsyncPair primary producer锚点变化".into());}world=world.replace(producer,"");
    let entry=world.rfind("void main(){").ok_or("AsyncPair WORLD缺少main")?;
    let mut main=world[entry..].to_owned();
    let waiting="bool waiting=status.y>.5&&status.y<1.5;";
    if main.matches(waiting).count()!=1{return Err("AsyncPair WORLD gate锚点变化".into());}
    main=main.replace(waiting,"if(imageLoad(rs_state_image,ivec3(coord,7)).w!=float(RS_WORLD)){IW_FLUSH();return;}\n        bool waiting=status.y>.5&&status.y<1.5;");
    // WORLD carries only its existing phase-specific state on resume; a fresh
    // initialization retains all mutable stores, including matte setup 15/16.
    let layers=if world.contains("#define RS_MUTABLE_LAYERS 17"){17}else{15};
    let load="pg_s[layer]=resume?imageLoad(rs_state_image,ivec3(coord,layer)):vec4(0.);";
    let store="imageStore(rs_state_image,ivec3(coord,layer),pg_s[layer]);";
    let token="for(int layer=0;layer<RS_MUTABLE_LAYERS;++layer)";
    // source_image_fragment may already honor IMAGE_STATIC_STATE. Restore its
    // exact generated main blocks locally before selecting WORLD state layers.
    if !main.contains(token){for expression in[load,store]{let mut expanded=String::from("{\n");for layer in 0..layers{expanded.push_str(&expression.replace("layer",&layer.to_string()));expanded.push('\n');}expanded.push('}');if main.matches(&expanded).count()!=1{return Err("AsyncPair已展开复制锚点变化".into());}main=main.replace(&expanded,&format!("{token}\n{expression}"));}}
    for(expression,is_load)in[(load,true),(store,false)]{
        let mut found=None;
        for(start,_)in main.match_indices(token){let rest=main[start+token.len()..].trim_start();if rest.starts_with(expression){found=Some((start,main.len()-rest.len()+expression.len()));}}
        let (start,end)=found.ok_or("AsyncPair WORLD需要未展开状态复制入口")?;
        let selected:Vec<u32>=if is_load{vec![0,1,2,3,4,7,8,9,10,11,12,14]}else{vec![0,1,2,3,4,5,7,10]};
        let mut text=String::from("if(resume){\n");
        for layer in selected{if is_load{text.push_str(&format!("pg_s[{layer}]=imageLoad(rs_state_image,ivec3(coord,{layer}));\n"));}else{text.push_str(&format!("imageStore(rs_state_image,ivec3(coord,{layer}),pg_s[{layer}]);\n"));}}
        if world.contains("#define RS_TERMINAL_VISIBILITY"){if is_load{text.push_str("pg_s[6]=imageLoad(rs_state_image,ivec3(coord,6));\n");}else{text.push_str("imageStore(rs_state_image,ivec3(coord,6),pg_s[6]);imageStore(rs_state_image,ivec3(coord,9),pg_s[9]);\n");}}
        text.push_str("}else{\n");for layer in 0..layers{if is_load{text.push_str(&format!("pg_s[{layer}]=vec4(0.);\n"));}else{text.push_str(&format!("imageStore(rs_state_image,ivec3(coord,{layer}),pg_s[{layer}]);\n"));}}text.push('}');main.replace_range(start..end,&text);
    }
    main=main.replace("rs_run();","rs_run();if(pg_s[5].y==0.)pg_s[5].y=2.;");
    world.truncate(entry);world.push_str(&main);
    let mut other=source_async_samples_flags(dense,true,false)?;
    let run=phase_named_block(&other,"void rs_run(){")?;let marker="if(phase==RS_WORLD){";
    let start=run.0+other[run.0..run.1].find(marker).ok_or("AsyncPair OTHER WORLD锚点缺失")?;
    let block=phase_named_block(&other[start..],marker)?;other.replace_range(start..start+block.1,"if(phase==RS_WORLD)break;");
    let entry=other.rfind("void main(){").ok_or("AsyncPair OTHER缺少main")?;let mut main=other[entry..].to_owned();
    let init="as_was_initialized=as_initialized();as_begin_initialized();";
    if main.matches(init).count()!=1{return Err("AsyncPair OTHER初始化锚点变化".into());}
    main=main.replace(init,"as_was_initialized=as_initialized();if(!as_was_initialized){IW_FLUSH();discard;}");
    if main.matches(waiting).count()!=1{return Err("AsyncPair OTHER gate锚点变化".into());}
    main=main.replace(waiting,"if(imageLoad(rs_state_image,ivec3(coord,7)).w==float(RS_WORLD)){IW_FLUSH();discard;}\n        bool waiting=status.y>.5&&status.y<1.5;");
    main=main.replace("rs_run();","rs_run();if(pg_s[5].y==0.)pg_s[5].y=2.;");other.truncate(entry);other.push_str(&main);
    if std::env::var("DEMSTUDIO_ASYNC_STATIC_STATE").as_deref()==Ok("on"){other=expand_async_state_loops(&other)?;}
    Ok([world,other])
}

// Expand only the image entry's two state-copy loops. Primitive arithmetic,
// phase budgets, and completion's minimal 5/6 stores remain untouched.
fn expand_async_state_loops(shader:&str)->Result<String,String>{
    let layers=shader.lines().find_map(|line|line.trim().strip_prefix("#define RS_MUTABLE_LAYERS ")?.parse::<usize>().ok())
        .filter(|&n|n==15||n==17).ok_or("独立采样状态层定义无效")?;
    let token="for(int layer=0;layer<RS_MUTABLE_LAYERS;++layer)";
    if !shader.contains(token) {
        let valid=(0..layers).all(|layer|shader.contains(&format!("pg_s[{layer}]=resume?imageLoad(rs_state_image,ivec3(coord,{layer})):vec4(0.);"))
            &&shader.contains(&format!("imageStore(rs_state_image,ivec3(coord,{layer}),pg_s[{layer}]);")));
        return if valid{Ok(shader.into())}else{Err("图像状态复制循环及展开内容缺失".into())};
    }
    let load="pg_s[layer]=resume?imageLoad(rs_state_image,ivec3(coord,layer)):vec4(0.);";
    let store="imageStore(rs_state_image,ivec3(coord,layer),pg_s[layer]);";
    let entry=shader.rfind("void main(){").ok_or("独立采样缺少片元入口")?;
    let mut out=shader[..entry].to_owned();let mut rest=&shader[entry..];let mut loads=0;let mut stores=0;
    while let Some(start)=rest.find(token){
        out.push_str(&rest[..start]);let after=&rest[start+token.len()..];let body=after.trim_start();
        let (expression,count)=if body.starts_with(load){(load,&mut loads)}else if body.starts_with(store){(store,&mut stores)}else{return Err("独立采样状态循环内容改变".into());};
        *count+=1;out.push_str("{\n");
        for layer in 0..layers {out.push_str(&expression.replace("layer",&layer.to_string()));out.push('\n');}
        out.push('}');rest=&body[expression.len()..];
    }
    if loads!=1||stores!=1{return Err("独立采样状态载入或写回循环缺失".into());}
    out.push_str(rest);Ok(out)
}


