use super::RenderParams;

pub(crate) fn validate(p: &RenderParams) -> Result<(), String> {
    if p.appearance_layers_version > 1 { return Err("不支持的外观设置版本".into()); }
    if !["sky","solid","gradient"].contains(&p.background_mode.as_str())
        || !["aligned","planar","triplanar"].contains(&p.texture_mapping.as_str())
        || !["fill","contain","repeat"].contains(&p.texture_fit.as_str())
        || !["overlay","normal","multiply","softlight"].contains(&p.texture_blend.as_str())
        || !["source","custom"].contains(&p.ramp_range_mode.as_str()) {
        return Err("无效的背景或外观模式".into());
    }
    for color in [&p.background_color,&p.background_color2,&p.ground_color,&p.material_specular_color]
        .into_iter().chain(p.ramp_stops.iter().map(|stop|&stop.color)) {
        if color.len()!=7 || !color.starts_with('#') || !color.as_bytes()[1..].iter().all(|byte|byte.is_ascii_hexdigit()) {
            return Err("颜色格式无效".into());
        }
    }
    for (value,low,high) in [
        (p.background_angle,-360.,360.),(p.texture_scale,0.01,100.),
        (p.texture_offset_x,-100.,100.),(p.texture_offset_y,-100.,100.),
        (p.texture_rotation,-360.,360.),(p.material_roughness,0.02,1.),
        (p.material_specular,0.,1.),(p.ramp_offset,-1.,1.),
    ] {
        if !value.is_finite() || !(low..=high).contains(&value) {
            return Err("背景或外观参数超出范围".into());
        }
    }
    if p.ramp_stops.len()>16 || p.ramp_stops.len()==1
        || p.ramp_stops.iter().any(|stop|!stop.position.is_finite() || !(0. ..=1.).contains(&stop.position))
        || p.ramp_stops.windows(2).any(|pair|pair[0].position>=pair[1].position)
        || !p.ramp_min.is_finite() || !p.ramp_max.is_finite()
        || p.ramp_min.abs()>1e12 || p.ramp_max.abs()>1e12
        || (p.ramp_range_mode=="custom" && p.ramp_max<=p.ramp_min) {
        return Err("色带位置或高程范围无效".into());
    }
    Ok(())
}
