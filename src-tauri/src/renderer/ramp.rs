use super::RenderParams;

pub(super) const HEADER: &str = r#"
uniform int ramp_stop_count,ramp_reverse,ramp_wrap,ramp_custom_range,ramp_numeric;
uniform float ramp_positions[16],ramp_offset;
uniform vec3 ramp_stop_colors[16];
uniform vec2 ramp_source_range,ramp_user_range;
float ramp_coordinate(float t){
    if(ramp_custom_range!=0){
        float q=ramp_numeric!=0?clamp((t-1e-6)/(1.-1e-6),0.,1.):t;
        t=(ramp_source_range.x+q*ramp_source_range.y-ramp_user_range.x)/max(ramp_user_range.y,1e-12);
    }
    if(ramp_reverse!=0)t=1.-t;
    t-=ramp_offset;
    return ramp_wrap!=0?fract(t):clamp(t,0.,1.);
}
vec3 ramp_stops_color(float t,int discrete){
    if(t<=ramp_positions[0])return ramp_stop_colors[0];
    for(int i=1;i<16;++i){
        if(i>=ramp_stop_count)break;
        if(t<ramp_positions[i]){
            if(discrete!=0)return ramp_stop_colors[i-1];
            float q=(t-ramp_positions[i-1])/max(ramp_positions[i]-ramp_positions[i-1],1e-12);
            return mix(ramp_stop_colors[i-1],ramp_stop_colors[i],q);
        }
    }
    return ramp_stop_colors[max(ramp_stop_count-1,0)];
}
"#;

pub(super) fn source(original: &str) -> String {
    original.replace("vec3 elevation_color(float t){",&format!("{HEADER}\nvec3 elevation_color(float t){{\nt=ramp_coordinate(t);if(ramp_stop_count>=2)return ramp_stops_color(t,appearance_discrete);"))
}

/// Positions and colors are validated before this projection to GLSL uniforms.
pub(super) fn count(p: &RenderParams) -> i32 { p.ramp_stops.len() as i32 }
