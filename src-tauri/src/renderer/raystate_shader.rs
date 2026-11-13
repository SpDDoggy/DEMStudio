//! Full path continuation for the currently enabled DEMStudio lighting model.
//! Not wired into the renderer until native output equivalence is verified.
pub(super) const STATE_LAYERS: u32 = 19;
pub(super) fn compute_batch_enabled()->bool {
    cfg!(test)&&std::env::var("DEMSTUDIO_DIAGNOSTIC_COMPUTE_BATCH").as_deref()==Ok("on")
}
pub(super) fn compute_group_size()->u32 {
    
    256
}
include!("phase_shader.rs");
include!("async_shader.rs");
// Preserve the rasterizer's actual varying interpolation, including rounding.
// Layer 18 is scratch only until the first traversal stores its primary ID.
pub(super) const INTERPOLATED_ST_FRAGMENT: &str = "#version 330 compatibility\nin vec2 st;layout(location=0) out vec4 rs_st;void main(){rs_st=vec4(st,0.,0.);}";
pub(super) const IMAGE_QUEUE_VERTEX: &str = r#"#version 330 compatibility
#extension GL_ARB_shader_storage_buffer_object : require
layout(std430,binding=1) readonly buffer ActiveQueue { uint words[]; };
uniform int rs_use_point_queue;
uniform ivec2 aq_extent;
out vec2 st;
uniform vec2 aspect;
uniform vec2 pixel_jitter;
void main(){
    if(rs_use_point_queue==0){
        vec2 p=vec2((gl_VertexID<<1)&2,gl_VertexID&2);
        gl_Position=vec4(p*2.-1.,0.,1.);
        st=(p*2.-1.)*aspect+pixel_jitter;
    }else{
        uint pixel=words[4+gl_VertexID];
        vec2 center=vec2(pixel%uint(aq_extent.x),pixel/uint(aq_extent.x))+.5;
        vec2 ndc=center/vec2(aq_extent)*2.-1.;
        gl_Position=vec4(ndc,0.,1.);gl_PointSize=1.;
        st=vec2(0.);
    }
}"#;

pub(super) fn source_with_mrt(dense: &str, wide: bool) -> Result<String, String> {
    for unsupported in ["OPT_FOG", "OPT_AREA", "OPT_IBL", "OPT_BG_A", "OPT_BG_C", "OPT_HOR", "OPT_LE", "OPT_DOF", "OPT_SPIN"] {
        if dense.lines().any(|line| line.trim_start().starts_with(&format!("#define {unsupported}"))) {
            return Err(format!("RayState 尚未覆盖已启用宏 {unsupported}"));
        }
    }
    let mut shader = dense.to_owned();
    if cfg!(test)&&std::env::var("DEMSTUDIO_LAZY_BOUNDARY").as_deref()==Ok("on"){
        shader=shader.replacen("#version 330 compatibility","#version 330 compatibility\n#define PG_LAZY_BOUNDARY",1);
    }
    if std::env::var("DEMSTUDIO_BICUBIC_PATCH_GATHER").as_deref()==Ok("on"){
        if !dense.contains("#define PRIM_BICUBIC"){return Err("BicubicPatchGather仅支持Bicubic".into());}
        shader=shader.replacen("#version 330 compatibility","#version 330 compatibility\n#extension GL_ARB_texture_gather : require\n#extension GL_ARB_gpu_shader5 : require\n#define BC_PATCH_GATHER",1);
    }
    if std::env::var("DEMSTUDIO_BILINEAR_LEAF_GATHER").as_deref()==Ok("on"){
        if !dense.lines().any(|line|line.trim()=="#define PRIM_SURF"){return Err("LeafGather仅支持Bilinear".into());}
        shader=shader.replacen("#version 330 compatibility","#version 330 compatibility\n#extension GL_ARB_texture_gather : require\n#extension GL_ARB_gpu_shader5 : require\n#define PG_BILINEAR_LEAF_GATHER",1);
    }
    if std::env::var("DEMSTUDIO_ANCESTOR_SEED").as_deref()==Ok("on"){
        shader=shader.replacen("#version 330 compatibility","#version 330 compatibility\n#define PG_ANCESTOR_SEED",1);
    }
    if std::env::var("DEMSTUDIO_GEO_MERGED").as_deref()==Ok("on"){
        shader=shader.replacen("#version 330 compatibility","#version 330 compatibility\n#define PG_GEO_MERGED",1);
    }
    if std::env::var("DEMSTUDIO_BICUBIC_ROW_CACHE").as_deref()==Ok("on")&&dense.contains("#define PRIM_BICUBIC") {
        let define=if cfg!(test)&&std::env::var("DEMSTUDIO_BICUBIC_ROW_BOUND").as_deref()==Ok("on"){"#version 330 compatibility\n#define BC_ROW_BOUND"}else{"#version 330 compatibility\n#define BC_ROW_CACHE"};
        shader=shader.replacen("#version 330 compatibility",define,1);
        if cfg!(test)&&std::env::var("DEMSTUDIO_BICUBIC_ROW_BOUND").as_deref()==Ok("on")&&std::env::var("DEMSTUDIO_BICUBIC_CELL_BOUND").as_deref()==Ok("on"){
            shader=shader.replacen("#define BC_ROW_BOUND","#define BC_ROW_BOUND\n#define BC_CELL_BOUND",1);
        }
    }
    if std::env::var("DEMSTUDIO_BICUBIC_LEAF_BOUNDS").as_deref()==Ok("on")&&dense.contains("#define PRIM_BICUBIC") {
        shader=shader.replacen("#version 330 compatibility","#version 330 compatibility\n#define BC_LEAF_BOUND_TEX",1);
    }
    if crate::renderer::tight_bounds::enabled("bilinear")&&dense.lines().any(|line|line.trim()=="#define PRIM_SURF") {
        shader=shader.replacen("#version 330 compatibility","#version 330 compatibility\n#define PG_TIGHT_BOUNDS",1);
    }
    if std::env::var("DEMSTUDIO_TERMINAL_VISIBILITY").as_deref()==Ok("on")
        &&(dense.contains("#define PRIM_BICUBIC")||dense.contains("#define PRIM_SURF\n")){
        shader=shader.replacen("#version 330 compatibility","#version 330 compatibility\n#define RS_TERMINAL_VISIBILITY",1);
    }
    if dense.contains("#define PRIM_BICUBIC")&&crate::renderer::bicubic_bounds::enabled("bicubic"){
        shader=shader.replacen("#version 330 compatibility","#version 330 compatibility\n#define BC_PRECOMPUTED_BOUNDS",1);
    }
    if wide { shader = shader.replacen("#version 330 compatibility", "#version 330 compatibility\n#define PG_WIDE_MRT", 1); }
    let insertion = shader.find("float geo_height(").or_else(|| shader.find("float heightLod(")).ok_or("RayState 缺少高程采样入口")?;
    let offset = if dense.contains("uniform vec2 geo_source_size;") { "vec2(1.)" } else { "vec2(0.)" };
    let header = include_str!("paged_header.glsl").replace("vec4 pg_s[7];", "vec4 pg_s[19];");
    shader.insert_str(insertion, &format!("{header}\nvec2 pg_logical_offset(){{return {offset};}}\n"));
    shader = shader.replace("texture2DLod(mipmap,", "pg_texture_mipmap(");
    let walk = include_str!("paged_walk.glsl").split("vec2 pg_world(").next().ok_or("RayState 缺少遍历函数")?;
    shader = replace_function(&shader, "vec2 ray_vs_world(", &format!("{walk}\nvec2 ray_vs_world(vec3 o,vec3 dir,inout float d,inout vec3 n){{return vec2(ID_AIR);}}"))?;
    shader = replace_function(&shader, "float shadow(", "float shadow(vec3 o,vec3 dir,float d){return 1.;}")?;
    shader = shader.replace("void main(void)", "void rs_unused_original_main(void)").replace("gl_FragColor", "pg_radiance");
    if dense.contains("uniform vec2 geo_source_size;") { shader.push_str("\n#define RS_GEO\n"); }
    if dense.contains("vec4 catch_shadow(") { shader.push_str("\n#define RS_MATTE\n"); }
    shader.push_str(include_str!("raystate_main.glsl"));
    
    
    
    Ok(shader)
}

/// GL 4.3 path: one invocation owns one pixel's complete state. The queue must
/// contain unique pixel IDs. Its four-word header is shared with the scheduler;
/// word zero is active count and words 4+ are row-major pixel IDs.
pub(super) fn source_compute(dense: &str) -> Result<String, String> {
    let mut shader = source_with_mrt(dense, false)?;
    let main = shader.rfind("void main(){").ok_or("RayState 缺少计算入口")?;
    shader.truncate(main);
    shader = shader.replacen("#version 330 compatibility", &format!("#version 430 compatibility\n{}", include_str!("raystate_compute_header.glsl")), 1);
    shader = shader.replace("gl_FragCoord.xy", "rs_fragcoord");
    shader = shader.replace("varying vec2 st;", "vec2 st;");
    
    // Fragment outputs do not exist in a compute stage. All 19 state layers
    // are written directly once; there are no bank re-executions or copies.
    for index in 0..7 {
        shader = shader.replace(&format!("layout(location={index}) out vec4 pg_out{index};"), "");
    }
    shader=shader.replace("layout(local_size_x=256)",&format!("layout(local_size_x={})",compute_group_size()));
    shader.push_str(&include_str!("raystate_compute_main.glsl").replace("256u",&format!("{}u",compute_group_size())));
    if compute_batch_enabled(){
        shader=shader.replace("void rs_run(){",&format!("{}\nvoid rs_run(){{",crate::renderer::gpu_batch::FRAGMENT_HEADER));
        shader=shader.replace("if(pg_resume==0){","if(!rs_batch_resume()){");
        shader=shader.replace("void main(){","void main(){\n    if(!rs_batch_enabled())return;\n    bool rs_compute_resume=rs_batch_resume();");
        shader=shader.replace("    rs_fragcoord=vec2(coord)+vec2(.5);",r#"    rs_fragcoord=vec2(coord)+vec2(.5);
    if(rs_compute_resume){
        vec4 status=imageLoad(rs_state_image,ivec3(coord,5));
        bool done=status.y>3.5,waiting=status.y>.5&&status.y<1.5;
        if(done||(waiting&&pg_page_slot(int(status.x))<0))return;
    }"#);
        shader=shader.replace("st=pg_resume==0?","st=!rs_compute_resume?");
        shader=shader.replace("pg_s[layer]=pg_resume!=0?","pg_s[layer]=rs_compute_resume?");
        shader=shader.replace("    bool done=pg_s[5].y>3.5;bool waiting=pg_s[5].y>.5&&pg_s[5].y<1.5;\n    if(done||(waiting&&pg_page_slot(int(pg_s[5].x))<0))return;","");
        // Bilinear uses the same ready-phase limit already verified in image
        // integration. Page faults and per-query traversal budgets still yield.
        shader=shader.replace("transitions<8","transitions<32");
    }
    Ok(shader)
}

/// One raster invocation stores the entire continuation state. Keep the
/// original language version and raster stage for interpolation and arithmetic.
pub(super) fn source_image_fragment(dense: &str) -> Result<String, String> {
    source_image_fragment_impl(dense,true)
}

fn source_image_fragment_impl(dense:&str,root_transport:bool)->Result<String,String>{
    #[cfg(not(test))] let _=root_transport;
    let mut shader = source_with_mrt(dense, false)?;
    
    // Counterfactual cost bound only: absent from application builds. This
    // changes radiance, so its output can never qualify as a renderer candidate.
    
    if std::env::var("DEMSTUDIO_VISIBILITY_PROBE").as_deref()==Ok("on"){
        inject_visibility_diagnostic(&mut shader)?;
    }
    let main = shader.rfind("void main(){").ok_or("RayState 缺少图像片元入口")?;
    shader.truncate(main);
    shader = shader.replacen("#version 330 compatibility", "#version 330 compatibility\n#extension GL_ARB_shader_image_load_store : require\n#extension GL_ARB_shading_language_420pack : require\nlayout(rgba32f,binding=0) uniform image2DArray rs_state_image;", 1);
    if crate::renderer::walk_profile::enabled(){
        crate::renderer::walk_profile::inject_header(&mut shader)?;
    }
    for index in 0..7 {
        shader = shader.replace(&format!("layout(location={index}) out vec4 pg_out{index};"), "");
    }
    // The immutable primary hit is consumed only by the host's first-sample
    // memo copy. Store it once at its producer, outside the mutable path state.
    shader=shader.replace("if(bounce==0){pg_s[17]=pg_s[3];pg_s[18]=pg_s[4];}",
        "if(bounce==0){imageStore(rs_state_image,ivec3(ivec2(gl_FragCoord.xy),17),pg_s[3]);imageStore(rs_state_image,ivec3(ivec2(gl_FragCoord.xy),18),pg_s[4]);}");
    // A full state dump is a diagnostic contract, including retired path layers.
    // Production consumers of DONE paths read status 5 and radiance 6 only.
    let full_state_diagnostic=cfg!(test)&&(std::env::var_os("DEMSTUDIO_STATE_DUMP").is_some()
        ||std::env::var_os("DEMSTUDIO_TRACE_INITIAL_RAY").is_some()
        ||std::env::var_os("DEMSTUDIO_TRACE_CAMERA_INPUT").is_some()
        ||std::env::var_os("DEMSTUDIO_TRACE_BICUBIC_PATCH").is_some());
    shader.push_str(done_store_header(std::env::var("DEMSTUDIO_DONE_STATE_STORE").as_deref()!=Ok("full")
        &&!full_state_diagnostic));
    let debug_camera=cfg!(test)&&(std::env::var_os("DEMSTUDIO_TRACE_INITIAL_RAY").is_some()
        ||std::env::var_os("DEMSTUDIO_TRACE_CAMERA_INPUT").is_some()
        ||std::env::var_os("DEMSTUDIO_TRACE_BICUBIC_PATCH").is_some());
    shader.push_str(if dense.contains("vec4 catch_shadow(")||debug_camera {"\n#define RS_MUTABLE_LAYERS 17\n"}else{"\n#define RS_MUTABLE_LAYERS 15\n"});
    shader=shader.replacen("#extension GL_ARB_shader_image_load_store : require","#extension GL_ARB_shader_storage_buffer_object : require\n#extension GL_ARB_shader_image_load_store : require",1);
    shader=shader.replace("void rs_run(){",&format!("{}\nvoid rs_run(){{",crate::renderer::gpu_batch::FRAGMENT_HEADER));
    shader=shader.replace("if(pg_resume==0){","if(!rs_batch_resume()){");
    // Keep more ready phases local. Faults and traversal-budget yields still
    // return immediately with the exact cursor; no query is replayed.
    // Bicubic's driver-generated arithmetic changes when phase boundaries are
    // fused further. Keep its verified boundary; only bilinear passed the full
    // 256-sample byte comparison for the larger local phase budget.
    if dense.contains("#define PRIM_SURF\n") {shader=shader.replace("transitions<8","transitions<32");}
    // Explicit test only: measure phase scheduling grain without changing any
    // traversal quantum, path arithmetic, state layout or commit order.
    
    let profile=crate::renderer::bicubic_profile::enabled()&&dense.contains("#define PRIM_BICUBIC");
    if profile {
        // The header must precede bicubic.glsl's conditional local counters.
        let insertion=shader.find("// DEMStudio uniform cubic B-spline surface.").ok_or("Bicubic 诊断缺少曲面入口")?;
        shader.insert_str(insertion,crate::renderer::bicubic_profile::SHADER_HEADER);
        shader.push_str(&include_str!("raystate_image_main.glsl").replace("rs_run();","rs_run();bc_profile_flush();"));
    }else{shader.push_str(include_str!("raystate_image_main.glsl"));}
    
    
    
    if std::env::var("DEMSTUDIO_IMAGE_STATIC_STATE").as_deref()==Ok("on") {shader=expand_async_state_loops(&shader)?;}
    Ok(shader)
}

fn inject_visibility_diagnostic(shader:&mut String)->Result<(),String>{
    let at=shader.find("bool pg_walk(").ok_or("可见性诊断缺少遍历入口")?;
    shader.insert_str(at,&crate::renderer::visibility_diagnostic::shader_header());Ok(())
}

fn done_store_header(minimal:bool)->&'static str {
    if minimal {"\n#define RS_DONE_MINIMAL_STORE\n"}else{""}
}

fn replace_function(source: &str, signature: &str, replacement: &str) -> Result<String, String> {
    let start = source.find(signature).ok_or_else(|| format!("RayState 缺少函数 {signature}"))?;
    let brace = source[start..].find('{').map(|p| start+p).ok_or("RayState 函数缺少起始括号")?;
    let mut depth = 0;
    for (position, byte) in source.as_bytes().iter().enumerate().skip(brace) {
        match byte { b'{' => depth += 1, b'}' => depth -= 1, _ => {} }
        if depth == 0 { return Ok(format!("{}\n{replacement}\n{}", &source[..start], &source[position+1..])); }
    }
    Err("RayState 函数括号不完整".into())
}










