//! Export-only transformation of the existing dense path shader. All material,
//! random-number, camera and primitive equations remain shared with that shader.

pub(super) const STATE_LAYERS: u32 = 7;

/// The current shader has one world query and at most one direct-light shadow
/// query per bounce, plus the final escaped/end query. Shadow-matte paths use
/// exactly two queries instead. This fits GL3.3's minimum 256 array layers even
/// for the explicitly enabled 100-bounce experiment.
pub(super) fn query_capacity(max_bounces: u32) -> Result<u32, String> {
    if !(1..=100).contains(&max_bounces) { return Err("分页反弹次数无效".into()); }
    Ok(2 * max_bounces + 2)
}

pub(super) fn source(dense: &str) -> Result<String, String> {
    source_with_mrt(dense, false)
}

pub(super) fn source_with_mrt(dense: &str, wide: bool) -> Result<String, String> {
    let mut shader = dense.to_owned();
    if dense.contains("#define PRIM_BICUBIC")&&crate::renderer::bicubic_bounds::enabled("bicubic"){
        shader=shader.replacen("#version 330 compatibility","#version 330 compatibility\n#define BC_PRECOMPUTED_BOUNDS",1);
    }
    if wide { shader = shader.replacen("#version 330 compatibility", "#version 330 compatibility\n#define PG_WIDE_MRT", 1); }
    // Declarations referencing mipmap_tex_inv_size must follow its declaration.
    // The header is inserted immediately before the first height sampler body.
    let insertion = shader.find("float geo_height(")
        .or_else(|| shader.find("float heightLod("))
        .ok_or("分页着色器缺少高程采样入口")?;
    let offset = if dense.contains("uniform vec2 geo_source_size;") { "vec2(1.)" } else { "vec2(0.)" };
    let header = format!("{}\nvec2 pg_logical_offset(){{return {};}}\n", include_str!("paged_header.glsl"), offset);
    shader.insert_str(insertion, &header);
    // Preserve all original coordinate transforms (including geographic border
    // offsets) and replace only the final raw texture fetch.
    shader = shader.replace("texture2DLod(mipmap,", "pg_texture_mipmap(");
    shader = replace_function(&shader, "vec2 ray_vs_world(",
        &format!("{}\nvec2 ray_vs_world(vec3 o,vec3 dir,inout float d,inout vec3 n){{return pg_world(o,dir,d,n);}}", include_str!("paged_walk.glsl")))?;
    shader = replace_function(&shader, "float shadow(",
        &format!("{}\nfloat shadow(vec3 o,vec3 dir,float d){{return pg_shadow(o,dir,d);}}", include_str!("paged_shadow.glsl")))?;
    let loop_marker = "while(ref>BRDF_EMIT)";
    if !shader.contains(loop_marker) { return Err("分页着色器缺少路径循环入口".into()); }
    shader = shader.replace(loop_marker, "while(ref>BRDF_EMIT&&!pg_yield)");
    if !shader.contains("void main(void)") { return Err("分页着色器缺少主函数入口".into()); }
    shader = shader.replace("void main(void)", "void pg_original_main(void)")
        .replace("gl_FragColor", "pg_radiance");
    shader.push_str(include_str!("paged_main.glsl"));
    Ok(shader)
}

fn replace_function(source: &str, signature: &str, replacement: &str) -> Result<String, String> {
    let start = source.find(signature).ok_or_else(|| format!("分页着色器缺少函数 {signature}"))?;
    let brace = source[start..].find('{').map(|p| start+p).ok_or("分页函数定义缺少起始括号")?;
    let mut depth = 0;
    for (position, byte) in source.as_bytes().iter().enumerate().skip(brace) {
        match byte { b'{' => depth += 1, b'}' => depth -= 1, _ => {} }
        if depth == 0 {
            return Ok(format!("{}\n{}\n{}", &source[..start], replacement, &source[position+1..]));
        }
    }
    Err("分页函数定义括号不完整".into())
}


