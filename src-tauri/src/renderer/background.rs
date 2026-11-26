#[derive(Clone, Debug, serde::Deserialize, serde::Serialize, PartialEq)]
#[serde(default, rename_all="camelCase")]
pub struct BackgroundState {
    pub mode:String, pub color:String, pub color2:String, pub angle:f32,
    pub weight:f32, pub components:Vec<BackgroundState>,
    pub follow_light:bool, pub sky_follow_light:bool,
}
impl Default for BackgroundState {fn default()->Self {Self {weight:1.,components:Vec::new(),mode:"sky".into(),color:"#30343b".into(),color2:"#647180".into(),angle:90.,follow_light:false,sky_follow_light:true}}}
pub(super) fn transition_active(p:&super::RenderParams)->bool {p.transition_background_from.is_some() && p.transition_background_mix<1. && !p.export_transparent}
pub(super) fn components(p:&super::RenderParams)->Vec<BackgroundState>{
    let Some(source)=&p.transition_background_from else{return Vec::new()};
    if source.components.is_empty(){vec![source.clone()]}else{source.components.clone()}
}
pub(super) fn physical_weight(p:&super::RenderParams)->f32{
    if !transition_active(p){return 0.;}
    let values=components(p);let total: f32=values.iter().map(|v|v.weight).sum();
    let old=values.iter().filter(|v|v.mode=="sky"&&!v.sky_follow_light).map(|v|v.weight).sum::<f32>()/total.max(1e-12);
    old*(1.-p.transition_background_mix)+if p.background_mode=="sky"&&!p.sky_follow_light {p.transition_background_mix}else{0.}
}
pub(super) fn validate(p:&super::RenderParams)->Result<(),String>{
    if !p.transition_background_mix.is_finite() || !(0. ..=1.).contains(&p.transition_background_mix){return Err("背景过渡比例无效".into());}
    for weight in [p.transition_sky_follow,p.transition_sky_model].into_iter().flatten(){
        if !weight.is_finite()||!(0. ..=1.).contains(&weight){return Err("天空过渡比例无效".into());}
    }
    let values=components(p);
    if values.len()>32{return Err("背景过渡最多支持 32 个分量".into());}
    let mut total=0.;
    for b in &values {
        if !["sky","solid","gradient"].contains(&b.mode.as_str()) || !b.angle.is_finite() || !b.weight.is_finite() || b.weight<0. || !b.components.is_empty(){return Err("背景过渡状态无效".into());}
        super::gl_renderer::parse_color(&b.color)?;super::gl_renderer::parse_color(&b.color2)?;total+=b.weight;
    }
    if !values.is_empty() && total<=0. {return Err("背景过渡分量权重之和必须大于零".into());}Ok(())
}
// Camera-visible background and independently shadowed physical ground.
pub(super) const HEADER: &str = include_str!("background.glsl");
pub(super) const POST_HEADER: &str = include_str!("background_post.glsl");

/// Applied to recovered source blocks before their concatenation. The original
/// sky/light functions remain untouched for every indirect illumination ray.
/// Disabling ground shadows also lets its sampled diffuse continuation reach
/// the environment; bypassing only direct visibility would retain terrain AO.
/// Opaque custom backgrounds use the existing shadow catcher for primary ground
/// hits. The ground remains physical for secondary rays and transparent exports.
pub(super) fn transform_block(block: &str) -> String {
    let block = if block.contains("int radiance(") {
        let helper=if block.contains("vec4 catch_shadow(vec3 p)") {
            "vec4 catch_shadow(vec3 p);vec4 scene_ground_shadow_matte(vec3 p){return catch_shadow(p);}".to_owned()
        } else {include_str!("shadow_catcher.glsl").replace("catch_shadow", "scene_ground_shadow_matte")};
        format!("{helper}\n{}\n{}\n{block}",include_str!("sky_backdrop.glsl"),include_str!("background_ground_transition.glsl"))
    }else{block.to_owned()};
    block.replace("vec4 pt(vec3 o,vec3 dir,vec3 A){","vec4 pt(vec3 o,vec3 dir,vec3 A){vec3 scene_primary_o=o,scene_primary_dir=dir;float scene_primary_ground_d=-1.;")
    .replace("path_id=ray_vs_world(o,dir,d,n);ref=radiance", "path_id=ray_vs_world(o,dir,d,n);if(bounce==0&&is_ground(path_id.y))scene_primary_ground_d=d;ref=radiance")
    .replace("return vec4(Lo*path_alpha,path_alpha);", "if(scene_primary_ground_d>0.){vec4 result=scene_transition_ground(vec4(Lo*path_alpha,path_alpha),scene_primary_o+scene_primary_dir*scene_primary_ground_d);path_alpha=result.a;Lo=result.rgb/max(path_alpha,1e-12);}return vec4(Lo*path_alpha,path_alpha);")
    .replace(
        "inout float inv_pdf_o_brdf){\n#if defined(OPT_BG_A)",
        "inout float inv_pdf_o_brdf){\nif(bounce==0&&is_ground(id.y)&&scene_layout_shadow_only!=0&&scene_background_transparent!=0){vec4 matte=scene_ground_shadow!=0?scene_ground_shadow_matte(o+dir*d):vec4(0.);Lo=matte.rgb;path_alpha=matte.a;return BRDF_EMIT;}\nif(bounce==0&&is_empty(max(id.x,id.y))&&scene_background_transition==0&&scene_background_mode==0&&scene_sky_follow!=0&&scene_background_transparent==0){Lo=scene_sky_backdrop(dir);path_alpha=1.;return BRDF_EMIT;}\nif(bounce==0&&is_empty(max(id.x,id.y))&&scene_background_transition!=0&&scene_background_physical_weight>0.){\n#ifdef OPT_SUN_DISK\nLo+=eval_light_sun(A,dir,INF,n,inv_pdf_o_brdf,0.,0.,path_mask.xy);\n#endif\nLo+=eval_light_sky(A,scene_visible_sky_direction(dir),INF,n,inv_pdf_o_brdf,0.,0.,path_mask.xy);path_alpha=scene_background_physical_weight;return BRDF_EMIT;}\nif(bounce==0&&is_empty(max(id.x,id.y))&&scene_background_override()){vec4 bg=scene_visible_background();Lo=bg.rgb;path_alpha=bg.a;return BRDF_EMIT;}\nif(bounce==0&&is_ground(id.y)&&scene_background_physical_weight<=0.&&(scene_background_transition!=0||scene_background_mode!=0||scene_sky_follow!=0)&&scene_background_transparent==0){vec3 p=o+dir*d;vec4 matte=scene_ground_shadow!=0?scene_ground_shadow_matte(p):vec4(0.);\n#ifdef OPT_GRID\nvec4 grid=grid_line(id,p,n);matte.a=grid.w*.35+matte.a*(1.-grid.w*.35);\n#endif\nif(scene_background_mode==0&&scene_background_transition==0){Lo=scene_sky_backdrop(dir)*(1.-matte.a)+matte.rgb;path_alpha=1.;}else{Lo=matte.rgb;path_alpha=matte.a;}return BRDF_EMIT;}\nif(bounce==0&&is_ground(id.y)&&(scene_background_mode==0||scene_background_physical_weight>0.)&&scene_sky_follow==0&&scene_background_transparent==0){float ground_weight=smoothstep(0.,.12,max(-dir.z,0.));Lo+=A*scene_sky(scene_visible_sky_direction(vec3(dir.xy,abs(dir.z))))*(1.-ground_weight);A*=ground_weight;}\n#if defined(OPT_BG_A)",
    ).replace(
        "Lo+=eval_light_sky(A,dir,INF,n,inv_pdf_o_brdf,0.0,0.0,path_mask.xy);",
        "Lo+=eval_light_sky(A*(bounce==0?scene_visible_sky_scale():1.),(bounce==0?scene_visible_sky_direction(dir):dir),INF,n,inv_pdf_o_brdf,0.0,0.0,path_mask.xy);",
    ).replace(
        "path_id=ray_vs_world(o,dir,d,n);",
        "if(scene_ground_shadow==0&&bounce>0&&is_ground(path_id.y)){path_id=vec2(ID_AIR);}else{path_id=ray_vs_world(o,dir,d,n);}",
    ).replace(
        "float shadow(vec3 o,vec3 dir,float d){",
        "float shadow(vec3 o,vec3 dir,float d){if(scene_ground_shadow==0&&is_ground(path_id.y))return 1.;",
    )
}
