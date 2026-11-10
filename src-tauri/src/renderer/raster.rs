//! Bounded terrain preview. PNG uses scanlines, TIFF storage chunks, JPEG full decode.
//! JPEG keeps source-wide extrema before area reduction; decoded source pixels are temporary.
use super::{
    georef::{self, GeoReference},
    HeightMap,
};
use std::{
    fs::File,
    io::{BufReader, Read},
    path::{Path, PathBuf},
};
use tiff::{
    decoder::{Decoder, DecodingResult},
    tags::Tag,
};

pub const MAX_EDGE: u32 = 81_200;
pub const PREVIEW_EDGE: u32 = 2_048;
// Source dimensions are independent of the dense height texture requested by a caller.
// Raising the source limit must never enable a 26 GiB full-image float allocation.
pub const MAX_DECODE_EDGE: u32 = 16_384;
const TIFF_CHUNK_BYTES: usize = 256 * 1024 * 1024;
const JPEG_PIXELS: u64 = 16_384 * 16_384;
pub(super) const JPEG_DECODE_BYTES: usize = 1024 * 1024 * 1024;
pub(super) const ASC_TEXT_BYTES: u64 = 256 * 1024 * 1024;
#[derive(Clone, Debug, serde::Serialize)]
pub struct RasterSource {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub geo: Option<GeoReference>,
}
pub fn dimensions(w: u32, h: u32) -> Result<(), String> {
    if w < 2 || h < 2 || w > MAX_EDGE || h > MAX_EDGE {
        return Err(format!(
            "图像尺寸 {w}×{h} 不受支持（此格式范围 2×2 至 {MAX_EDGE}×{MAX_EDGE}）"
        ));
    }
    Ok(())
}
fn preview_size(w: u32, h: u32, max: u32) -> (u32, u32) {
    let ratio = (max as f64 / w.max(h) as f64).min(1.);
    (
        ((w as f64 * ratio).round() as u32).max(2),
        ((h as f64 * ratio).round() as u32).max(2),
    )
}
#[derive(Clone, Copy)]
struct Coverage {
    first: usize,
    first_weight: f64,
    second_weight: f64,
}
fn pixel_coverage(source: u32, output: u32, pixel: u32) -> Coverage {
    // Integer boundaries avoid drifting across pixel edges at non-integer scale ratios.
    let start = u64::from(pixel) * u64::from(output);
    let end = start + u64::from(output);
    let first = start / u64::from(source);
    let boundary = (first + 1) * u64::from(source);
    Coverage {
        first: first as usize,
        first_weight: (end.min(boundary) - start) as f64 / output as f64,
        second_weight: end.saturating_sub(boundary) as f64 / output as f64,
    }
}
fn coverage(source: u32, output: u32) -> Vec<Coverage> {
    (0..source)
        .map(|pixel| pixel_coverage(source, output, pixel))
        .collect()
}
struct AxisCoverage {
    source: u32,
    output: u32,
    table: Option<Vec<Coverage>>,
}
impl AxisCoverage {
    fn new(source: u32, output: u32) -> Self {
        Self {
            source,
            output,
            table: (source <= MAX_EDGE).then(|| coverage(source, output)),
        }
    }
    fn at(&self, pixel: u32) -> Coverage {
        self.table.as_ref().map_or_else(
            || pixel_coverage(self.source, self.output, pixel),
            |table| table[pixel as usize],
        )
    }
}
/// Allocated only for a smaller preview. All sums stay in source-value units until normalization.
struct AreaPreview {
    width: usize,
    origin: f64,
    x: AxisCoverage,
    y: AxisCoverage,
    sums: Vec<f64>,
    weights: Vec<f64>,
}
impl AreaPreview {
    fn new(w: u32, h: u32, pw: u32, ph: u32) -> Option<Self> {
        if (w, h) == (pw, ph) {
            return None;
        }
        Some(Self {
            width: pw as usize,
            origin: f64::NAN,
            x: AxisCoverage::new(w, pw),
            y: AxisCoverage::new(h, ph),
            sums: vec![0.; pw as usize * ph as usize],
            weights: vec![0.; pw as usize * ph as usize],
        })
    }
    fn add(&mut self, x: u32, y: u32, value: f64) {
        if !value.is_finite() {
            return;
        }
        // Sum differences, not huge absolute elevations: millions of samples at
        // a large float64 offset must retain their small relief differences.
        if self.origin.is_nan() {
            self.origin = value;
        }
        let value = value - self.origin;
        let xc = self.x.at(x);
        let yc = self.y.at(y);
        let base = yc.first * self.width + xc.first;
        let weight = xc.first_weight * yc.first_weight;
        self.sums[base] += value * weight;
        self.weights[base] += weight;
        if xc.second_weight > 0. {
            let weight = xc.second_weight * yc.first_weight;
            self.sums[base + 1] += value * weight;
            self.weights[base + 1] += weight;
        }
        if yc.second_weight > 0. {
            let weight = xc.first_weight * yc.second_weight;
            self.sums[base + self.width] += value * weight;
            self.weights[base + self.width] += weight;
            if xc.second_weight > 0. {
                let weight = xc.second_weight * yc.second_weight;
                self.sums[base + self.width + 1] += value * weight;
                self.weights[base + self.width + 1] += weight;
            }
        }
    }
    fn means(mut self) -> Vec<f64> {
        for (sum, weight) in self.sums.iter_mut().zip(self.weights) {
            *sum = if weight > 0. {
                *sum / weight + self.origin
            } else {
                f64::NAN
            };
        }
        self.sums
    }
    fn finish(self, output: &mut [f32]) {
        for (out, value) in output.iter_mut().zip(self.means()) {
            *out = if value.is_finite() { value as f32 } else { 0. };
        }
    }
}
fn empty(path: &Path, w: u32, h: u32, max: u32, geo: Option<GeoReference>) -> HeightMap {
    let (pw, ph) = preview_size(w, h, max);
    HeightMap {
        width: pw,
        height: ph,
        values: vec![0.; pw as usize * ph as usize],
        display_range: [0., 1.],
        source_elevation_range: None,
        bottom: 0.,
        span: 100. * pw.max(ph) as f32 / w.max(h) as f32,
        name: path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        source: Some(RasterSource {
            path: path.to_owned(),
            width: w,
            height: h,
            geo,
        }),
    }
}
pub fn load(path: &Path, max_edge: u32) -> Result<HeightMap, String> {
    if !(2..=MAX_DECODE_EDGE).contains(&max_edge) {
        return Err("预览尺寸无效".into());
    }
    load_with_limit(path, PreviewLimit::Explicit(max_edge))
}

/// Choose preview detail from source dimensions already read by its decoder.
/// Explicit `load(path, edge)` remains available for fixed export geometry.
pub fn load_preview(path: &Path) -> Result<HeightMap, String> {
    load_with_limit(path, PreviewLimit::Automatic)
}

#[derive(Clone, Copy)]
enum PreviewLimit {
    Explicit(u32),
    Automatic,
}
impl PreviewLimit {
    fn edge(self, width: u32, height: u32) -> u32 {
        match self {
            Self::Explicit(edge) => edge,
            Self::Automatic => {
                let longest = width.max(height);
                // ceil((longest / 4) / 2048) * 2048, without float rounding
                // or u32 overflow. preview_size also keeps both axes unscaled up.
                let tier = (u64::from(longest).div_ceil(8192) * 2048)
                    .clamp(u64::from(PREVIEW_EDGE), u64::from(MAX_DECODE_EDGE)) as u32;
                tier.min(longest).max(2)
            }
        }
    }
}

fn load_with_limit(path: &Path, limit: PreviewLimit) -> Result<HeightMap, String> {
    let ext = path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    match ext.as_str() {
        "png" => png(path, limit),
        "jpg" | "jpeg" => jpeg(path, limit),
        "tif" | "tiff" => tiff(path, limit),
        "asc" => {
            // ASC is the legacy dense-text parser, not the streaming PNG/TIFF path.
            let mut text = String::new();
            File::open(path)
                .map_err(|e| e.to_string())?
                .take(ASC_TEXT_BYTES + 1)
                .read_to_string(&mut text)
                .map_err(|e| e.to_string())?;
            if text.len() as u64 > ASC_TEXT_BYTES {
                return Err("ASC 文本超过 256 MiB，请转换为分块 TIFF".into());
            }
            let mut m = super::heightmap::parse_asc(
                &text,
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
            )?;
            let (w, h) = (m.width, m.height);
            let (pw, ph) = preview_size(w, h, limit.edge(w, h));
            if let Some(mut area) = AreaPreview::new(w, h, pw, ph) {
                for y in 0..h {
                    for x in 0..w {
                        area.add(
                            x,
                            y,
                            f64::from(m.values[y as usize * w as usize + x as usize]),
                        );
                    }
                }
                let mut values = vec![0.; pw as usize * ph as usize];
                area.finish(&mut values);
                m.values = values;
            }
            m.width = pw;
            m.height = ph;
            m.span *= pw.max(ph) as f32 / w.max(h) as f32;
            // Preserve any source metadata supplied by the ASC parser.
            if let Some(ref mut source) = m.source {
                source.path = path.to_owned();
            } else {
                m.source = Some(RasterSource {
                    path: path.to_owned(),
                    width: w,
                    height: h,
                    geo: georef::read_sidecars(path)?,
                });
            }
            Ok(m)
        }
        _ => Err("支持 JPG、PNG、TIFF 和 ASC".into()),
    }
}
fn png(path: &Path, limit: PreviewLimit) -> Result<HeightMap, String> {
    let mut decoder =
        png::Decoder::new(BufReader::new(File::open(path).map_err(|e| e.to_string())?));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let (w, h) = (reader.info().width, reader.info().height);
    dimensions(w, h)?;
    let interlaced = reader.info().interlaced;
    let (color, depth) = reader.output_color_type();
    let channels = color.samples();
    let bytes = if depth == png::BitDepth::Sixteen {
        2
    } else {
        1
    };
    if !matches!(depth, png::BitDepth::Eight | png::BitDepth::Sixteen) {
        return Err("PNG 位深未展开".into());
    }
    let mut map = empty(path, w, h, limit.edge(w, h), georef::read_sidecars(path)?);
    let mut area = AreaPreview::new(w, h, map.width, map.height);
    let mut range = [f32::INFINITY, f32::NEG_INFINITY];
    let passes: &[(u32, u32, u32, u32)] = if interlaced {
        &[
            (0, 0, 8, 8),
            (4, 0, 8, 8),
            (0, 4, 4, 8),
            (2, 0, 4, 4),
            (0, 2, 2, 4),
            (1, 0, 2, 2),
            (0, 1, 1, 2),
        ]
    } else {
        &[(0, 0, 1, 1)]
    };
    for &(sx, sy, dx, dy) in passes {
        if sx >= w || sy >= h {
            continue;
        }
        for y in (sy..h).step_by(dy as usize) {
            let row = reader
                .next_row()
                .map_err(|e| e.to_string())?
                .ok_or("PNG 行数不足")?;
            for (i, x) in (sx..w).step_by(dx as usize).enumerate() {
                let offset = i * channels * bytes;
                let value = |c: usize| -> f32 {
                    let p = offset + c * bytes;
                    if bytes == 2 {
                        u16::from_be_bytes([row.data()[p], row.data()[p + 1]]) as f32 / 65535.
                    } else {
                        row.data()[p] as f32 / 255.
                    }
                };
                let q = match color {
                    png::ColorType::Grayscale | png::ColorType::GrayscaleAlpha => value(0),
                    _ => 0.2126 * value(0) + 0.7152 * value(1) + 0.0722 * value(2),
                };
                range[0] = range[0].min(q);
                range[1] = range[1].max(q);
                if let Some(area) = &mut area {
                    area.add(x, y, f64::from(q));
                } else {
                    map.values[y as usize * map.width as usize + x as usize] = q;
                }
            }
        }
    }
    if reader.next_row().map_err(|e| e.to_string())?.is_some() {
        return Err("PNG 行数不符".into());
    }
    if let Some(area) = area {
        area.finish(&mut map.values);
    }
    map.display_range = range;
    Ok(map)
}
fn jpeg(path: &Path, limit: PreviewLimit) -> Result<HeightMap, String> {
    let mut decoder =
        jpeg_decoder::Decoder::new(BufReader::new(File::open(path).map_err(|e| e.to_string())?));
    decoder.set_max_decoding_buffer_size(JPEG_DECODE_BYTES);
    decoder.read_info().map_err(|e| e.to_string())?;
    let info = decoder.info().ok_or("JPEG 缺少尺寸")?;
    let (w, h) = (info.width as u32, info.height as u32);
    dimensions(w, h)?;
    jpeg_dimensions(w, h)?;
    let mut map = empty(path, w, h, limit.edge(w, h), georef::read_sidecars(path)?);
    // Decode source samples so extrema do not change with preview resolution.
    // JPEG remains lossy; PNG/TIFF preserve DEM samples.
    let pixels = decoder.decode().map_err(|e| e.to_string())?;
    let info = decoder.info().ok_or("JPEG 缺少图像信息")?;
    let mut area = AreaPreview::new(info.width as u32, info.height as u32, map.width, map.height);
    let channels = info.pixel_format.pixel_bytes();
    let mut range = [f32::INFINITY, f32::NEG_INFINITY];
    for y in 0..u32::from(info.height) {
        for x in 0..u32::from(info.width) {
            let i = (y as usize * info.width as usize + x as usize) * channels;
            let q = match info.pixel_format {
                jpeg_decoder::PixelFormat::L8 => pixels[i] as f32 / 255.,
                jpeg_decoder::PixelFormat::L16 => {
                    u16::from_ne_bytes([pixels[i], pixels[i + 1]]) as f32 / 65535.
                }
                jpeg_decoder::PixelFormat::RGB24 => {
                    (0.2126 * pixels[i] as f32
                        + 0.7152 * pixels[i + 1] as f32
                        + 0.0722 * pixels[i + 2] as f32)
                        / 255.
                }
                jpeg_decoder::PixelFormat::CMYK32 => {
                    let k = pixels[i + 3] as f32 / 255.;
                    (0.2126 * (1. - pixels[i] as f32 / 255.)
                        + 0.7152 * (1. - pixels[i + 1] as f32 / 255.)
                        + 0.0722 * (1. - pixels[i + 2] as f32 / 255.))
                        * (1. - k)
                }
            };
            range[0] = range[0].min(q);
            range[1] = range[1].max(q);
            if let Some(area) = &mut area {
                area.add(x, y, f64::from(q));
            } else {
                map.values[y as usize * map.width as usize + x as usize] = q;
            }
        }
    }
    if let Some(area) = area {
        area.finish(&mut map.values);
    }
    map.display_range = range;
    Ok(map)
}
fn tiff(path: &Path, limit: PreviewLimit) -> Result<HeightMap, String> {
    let mut limits = tiff::decoder::Limits::default();
    // A storage chunk may be one large strip. Bound it explicitly rather than disabling limits.
    limits.decoding_buffer_size = TIFF_CHUNK_BYTES;
    limits.intermediate_buffer_size = TIFF_CHUNK_BYTES;
    let mut decoder = Decoder::new(BufReader::new(File::open(path).map_err(|e| e.to_string())?))
        .map_err(|e| e.to_string())?
        .with_limits(limits);
    let (w, h) = decoder.dimensions().map_err(|e| e.to_string())?;
    if w < 2 || h < 2 {
        return Err(format!(
            "TIFF 尺寸 {w}×{h} 无法构建地形，宽高均需至少 2 像元"
        ));
    }
    let color = decoder.colortype().map_err(|e| e.to_string())?;
    if decoder.get_tag_u32(Tag::Orientation).unwrap_or(1) != 1 {
        return Err("暂不支持含旋转 Orientation 标签的 TIFF，请先转为左上角原点".into());
    }
    let planar = decoder.get_tag_u32(Tag::PlanarConfiguration).unwrap_or(1) == 2;
    let bands = if planar {
        1
    } else {
        color.num_samples() as usize
    };
    if matches!(
        color,
        tiff::ColorType::Palette(_)
            | tiff::ColorType::YCbCr(_)
            | tiff::ColorType::Lab(_)
            | tiff::ColorType::CMYK(_)
            | tiff::ColorType::CMYKA(_)
    ) {
        return Err("TIFF 请使用灰度、RGB 或数值高程波段".into());
    }
    let (cw, ch) = decoder.chunk_dimensions();
    let nx = w.div_ceil(cw);
    let ny = h.div_ceil(ch);
    nx.checked_mul(ny).ok_or("TIFF 分块数量超出可寻址范围")?;
    let geo = match georef::read_tiff(&mut decoder)? {
        Some(g) => Some(g),
        None => georef::read_sidecars(path)?,
    };
    let nodata = decoder
        .get_tag_ascii_string(Tag::GdalNodata)
        .ok()
        .and_then(|s| s.trim_matches('\0').trim().parse::<f64>().ok());
    // GDAL's text is rounded to the source sample type before exact matching.
    // Comparing f32 samples against the original decimal f64 misses e.g. -9999.1.
    let float32 = color.bit_depth() == 32
        && decoder
            .get_tag_u32_vec(Tag::SampleFormat)
            .ok()
            .is_some_and(|formats| formats.first() == Some(&3));
    let nodata = nodata.map(|value| {
        if float32 {
            f64::from(value as f32)
        } else {
            value
        }
    });
    let mut map = empty(path, w, h, limit.edge(w, h), geo);
    // Normalize after discovering the source-wide range, without rounding float64 elevations to float32 first.
    let mut area = AreaPreview::new(w, h, map.width, map.height);
    let mut sampled = if area.is_none() {
        vec![f64::NAN; map.values.len()]
    } else {
        Vec::new()
    };
    let mut minimum = f64::INFINITY;
    let mut maximum = f64::NEG_INFINITY;
    let mut numeric = false;
    if super::tiff_stream::supported(&mut decoder) {
        numeric = decoder
            .get_tag_u32_vec(Tag::SampleFormat)
            .ok()
            .is_some_and(|formats| formats.iter().any(|v| matches!(v, 2 | 3)))
            || map.source.as_ref().is_some_and(|s| s.geo.is_some());
        super::tiff_stream::read_rows(&mut decoder, |x, y, values| {
            for (i, &v) in values.iter().enumerate() {
                if v.is_finite() && nodata.is_none_or(|n| v != n) {
                    minimum = minimum.min(v);
                    maximum = maximum.max(v);
                    if let Some(area) = &mut area {
                        area.add(x + i as u32, y, v);
                    } else {
                        sampled[y as usize * w as usize + x as usize + i] = v;
                    }
                }
            }
            Ok(())
        })?;
    } else {
        // Less common packed/JPEG TIFF layouts retain the established bounded decoder.
        tiff_chunk_budget(cw, ch.min(h), bands, color.bit_depth())?;
        for cy in 0..ny {
            for cx in 0..nx {
                let index = cy * nx + cx;
                let (dw, dh) = decoder.chunk_data_dimensions(index);
                let chunk = decoder
                    .read_chunk(index)
                    .map_err(|e| format!("TIFF 分块 {index} 解码失败: {e}"))?;
                numeric |= matches!(
                    chunk,
                    DecodingResult::F16(_)
                        | DecodingResult::F32(_)
                        | DecodingResult::F64(_)
                        | DecodingResult::I8(_)
                        | DecodingResult::I16(_)
                        | DecodingResult::I32(_)
                        | DecodingResult::I64(_)
                ) || map.source.as_ref().is_some_and(|s| s.geo.is_some());
                let q = |i: usize| -> f64 {
                    match &chunk {
                        DecodingResult::U8(v) => v[i] as f64,
                        DecodingResult::U16(v) => v[i] as f64,
                        DecodingResult::U32(v) => v[i] as f64,
                        DecodingResult::U64(v) => v[i] as f64,
                        DecodingResult::I8(v) => v[i] as f64,
                        DecodingResult::I16(v) => v[i] as f64,
                        DecodingResult::I32(v) => v[i] as f64,
                        DecodingResult::I64(v) => v[i] as f64,
                        DecodingResult::F16(v) => v[i].to_f64(),
                        DecodingResult::F32(v) => v[i] as f64,
                        DecodingResult::F64(v) => v[i],
                    }
                };
                let value = |i: usize| -> f64 {
                    if !planar
                        && matches!(color, tiff::ColorType::RGB(_) | tiff::ColorType::RGBA(_))
                    {
                        0.2126 * q(i) + 0.7152 * q(i + 1) + 0.0722 * q(i + 2)
                    } else {
                        q(i)
                    }
                };
                let valid = |v: f64| v.is_finite() && nodata.is_none_or(|n| v != n);
                for local_y in 0..dh {
                    for local_x in 0..dw {
                        let v = value((local_y as usize * dw as usize + local_x as usize) * bands);
                        let x = cx * cw + local_x;
                        let y = cy * ch + local_y;
                        if valid(v) {
                            minimum = minimum.min(v);
                            maximum = maximum.max(v);
                            if let Some(area) = &mut area {
                                area.add(x, y, v);
                            } else {
                                sampled[y as usize * w as usize + x as usize] = v;
                            }
                        }
                    }
                }
            }
        }
    }
    if !minimum.is_finite() {
        return Err("TIFF 不含有效高程样本".into());
    }
    if let Some(area) = area {
        sampled = area.means();
    }
    if numeric {
        map.source_elevation_range=Some([minimum,maximum]);
        map.display_range = [1e-6, 1.];
        let range = maximum - minimum;
        // The recovered intersection shader reserves h=0 for empty space, including NoData.
        // Keep every valid minimum (and every valid constant plane) strictly above that sentinel.
        for (out, v) in map.values.iter_mut().zip(sampled) {
            *out = if v.is_finite() {
                let q = if range > 0. {
                    ((v - minimum) / range) as f32
                } else {
                    0.
                };
                q * (1. - 1e-6) + 1e-6
            } else {
                0.
            };
        }
        let geo = map.source.as_ref().and_then(|s| s.geo.as_ref());
        let meters = geo.and_then(|g| Some(g.pixel_meters(w, h)? / g.vertical_meters()?));
        let ratio = map.width as f32 / w as f32;
        if let Some(cell) = meters.filter(|v| v.is_finite() && *v > 0.) {
            map.bottom = (minimum / cell) as f32;
            map.span = (if range > 0. { range / cell } else { 1. }) as f32 * ratio;
        } else if geo.is_some() {
            // Unknown CRS units: show a stable normalized relief, without inventing a physical scale.
            map.bottom = minimum as f32;
            map.span = 100. * ratio;
        } else {
            map.bottom = minimum as f32;
            map.span = (if range > 0. { range } else { 1. }) as f32 * ratio;
        }
    } else {
        let divisor = match color.bit_depth() {
            8 => 255.,
            16 => 65535.,
            32 => u32::MAX as f32,
            _ => return Err("TIFF 无符号位深暂不支持".into()),
        };
        map.display_range = [
            (minimum / divisor as f64) as f32,
            (maximum / divisor as f64) as f32,
        ];
        for (out, v) in map.values.iter_mut().zip(sampled) {
            *out = if v.is_finite() {
                v as f32 / divisor
            } else {
                0.
            };
        }
    }
    Ok(map)
}

pub(super) fn jpeg_dimensions(w: u32, h: u32) -> Result<(), String> {
    // JPEG's dimensions are 16-bit. The decoder additionally holds whole-image
    // sample/coefficient buffers; preserve exact source extrema within the existing
    // pixel budget, rather than silently switching to lossy reduced-IDCT statistics.
    if w > u16::MAX as u32 || h > u16::MAX as u32 || u64::from(w) * u64::from(h) > JPEG_PIXELS {
        Err("JPEG 超出整图解码范围，请转换为 PNG 或分块 TIFF".into())
    } else {
        Ok(())
    }
}

fn tiff_chunk_budget(w: u32, h: u32, bands: usize, bits: u8) -> Result<(), String> {
    let samples = u64::from(w)
        .checked_mul(u64::from(h))
        .and_then(|v| v.checked_mul(bands as u64))
        .and_then(|v| v.checked_mul(u64::from(bits).div_ceil(8).max(1)))
        .ok_or("TIFF 分块尺寸溢出")?;
    if samples > TIFF_CHUNK_BYTES as u64 {
        Err("TIFF 单个 tile/strip 超过 256 MiB，请重存为分块 TIFF（如 512×512 tile）".into())
    } else {
        Ok(())
    }
}
