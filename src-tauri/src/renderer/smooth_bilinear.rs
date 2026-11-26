pub(super) fn enabled() -> bool {
    match std::env::var("DEMSTUDIO_SMOOTH_BILINEAR").as_deref() {
        Ok("on") => true,
        Ok("off") => false,
        _ => true,
    }
}
pub(super) fn source(block: &str) -> String {
    let normal="out_n=normalize(vec3(-bilinear_grad(h,(o.xy+dir.xy*e.x-i_pos)*inv),scale));";
    assert!(block.contains(normal),"smooth bilinear requires the original normal site");
    block.replace("float bi_delta(",&format!("{}\nfloat bi_delta(",include_str!("smooth_bilinear.glsl")))
        .replace(normal,"out_n=normalize(vec3(-sn_gradient(h,i_pos,clamp((o.xy+dir.xy*e.x-i_pos)*inv,vec2(0.),vec2(1.)),scale),scale));")
}
