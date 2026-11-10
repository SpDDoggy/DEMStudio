//! Complete source residency uses the original path equations and traversals.
//! Only raw height fetches are redirected to compact globally aligned pages.
use super::paged_source::MipLevel;

const FETCH: &str = r#"
uniform int rs_primary_cached;
uniform sampler2D rs_primary_hit,rs_primary_id;
layout(location=0) out vec4 rs_color;
layout(location=1) out vec4 rs_cache_hit;
layout(location=2) out vec4 rs_cache_id;
bool rs_first_world=true;
uniform ivec2 rs_source_size;
uniform sampler2DArray rs_low;
uniform sampler2D rs_coarse;
vec4 rs_texture_mipmap(vec2 uv,float level){
    int lod=max(0,int(floor(level+.5)));
    // Match nearest mip sampling after the existing geographic/bicubic UV
    // transforms. Padding is the clamped source boundary at every level.
    vec2 p=clamp(uv/mipmap_tex_inv_size-vec2(.5),vec2(0.),vec2(rs_source_size-ivec2(1)));
    ivec2 cell=ivec2(floor((p+vec2(.5))/exp2(float(lod))));
    if(lod>=5){
        int coarse_lod=lod-5;
        cell=clamp(cell,ivec2(0),textureSize(rs_coarse,coarse_lod)-ivec2(1));
        return vec4(texelFetch(rs_coarse,cell,coarse_lod).x);
    }
    int edge=512>>lod;
    ivec2 page=cell/edge;
    int layer=page.y*((rs_source_size.x+511)/512)+page.x;
    return vec4(texelFetch(rs_low,ivec3(cell-page*edge,layer),lod).x);
}
"#;

const PRIMARY_QUERY: &str = r#"
vec2 ray_vs_world(vec3 o,vec3 dir,inout float d,inout vec3 n){
    if(!rs_first_world) return rs_uncached_world(o,dir,d,n);
    rs_first_world=false;
    if(rs_primary_cached!=0){
        ivec2 pixel=ivec2(gl_FragCoord.xy);
        vec4 id=texelFetch(rs_primary_id,pixel,0);
        // The shadow catcher bypasses world intersection. Its cache is empty,
        // including when the separate surface pass later intersects this ray.
        if(id.z>.5){
            vec4 hit=texelFetch(rs_primary_hit,pixel,0);
            n=hit.xyz;d=hit.w;return id.xy;
        }
    }
    vec2 id=rs_uncached_world(o,dir,d,n);
    rs_cache_hit=vec4(n,d);
    rs_cache_id=vec4(id,1.,0.);
    return id;
}
"#;

pub(super) fn source(dense: &str) -> Result<String, String> {
    let insertion = dense.find("float geo_height(")
        .or_else(|| dense.find("float heightLod("))
        .ok_or("常驻源着色器缺少高程采样入口")?;
    if !dense[..insertion].contains("uniform vec2 mipmap_tex_inv_size;")
        || !dense.contains("texture2DLod(mipmap,") {
        return Err("常驻源着色器缺少原始纹理采样".into());
    }
    let traversal = "for(int i=0;i<100000&&e.x<e.y;++i)";
    if dense.matches(traversal).count() != 2 {
        return Err("常驻源着色器高程遍历入口不匹配".into());
    }
    let mut result = dense.to_owned();
    result.insert_str(insertion, FETCH);
    let signature="vec2 ray_vs_world(";
    let start=result.find(signature).ok_or("常驻源着色器缺少场景求交入口")?;
    let open=result[start..].find('{').map(|p| start+p).ok_or("场景求交函数缺少起始括号")?;
    let mut depth=0;
    let mut end=None;
    for (position,byte) in result.as_bytes().iter().enumerate().skip(open) {
        match byte { b'{' => depth+=1, b'}' => depth-=1, _ => {} }
        if depth==0 { end=Some(position+1);break; }
    }
    let end=end.ok_or("场景求交函数括号不完整")?;
    result.insert_str(end,PRIMARY_QUERY);
    result.replace_range(start..start+signature.len(),"vec2 rs_uncached_world(");
    if result.matches("void main(void){").count()!=1 {
        return Err("常驻源着色器主函数入口不匹配".into());
    }
    result=result.replace("void main(void){", "void main(void){rs_first_world=true;rs_cache_hit=vec4(0.);rs_cache_id=vec4(0.);")
        .replace("gl_FragColor","rs_color");
    // The paged path resumes instead of silently truncating long queries.
    // Resident queries require no CPU checkpoint, so retain the interval's
    // actual termination condition without the old dense iteration cap.
    Ok(result.replace("texture2DLod(mipmap,", "rs_texture_mipmap(")
        .replace(traversal, "while(e.x<e.y)"))
}

/// Retain the aligned core from the existing conservative halo hierarchy.
/// Halo samples have already contributed to the 3x3 reduction, so cropping
/// these levels preserves cross-page bounds without storing duplicate halos.
pub(super) fn packed_level(level: &MipLevel) -> Vec<f32> {
    assert!(level.level < 5, "Only low source levels have packed pages");
    let edge = (512 >> level.level) as usize;
    let border = (64 >> level.level) as usize;
    let stride = level.edge as usize;
    assert_eq!(stride, edge + 2 * border);
    assert_eq!(level.values.len(), stride * stride);
    let mut packed = Vec::with_capacity(edge * edge);
    for row in border..border + edge {
        packed.extend_from_slice(&level.values[row * stride + border..row * stride + border + edge]);
    }
    packed
}
