use super::HeightMap;
use std::path::Path;

pub fn demo() -> HeightMap {
    let size = 128;
    let mut values = Vec::with_capacity(size * size);
    for y in 0..size {
        for x in 0..size {
            let u = (x as f32 / (size - 1) as f32 - 0.5) * 2.;
            let v = (y as f32 / (size - 1) as f32 - 0.5) * 2.;
            let ridge = (u * 8. + v * 3.).sin() * (v * 7.).cos() * 0.08;
            values.push(((1. - u * u - v * v).max(0.).powf(2.) * (0.65 + ridge)).max(0.));
        }
    }
    HeightMap {
        width: size as u32,
        height: size as u32,
        values,
        display_range: [0., 1.],
        source_elevation_range: None,
        bottom: 0.,
        span: 60.,
        name: "内置地形".into(),
        source: None,
    }
}
fn dimensions(w: u32, h: u32) -> Result<(), String> {
    super::raster::dimensions(w, h)
}
pub fn load(path: &Path) -> Result<HeightMap, String> {
    super::raster::load_preview(path)
}
fn normalize(
    width: u32,
    height: u32,
    values: Vec<f32>,
    cell: f32,
    name: String,
) -> Result<HeightMap, String> {
    dimensions(width, height)?;
    let extrema=values.iter().copied().map(|v|if v.is_finite(){v.max(0.)}else{0.})
        .fold([f32::INFINITY,f32::NEG_INFINITY],|r,v|[r[0].min(v),r[1].max(v)]);
    let values: Vec<f32> = values
        .into_iter()
        .map(|v| if v.is_finite() { v.max(0.) / cell } else { 0. })
        .collect();
    let bottom = values.iter().copied().fold(f32::INFINITY, f32::min);
    let span = (values.iter().copied().fold(f32::NEG_INFINITY, f32::max) - bottom).max(1e-6);
    let span = if span <= 1e-6 { 1. } else { span };
    let values = values
        .into_iter()
        .map(|v| {
            let q = (v - bottom) / span;
            if q <= 0. {
                0.
            } else {
                q.max(1. / 65536.)
            }
        })
        .collect();
    Ok(HeightMap {
        width,
        height,
        values,
        display_range: [0., 1.],
        source_elevation_range: Some(extrema.map(f64::from)),
        bottom,
        span,
        name,
        source: None,
    })
}
pub fn parse_asc(text: &str, name: String) -> Result<HeightMap, String> {
    let mut width = None;
    let mut height = None;
    let mut cell = 1.;
    let mut nodata = None;
    let mut data = Vec::new();
    let mut started = false;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let tokens: Vec<_> = line.split_whitespace().collect();
        let key = tokens[0].to_ascii_lowercase();
        if !started
            && matches!(
                key.as_str(),
                "ncols"
                    | "nrows"
                    | "xllcorner"
                    | "yllcorner"
                    | "xllcenter"
                    | "yllcenter"
                    | "cellsize"
                    | "nodata_value"
            )
        {
            let value = *tokens.get(1).ok_or("ASC 头缺少值")?;
            match key.as_str() {
                "ncols" => width = Some(value.parse::<u32>().map_err(|_| "ncols 无效")?),
                "nrows" => height = Some(value.parse::<u32>().map_err(|_| "nrows 无效")?),
                "cellsize" => cell = value.parse::<f32>().map_err(|_| "cellsize 无效")?,
                "nodata_value" => nodata = Some(value.parse::<f32>().map_err(|_| "NoData 无效")?),
                _ => {}
            }
        } else {
            started = true;
            for token in tokens {
                let v = token.parse::<f32>().map_err(|_| "ASC 含无效数值")?;
                data.push(if Some(v) == nodata { 0. } else { v });
            }
        }
    }
    let (w, h) = (width.ok_or("缺少 ncols")?, height.ok_or("缺少 nrows")?);
    dimensions(w, h)?;
    if !cell.is_finite() || cell <= 0. || data.len() != w as usize * h as usize {
        return Err("ASC 网格数量或 cellsize 无效".into());
    }
    normalize(w, h, data, cell, name)
}

