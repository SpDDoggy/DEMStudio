//! Shared source expansion; both ordinary and resumable paths call these BRDFs.
pub fn material_source(source: &str) -> String {
    let Some(start) = source.find("vec3 mtl_diffuse_brdf(") else { return source.into(); };
    let Some(end) = source[start..].find("SAMPLE_LIGHT_DEF(sample_light_diffuse,") else { return source.into(); };
    format!("{}\n{}\n{}", &source[..start], include_str!("material_response.glsl"), &source[start+end..])
}
