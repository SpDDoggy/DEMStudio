//! Pixel-corner georeferencing; no reprojection is performed here.
use std::{
    fs,
    io::{Read, Seek},
    path::{Path, PathBuf},
};
use tiff::{decoder::Decoder, tags::Tag};

#[derive(Clone, Debug, serde::Serialize)]
pub struct GeoReference {
    /// GDAL order: origin X, column X, row X, origin Y, column Y, row Y.
    pub transform: [f64; 6],
    pub keys: Vec<u16>,
    pub doubles: Vec<f64>,
    pub ascii: String,
    pub wkt: Option<String>,
}
impl GeoReference {
    pub fn crs_label(&self) -> String {
        for id in [3072, 2048] {
            if let Some(code) = inline_key(&self.keys, id).filter(|c| *c != 32767 && *c != 0) {
                return format!("EPSG:{code}");
            }
        }
        if let Some(wkt) = &self.wkt {
            return wkt.clone();
        }
        self.ascii.trim_matches(['|', '\0']).to_owned()
    }
    pub fn pixel_is_point(&self) -> bool {
        inline_key(&self.keys, 1025) == Some(2)
    }
    /// Move the grid origin by whole source pixels without changing pixel basis or CRS.
    /// Offsets are relative to this corner-based transform, including for Point sources.
    pub fn shifted_pixels(&self, column_offset: i64, row_offset: i64) -> Result<Self, String> {
        let mut shifted = self.clone();
        shifted.transform[0] +=
            column_offset as f64 * self.transform[1] + row_offset as f64 * self.transform[2];
        shifted.transform[3] +=
            column_offset as f64 * self.transform[4] + row_offset as f64 * self.transform[5];
        if !shifted.valid() {
            return Err("扩展范围的地理定位无效".into());
        }
        Ok(shifted)
    }
    /// Approximate local horizontal metres for a source pixel column. Unknown units stay unknown.
    pub fn pixel_meters(&self, width: u32, height: u32) -> Option<f64> {
        let projected = inline_key(&self.keys, 3072);
        let geographic = inline_key(&self.keys, 2048);
        let t = self.transform;
        if projected.is_some() || inline_key(&self.keys, 1024) == Some(1) {
            let units = inline_key(&self.keys, 3076);
            let factor = match units {
                Some(9001) => 1.,
                Some(9002) => 0.3048,
                Some(9003) => 1200. / 3937.,
                None if projected.is_some_and(|v| {
                    (32601..=32660).contains(&v) || (32701..=32760).contains(&v) || v == 3857
                }) =>
                {
                    1.
                }
                _ => return None,
            };
            return Some(t[1].hypot(t[4]) * factor);
        }
        if geographic.is_some() || inline_key(&self.keys, 1024) == Some(2) {
            let radians = match inline_key(&self.keys, 2054) {
                Some(9101) => 1.,
                Some(9102) => std::f64::consts::PI / 180.,
                Some(9103) => std::f64::consts::PI / 10800.,
                Some(9104) => std::f64::consts::PI / 648000.,
                Some(9105 | 9106) => std::f64::consts::PI / 200.,
                None if geographic == Some(4326) => std::f64::consts::PI / 180.,
                _ => return None,
            };
            let lat = (t[3] + width as f64 * 0.5 * t[4] + height as f64 * 0.5 * t[5]) * radians;
            if lat.abs() > std::f64::consts::FRAC_PI_2 {
                return None;
            }
            // WGS84 tangent metric is a local display approximation, never an affine-coordinate rewrite.
            let e2 = 0.006_694_379_990_141_316_5;
            let d = 1. - e2 * lat.sin().powi(2);
            let meridian = 6_378_137. * (1. - e2) / d.powf(1.5);
            let prime = 6_378_137. / d.sqrt();
            return Some((t[1] * radians * prime * lat.cos()).hypot(t[4] * radians * meridian));
        }
        None
    }
    pub fn vertical_meters(&self) -> Option<f64> {
        match inline_key(&self.keys, 4099) {
            None | Some(9001) => Some(1.),
            Some(9002) => Some(0.3048),
            Some(9003) => Some(1200. / 3937.),
            _ => None,
        }
    }
    pub fn valid(&self) -> bool {
        self.transform.iter().all(|v| v.is_finite())
            && (self.transform[1] * self.transform[5] - self.transform[2] * self.transform[4]).abs()
                > 0.
    }
}
fn inline_key(keys: &[u16], id: u16) -> Option<u16> {
    let count = usize::from(*keys.get(3)?);
    keys.get(4..4 + count * 4)?
        .chunks_exact(4)
        .find(|k| k[0] == id && k[1] == 0 && k[2] == 1)
        .map(|k| k[3])
}
pub fn read_tiff<R: Read + Seek>(decoder: &mut Decoder<R>) -> Result<Option<GeoReference>, String> {
    let keys = decoder
        .get_tag_u16_vec(Tag::GeoKeyDirectoryTag)
        .unwrap_or_default();
    let doubles = decoder
        .get_tag_f64_vec(Tag::GeoDoubleParamsTag)
        .unwrap_or_default();
    let ascii = decoder
        .get_tag_ascii_string(Tag::GeoAsciiParamsTag)
        .unwrap_or_default();
    let matrix = decoder.get_tag_f64_vec(Tag::ModelTransformationTag).ok();
    let scale = decoder.get_tag_f64_vec(Tag::ModelPixelScaleTag).ok();
    let tie = decoder.get_tag_f64_vec(Tag::ModelTiepointTag).ok();
    let transform = if let Some(m) = matrix {
        if m.len() != 16 || m[12] != 0. || m[13] != 0. || m[15] != 1. {
            return Err("GeoTIFF 仿射矩阵无效或含透视分量".into());
        }
        [m[3], m[0], m[1], m[7], m[4], m[5]]
    } else if let (Some(s), Some(t)) = (scale, tie) {
        if s.len() < 2 || t.len() < 6 {
            return Err("GeoTIFF 像元尺度或锚点无效".into());
        }
        [t[3] - t[0] * s[0], s[0], 0., t[4] + t[1] * s[1], 0., -s[1]]
    } else {
        return Ok(None);
    };
    let mut geo = GeoReference {
        transform,
        keys,
        doubles,
        ascii,
        wkt: None,
    };
    if geo.pixel_is_point() {
        geo.transform[0] -= (geo.transform[1] + geo.transform[2]) * 0.5;
        geo.transform[3] -= (geo.transform[4] + geo.transform[5]) * 0.5;
    }
    if !geo.valid() {
        return Err("GeoTIFF 地理定位无效".into());
    }
    Ok(Some(geo))
}
fn sidecar(path: &Path, extensions: &[String]) -> Option<PathBuf> {
    for ext in extensions {
        let candidate = path.with_extension(ext);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    // Windows is case-insensitive, but this also keeps test fixtures portable.
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let stem = path.file_stem()?.to_str()?;
    fs::read_dir(parent)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| {
            p.file_stem()
                .and_then(|v| v.to_str())
                .is_some_and(|s| s.eq_ignore_ascii_case(stem))
                && p.extension()
                    .and_then(|v| v.to_str())
                    .is_some_and(|x| extensions.iter().any(|e| e.eq_ignore_ascii_case(x)))
        })
}
pub fn read_sidecars(path: &Path) -> Result<Option<GeoReference>, String> {
    let ext = path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    let short = match ext.as_str() {
        "jpg" | "jpeg" => "jgw",
        "tif" | "tiff" => "tfw",
        "png" => "pgw",
        _ => "wld",
    };
    let Some(world) = sidecar(path, &[short.into(), format!("{ext}w"), "wld".into()]) else {
        return Ok(None);
    };
    let values = fs::read_to_string(world)
        .map_err(|e| e.to_string())?
        .split_whitespace()
        .map(str::parse::<f64>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "世界文件含无效数字")?;
    if values.len() != 6 {
        return Err("世界文件应包含 6 个数字".into());
    }
    let [a, d, b, e, c, f] = values.try_into().unwrap();
    let wkt = sidecar(path, &["prj".into()])
        .map(fs::read_to_string)
        .transpose()
        .map_err(|e| e.to_string())?
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    let keys = wkt.as_ref().map(|w| keys_from_wkt(w)).unwrap_or_default();
    let geo = GeoReference {
        transform: [c - (a + b) * 0.5, a, b, f - (d + e) * 0.5, d, e],
        keys,
        doubles: vec![],
        ascii: String::new(),
        wkt,
    };
    if !geo.valid() {
        return Err("世界文件地理定位无效".into());
    }
    Ok(Some(geo))
}
fn keys_from_wkt(wkt: &str) -> Vec<u16> {
    let compact: String = wkt.chars().filter(|c| !c.is_whitespace()).collect();
    let upper = compact.to_ascii_uppercase();
    // Only accept a top-level CRS authority (the final authority), never a unit/datum code.
    for marker in ["AUTHORITY[\"EPSG\",", "ID[\"EPSG\","] {
        if let Some(index) = upper.rfind(marker) {
            let mut depth = 0i32;
            let mut quoted = false;
            for ch in upper[..index].chars() {
                match ch {
                    '"' => quoted = !quoted,
                    '[' if !quoted => depth += 1,
                    ']' if !quoted => depth -= 1,
                    _ => {}
                }
            }
            if depth != 1 {
                continue;
            }
            let tail = &upper[index + marker.len()..];
            let tail = tail.trim_start_matches('"');
            let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
            let after = tail[digits.len()..].trim_start_matches('"');
            if !after.is_empty() && after.chars().all(|c| c == ']') {
                if let Ok(code) = digits.parse::<u16>() {
                    if code != 0 && code != 32767 {
                        let projected =
                            upper.starts_with("PROJCS[") || upper.starts_with("PROJCRS[");
                        let geographic = upper.starts_with("GEOGCS[")
                            || upper.starts_with("GEOGCRS[")
                            || upper.starts_with("GEODCRS[");
                        if projected || geographic {
                            return vec![
                                1,
                                1,
                                0,
                                3,
                                1024,
                                0,
                                1,
                                if projected { 1 } else { 2 },
                                1025,
                                0,
                                1,
                                1,
                                if projected { 3072 } else { 2048 },
                                0,
                                1,
                                code,
                            ];
                        }
                    }
                }
            }
        }
    }
    vec![]
}


