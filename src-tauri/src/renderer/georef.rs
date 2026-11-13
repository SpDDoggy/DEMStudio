//! Pixel-corner georeferencing; no reprojection is performed here.
use std::{
    fs,
    io::{Read, Seek, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
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
    /// A source CRS definition, independent of its affine pixel grid. Free-form
    /// GeoAscii citations are not coordinate-system definitions.
    pub fn has_crs(&self) -> bool {
        self.has_embedded_crs() || self.wkt.as_deref().is_some_and(valid_wkt)
    }
    pub(super) fn has_embedded_crs(&self) -> bool {
        valid_geokeys(self) && [3072, 2048].into_iter()
            .any(|id| inline_key(&self.keys, id).is_some_and(|v| v != 0 && v != 32767))
    }
    fn pam_srs(&self) -> Option<String> {
        if let Some(wkt) = self.wkt.as_deref().filter(|w| valid_wkt(w)) {
            return Some(wkt.trim().to_owned());
        }
        if self.has_embedded_crs() {
            return [3072, 2048].into_iter().find_map(|id|
                inline_key(&self.keys, id).filter(|v| *v != 0 && *v != 32767)
                    .map(|code| format!("EPSG:{code}")));
        }
        None
    }
    /// Scale the complete source footprint onto the unpadded output grid, then
    /// shift its origin by output-pixel margins. Grid dimensions exclude margins.
    pub fn resized_grid(
        &self, source_width: u32, source_height: u32,
        grid_width: u32, grid_height: u32, left: u32, top: u32,
    ) -> Result<Self, String> {
        if [source_width, source_height, grid_width, grid_height].contains(&0) || !self.valid() {
            return Err("地理输出网格无效".into());
        }
        let mut geo = self.clone();
        let sx = f64::from(source_width) / f64::from(grid_width);
        let sy = f64::from(source_height) / f64::from(grid_height);
        geo.transform[1] *= sx;
        geo.transform[4] *= sx;
        geo.transform[2] *= sy;
        geo.transform[5] *= sy;
        geo.shifted_pixels(-i64::from(left), -i64::from(top))
    }
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
        let determinant = self.transform[1] * self.transform[5] - self.transform[2] * self.transform[4];
        self.transform.iter().all(|v| v.is_finite())
            && determinant.is_finite() && determinant != 0.
    }
}

fn valid_geokeys(geo: &GeoReference) -> bool {
    let keys = &geo.keys;
    if keys.len() < 4 || keys[0] != 1 { return false; }
    let end = 4 + usize::from(keys[3]) * 4;
    let Some(entries) = keys.get(4..end) else { return false; };
    entries.chunks_exact(4).all(|key| {
        let count = usize::from(key[2]);
        let offset = usize::from(key[3]);
        count > 0 && match key[1] {
            0 => count == 1,
            34735 => offset >= end && offset + count <= keys.len(),
            34736 => offset + count <= geo.doubles.len()
                && geo.doubles[offset..offset + count].iter().all(|v| v.is_finite()),
            34737 => offset + count <= geo.ascii.len() + 1,
            _ => false,
        }
    })
}

// Retain source WKT without a projection-library dependency. Check its root and
// balanced syntax; this does not resolve CRS authorities or reproject anything.
fn valid_wkt(value: &str) -> bool {
    let value = value.trim();
    let Some(open) = value.find('[') else { return false; };
    let root = value[..open].trim().to_ascii_uppercase();
    if !matches!(root.as_str(),
        "PROJCS" | "GEOGCS" | "GEOCCS" | "LOCAL_CS" | "COMPD_CS" | "VERT_CS"
        | "PROJCRS" | "GEOGCRS" | "GEODCRS" | "VERTCRS" | "ENGCRS"
        | "COMPOUNDCRS" | "BOUNDCRS" | "DERIVEDPROJCRS") { return false; }
    let body = value[open + 1..].trim_start();
    if root == "BOUNDCRS" {
        let Some(source_open) = body.find('[') else { return false; };
        if !body[..source_open].trim().eq_ignore_ascii_case("SOURCECRS") { return false; }
    } else if !body.starts_with('"') { return false; }
    let mut depth = 0i32;
    let mut quoted = false;
    let mut chars = value[open..].chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_control() && !c.is_whitespace() { return false; }
        if c == '"' {
            if quoted && chars.peek() == Some(&'"') { chars.next(); }
            else { quoted = !quoted; }
        } else if !quoted {
            match c {
                '[' => depth += 1,
                ']' => { depth -= 1; if depth == 0 { return chars.all(char::is_whitespace); } },
                _ => {}
            }
            if depth < 0 { return false; }
        }
    }
    false
}

// GDAL's PAM loader expects the dataset node first; a leading XML comment can
// make it ignore the entire file. Keep ownership inside that root element.
const PAM_MARKER: &str = "<PAMDataset>\n<!-- DEMStudio georeference v1 -->\n";
static NEXT_AUX: AtomicU64 = AtomicU64::new(0);

fn aux_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".aux.xml");
    PathBuf::from(name)
}

/// Call before replacing the output image. Unowned location files must never be
/// deleted or silently inherited by an output whose geography was disabled.
pub fn validate_output_sidecars(path: &Path, writing_geo: bool) -> Result<(), String> {
    let ext = path.extension().unwrap_or_default().to_string_lossy().to_ascii_lowercase();
    let short = match ext.as_str() { "png" => "pgw", "tif" | "tiff" => "tfw", _ => "wld" };
    if !writing_geo {
        if let Some(world) = sidecar(path, &[short.into(), format!("{ext}w"), "wld".into()]) {
            return Err(format!("输出路径已有地理辅助文件，请更换文件名：{}", world.display()));
        }
    }
    let aux = aux_path(path);
    match fs::read_to_string(&aux) {
        Ok(value) if value.starts_with(PAM_MARKER) => Ok(()),
        Ok(_) => Err(format!("输出路径已有外部地理辅助文件，请更换文件名：{}", aux.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("无法检查地理辅助文件：{e}")),
    }
}

/// Remove only the PAM metadata created by this application, after geo-off
/// image output succeeds. Unknown sidecars are reported, never removed.
pub fn clear_output_aux(path: &Path) -> Result<(), String> {
    validate_output_sidecars(path, false)?;
    match fs::remove_file(aux_path(path)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("无法清理原输出地理辅助文件：{e}")),
    }
}

fn xml_text(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
        .replace('"', "&quot;").replace('\'', "&apos;")
}

fn xml_unescape(value: &str) -> Result<String, String> {
    let mut result = String::new();
    let mut rest = value;
    while let Some(index) = rest.find('&') {
        result.push_str(&rest[..index]);
        rest = &rest[index..];
        let Some(end) = rest.find(';') else { return Err("地理辅助文件 XML 转义无效".into()); };
        result.push(match &rest[..=end] {
            "&amp;" => '&', "&lt;" => '<', "&gt;" => '>', "&quot;" => '"', "&apos;" => '\'',
            _ => return Err("地理辅助文件 XML 转义无效".into()),
        });
        rest = &rest[end + 1..];
    }
    result.push_str(rest);
    Ok(result)
}
fn pam_field<'a>(xml: &'a str, open: &str, close: &str) -> Result<&'a str, String> {
    let (_, tail) = xml.split_once(open).ok_or("地理辅助文件缺少字段")?;
    let (field, after) = tail.split_once(close).ok_or("地理辅助文件字段无效")?;
    if field.contains('<') || after.contains(open) { return Err("地理辅助文件字段无效".into()); }
    Ok(field)
}

/// Read only this application's PAM shape. External PAM belongs to its writer;
/// ordinary world/PRJ sidecars continue through the existing import path.
pub fn read_aux(path: &Path) -> Result<Option<GeoReference>, String> {
    let xml = match fs::read_to_string(aux_path(path)) {
        Ok(xml) => xml,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("读取地理辅助文件失败：{e}")),
    };
    if !xml.starts_with(PAM_MARKER) { return Ok(None); }
    let srs = xml_unescape(pam_field(&xml, "<SRS>", "</SRS>")?)?;
    let transform = pam_field(&xml, "<GeoTransform>", "</GeoTransform>")?.split(',')
        .map(|v| v.trim().parse::<f64>().map_err(|_| "地理辅助文件仿射变换无效".to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    let transform: [f64; 6] = transform.try_into().map_err(|_| "地理辅助文件应包含六项仿射变换")?;
    let (keys, wkt) = if let Some(code) = srs.strip_prefix("EPSG:") {
        let code = code.parse::<u16>().ok().filter(|v| *v != 0 && *v != 32767)
            .ok_or("地理辅助文件 EPSG 无效")?;
        let kind = pam_field(&xml, "<MDI key=\"CrsGeoKey\">", "</MDI>")?
            .parse::<u16>().map_err(|_| "地理辅助文件坐标系类型无效")?;
        if ![2048, 3072].contains(&kind) { return Err("地理辅助文件坐标系类型无效".into()); }
        (vec![1,1,0,3,1024,0,1,if kind == 3072 {1} else {2},1025,0,1,1,kind,0,1,code], None)
    } else if valid_wkt(&srs) {
        (keys_from_wkt(&srs), Some(srs))
    } else { return Err("地理辅助文件坐标系无效".into()); };
    let geo = GeoReference { transform, keys, doubles: vec![], ascii: String::new(), wkt };
    if !geo.valid() || !geo.has_crs() { return Err("地理辅助文件地理定位无效".into()); }
    Ok(Some(geo))
}

struct PendingAux(PathBuf);
impl Drop for PendingAux { fn drop(&mut self) { let _ = fs::remove_file(&self.0); } }

/// Standard GDAL PAM for PNG or TIFF; the full image suffix is retained.
/// EPSG identifiers are SRS user input here, never mislabelled as PRJ WKT.
pub fn write_png_aux(path: &Path, geo: &GeoReference) -> Result<PathBuf, String> {
    validate_output_sidecars(path, true)?;
    if !geo.valid() { return Err("地理仿射变换无效".into()); }
    let srs = geo.pam_srs().ok_or("源图像未定义可写出的地理坐标系")?;
    let transform = geo.transform.iter().map(|v| format!("{v:.17e}")).collect::<Vec<_>>().join(", ");
    let kind = if srs.starts_with("EPSG:") {
        let key = if inline_key(&geo.keys, 3072).is_some_and(|v| v != 0 && v != 32767) {3072} else {2048};
        format!("  <Metadata domain=\"DEMStudio\"><MDI key=\"CrsGeoKey\">{key}</MDI></Metadata>\n")
    } else { String::new() };
    let xml = format!("{PAM_MARKER}  <SRS>{}</SRS>\n  <GeoTransform>{transform}</GeoTransform>\n{kind}</PAMDataset>\n", xml_text(&srs));
    let aux = aux_path(path);
    let parent = aux.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = aux.file_name().ok_or("地理辅助文件名无效")?.to_string_lossy();
    for _ in 0..32 {
        let temporary = parent.join(format!(".{name}.{}.{}.part", std::process::id(), NEXT_AUX.fetch_add(1, Ordering::Relaxed)));
        match fs::OpenOptions::new().write(true).create_new(true).open(&temporary) {
            Ok(mut file) => {
                let pending = PendingAux(temporary);
                file.write_all(xml.as_bytes()).map_err(|e| format!("写入地理辅助文件失败：{e}"))?;
                file.sync_all().map_err(|e| format!("保存地理辅助文件失败：{e}"))?;
                drop(file);
                fs::rename(&pending.0, &aux).map_err(|e| format!("完成地理辅助文件失败：{e}"))?;
                return Ok(aux);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {},
            Err(e) => return Err(format!("无法创建地理辅助文件：{e}")),
        }
    }
    Err("无法分配地理辅助文件".into())
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

/// Embedded CRS remains authoritative. Area/affine-only TIFF output can use
/// this application's complete PAM grid and WKT, including resized exports.
pub fn read_tiff_with_sidecars<R: Read + Seek>(
    decoder: &mut Decoder<R>, path: &Path,
) -> Result<Option<GeoReference>, String> {
    match read_tiff(decoder)? {
        Some(geo) if geo.has_crs() => Ok(Some(geo)),
        Some(geo) => Ok(read_aux(path)?.or(Some(geo))),
        None => read_sidecars(path),
    }
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
    if let Some(geo) = read_aux(path)? { return Ok(Some(geo)); }
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


