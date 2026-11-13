// Included in raystate_shader's module so the existing source generator remains
// the sole owner of batch gates, primary-hit stores and DONE storage policy.
/// Stage zero initializes the raster-interpolated camera ray. Later stages are
/// unique-pixel POINTS jobs: one complete phase, then persist the same RNG cursor.
pub(super) fn source_image_phase(dense:&str,stage:u32)->Result<String,String>{
    if stage>4{return Err(format!("RayState 阶段编号无效 {stage}"));}
    let mut shader=source_image_fragment(dense)?;
    let function=phase_named_block(&shader,"void rs_run(){")?;
    let run=&shader[function.0..function.1];
    let init=phase_named_block(run,"if(!rs_batch_resume()){")?;
    let loop_start=run.find("for(int transitions=0;").ok_or("RayState 阶段缺少循环")?;
    let loop_block=phase_named_block(&run[loop_start..],"for(int transitions=0;")?;
    let loop_end=loop_start+loop_block.1;
    let body=if stage==0 {
        // Include the complete guarded initialization, including matte setup;
        // never restore RNG or enter any intersection/surface/scatter phase.
        format!("{}\nreturn;\n}}",&run[..init.1])
    }else{
        let phase_loop=&run[loop_start..loop_end];
        let world=phase_named_block(phase_loop,"if(phase==RS_WORLD){")?;
        let selected=match stage {
            1=>phase_named_block(phase_loop,"if(phase==RS_WORLD){")?,
            2=>phase_named_block(phase_loop,"if(phase==RS_SURFACE){")?,
            3=>phase_named_block(phase_loop,"if(phase==RS_SHADOW){")?,
            4=>phase_named_block(phase_loop,"if(phase==RS_SCATTER){")?,
            _=>unreachable!(),
        };
        let loop_open=phase_loop.find('{').ok_or("RayState 阶段循环缺少起始括号")?;
        let matte=if stage==3 {
            let range=phase_named_block(phase_loop,"if(phase==RS_MATTE_SUN||phase==RS_MATTE_SKY){")?;
            format!("\n#ifdef RS_MATTE\n{}\n#endif\n",&phase_loop[range.0..range.1])
        }else{String::new()};
        // Remove the initialization branch entirely: no sample_ray/output_st(st)
        // call is reachable from any resumed POINTS invocation.
        format!("{}{}for(int transitions=0;transitions<1;++transitions){{{}{}{}\n}}{}",
            &run[..init.0],&run[init.1..loop_start],
            &phase_loop[loop_open+1..world.0],&phase_loop[selected.0..selected.1],matte,
            &run[loop_end..])
    };
    shader.replace_range(function.0..function.1,&body);
    if stage==0 {
        shader=shader.replace("if(!rs_batch_enabled())return;",
            "if(!rs_batch_enabled())return;\n    if(rs_batch_resume())return;");
    }else{
        // The queue itself selects the initialized ray and its current phase.
        shader=shader.replace("bool resume=rs_batch_resume();","bool resume=true;");
    }
    // A phase boundary is a resumable active path, just like an iteration
    // yield. Compact feedback reserves status zero for invalid/uninitialized
    // state, so rs_clear_query's transient zero must not escape a phase Draw.
    shader=shader.replace("rs_run();","rs_run();if(pg_s[5].y==0.)pg_s[5].y=2.;");
    if stage>0 {shader=phase_state_accesses(&shader,stage)?;}
    Ok(shader)
}

/// Experimental resumed-phase compute source only. Camera initialization stays
/// in stage zero's raster shader; no varying is reconstructed in this program.
/// This generator does not select a runtime path or change the default backend.
pub(super) fn source_compute_phase(dense:&str,stage:u32)->Result<String,String>{
    if !(1..=4).contains(&stage){return Err("Compute阶段仅支持已初始化的1～4阶段".into());}
    let mut shader=source_image_phase(dense,stage)?;
    shader=shader.replacen("#version 330 compatibility","#version 430 core",1);
    // These are core in 4.3. Remove the old fragment extension declarations so
    // the compute global declarations can precede the shared helper functions.
    for extension in ["GL_ARB_shader_image_load_store","GL_ARB_shader_storage_buffer_object","GL_ARB_shading_language_420pack"]{
        shader=shader.replace(&format!("#extension {extension} : require"),"");
    }
    let version_end=shader.find('\n').ok_or("Compute阶段缺少版本行")?+1;
    shader.insert_str(version_end,r#"
layout(local_size_x=64) in;
layout(std430,binding=1) readonly buffer PhaseComputeQueue { uint pq_words[]; };
uniform ivec2 pq_extent;
uniform uint pq_index_base,pq_phase;
vec2 rs_fragcoord;
"#);
    // The shared compute generator replaces the varying and fragment position;
    // apply the same coordinate adaptation to the already specialized rs_run.
    shader=shader.replace("varying vec2 st;","vec2 st;");
    shader=shader.replace("gl_FragCoord.xy","rs_fragcoord");
    // Legacy texture spellings are unavailable in core GLSL. The modern
    // equivalents retain explicit LOD where present; texture() in a compute
    // stage uses the base LOD, as the existing compatibility compute path does.
    shader=shader.replace("texture2DLod(","textureLod(").replace("texture2D(","texture(");
    let main=phase_named_block(&shader,"void main(){")?;
    let body=&shader[main.0..main.1];
    let mapped=body.replacen("void main(){",r#"void main(){
    uint ordinal=gl_GlobalInvocationID.x+gl_GlobalInvocationID.y*gl_NumWorkGroups.x*64u;
    if(pq_phase>=4u||any(lessThanEqual(pq_extent,ivec2(0))))return;
    if(ordinal>=pq_words[pq_phase*4u])return;
    uint pixel=pq_words[pq_index_base+ordinal];
    uint total=uint(pq_extent.x)*uint(pq_extent.y);
    if(pixel>=total)return;
    rs_fragcoord=vec2(pixel%uint(pq_extent.x),pixel/uint(pq_extent.x))+vec2(.5);
    st=vec2(0.);
"#,1);
    shader.replace_range(main.0..main.1,&mapped);
    if shader.contains("gl_FragCoord")||shader.contains("varying ")||shader.contains("gl_FragColor")
        ||shader.contains("layout(location=")||shader.contains("out vec4 pg_out"){
        return Err("Compute阶段残留片元专用输入输出".into());
    }
    Ok(shader)
}

/// Explicit constant-index access leaves every other persistent layer untouched.
/// Include old values for conditionally assigned outputs, including yielded
/// WORLD results and unfinished radiance, so full-state diagnostics also retain
/// their previous contents. Traversal helper pg_walk mutates only 0/1/2/5.
fn phase_state_layers(stage:u32)->(&'static [u32],&'static [u32]){
    match stage {
        1=>(&[0,1,2,3,4,7,8,9,10,11,12,14],&[0,1,2,3,4,5,7,10]),
        2=>(&[3,4,6,7,8,9,10,11,12,13,14],&[5,6,7,9,10,11,12,13]),
        3=>(&[0,1,2,6,7,8,9,10,11,12,13,14,15,16],&[0,1,2,5,6,7,10,15]),
        4=>(&[7,8,9,10,11,12,14],&[5,7,8,9,10,14]),
        _=>(&[],&[]),
    }
}

fn phase_state_accesses(shader:&str,stage:u32)->Result<String,String>{
    let main=phase_named_block(shader,"void main(){")?;
    let mut body=shader[main.0..main.1].to_owned();
    let (loads,stores)=phase_state_layers(stage);
    let expand=|layers:&[u32],load:bool|{
        let mut text=String::new();
        for &layer in layers {
            if layer>=15{text.push_str("\n#ifdef RS_MATTE\n");}
            if load {text.push_str(&format!("pg_s[{layer}]=imageLoad(rs_state_image,ivec3(coord,{layer}));\n"));}
            else{text.push_str(&format!("imageStore(rs_state_image,ivec3(coord,{layer}),pg_s[{layer}]);\n"));}
            if layer>=15{text.push_str("#endif\n");}
        }
        if stage==1&&shader.contains("#define RS_TERMINAL_VISIBILITY"){
            // A terminal WORLD can finish directly. Preserve old radiance on
            // other WORLD exits, and persist the same opaque alpha as SURFACE.
            if load{text.push_str("pg_s[6]=imageLoad(rs_state_image,ivec3(coord,6));\n");}
            else{text.push_str("imageStore(rs_state_image,ivec3(coord,6),pg_s[6]);\nimageStore(rs_state_image,ivec3(coord,9),pg_s[9]);\n");}
        }
        text
    };
    let marker="for(int layer=0;layer<RS_MUTABLE_LAYERS;++layer)";
    // Replace the load and final store statements only. The preceding DONE
    // branch keeps its dedicated 5/6 writes and early return unchanged.
    for (layers,load) in [(loads,true),(stores,false)] {
        let start=body.find(marker).ok_or("RayState 阶段缺少状态层循环")?;
        let statement=start+marker.len();
        let end=body[statement..].find(';').map(|n|statement+n+1).ok_or("RayState 阶段状态语句不完整")?;
        let expected=if load {"pg_s[layer]=resume?imageLoad("}else{"imageStore(rs_state_image,"};
        if !body[statement..end].trim_start().starts_with(expected){return Err("RayState 阶段状态循环结构改变".into());}
        body.replace_range(start..end,&expand(layers,load));
    }
    let mut output=shader.to_owned();output.replace_range(main.0..main.1,&body);Ok(output)
}

/// Return a complete named statement/function including its balanced body.
/// Braces inside GLSL comments do not change lexical nesting.
fn phase_named_block(source:&str,marker:&str)->Result<(usize,usize),String>{
    let start=source.find(marker).ok_or_else(||format!("RayState 阶段缺少入口 {marker}"))?;
    let open=source[start..].find('{').map(|i|start+i).ok_or("RayState 阶段缺少起始括号")?;
    let bytes=source.as_bytes();let mut i=open;let mut depth=0usize;
    while i<bytes.len(){
        if bytes[i..].starts_with(b"//"){
            i+=2;while i<bytes.len()&&bytes[i]!=b'\n'{i+=1;}continue;
        }
        if bytes[i..].starts_with(b"/*"){
            i+=2;while i+1<bytes.len()&&!bytes[i..].starts_with(b"*/"){i+=1;}
            if i+1>=bytes.len(){return Err("RayState 阶段注释不完整".into());}i+=2;continue;
        }
        match bytes[i]{b'{'=>depth+=1,b'}'=>{depth-=1;if depth==0{return Ok((start,i+1));}},_=>{}}
        i+=1;
    }
    Err("RayState 阶段括号不完整".into())
}


