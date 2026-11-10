//! Source-grid orthographic output. Callbacks return top-to-bottom straight RGBA.
//! RGBA16 samples use native endian; TIFF serializes values in its declared byte order.
//! Streaming strips intentionally use no compression: tiff 0.11's public
//! `write_strip` does not activate the compressor used by `write_data`.

use super::{georef::GeoReference, raster::RasterSource};
use std::{
    fs::{self, File, OpenOptions},
    io::{BufWriter, Seek, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
use tiff::{
    encoder::{
        colortype::{ColorType, RGBA16, RGBA8},
        TiffEncoder, TiffKind,
    },
    tags::Tag,
};

const STRIP_HEIGHT: u32 = 128;
const TILE_WIDTH: u32 = 1024;
const MAX_STRIP_BYTES: usize = 24 * 1024 * 1024;
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

/// Export only a renderer explicitly configured for source-aligned orthography.
/// Source pixel (x,y) must map to callback pixel (x,y); no camera projection or
/// image flips are applied here. The georeference transform already uses corners.
pub fn write_orthographic<F>(
    path: &Path,
    source: &RasterSource,
    render_tile: F,
) -> Result<(), String>
where
    F: FnMut(u32, u32, u32, u32) -> Result<Vec<u8>, String>,
{
    write_orthographic_extent(path, source, source.width, source.height, 0, 0, render_tile)
}

/// The expanded grid keeps the source pixel basis: source (0,0) lands at (left,top).
/// Render callbacks use coordinates in the expanded output, never shifted source coordinates.
pub fn write_orthographic_extent<F>(
    path: &Path,
    source: &RasterSource,
    width: u32,
    height: u32,
    left: u32,
    top: u32,
    render_tile: F,
) -> Result<(), String>
where
    F: FnMut(u32, u32, u32, u32) -> Result<Vec<u8>, String>,
{
    write_orthographic_extent_depth(path, source, width, height, left, top, 8, render_tile)
}

/// RGBA16 tiles contain native-endian u16 values, including straight alpha.
pub fn write_orthographic_extent_depth<F>(
    path: &Path,
    source: &RasterSource,
    width: u32,
    height: u32,
    left: u32,
    top: u32,
    depth: u8,
    render_tile: F,
) -> Result<(), String>
where
    F: FnMut(u32, u32, u32, u32) -> Result<Vec<u8>, String>,
{
    validate_depth(depth)?;
    if left.checked_add(source.width).is_none_or(|w| w > width)
        || top.checked_add(source.height).is_none_or(|h| h > height)
    {
        return Err("扩展 TIFF 范围必须完整包含源图像".into());
    }
    let geo = source
        .geo
        .as_ref()
        .ok_or("源图像没有地理坐标，无法导出带坐标系的 TIFF")?;
    let shifted = geo.shifted_pixels(-i64::from(left), -i64::from(top))?;
    write_grid_depth(path, width, height, &shifted, depth, render_tile)
}

struct PendingFile(PathBuf);
impl Drop for PendingFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn temporary_file(path: &Path) -> Result<(PendingFile, File), String> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path.file_name().ok_or("导出文件名无效")?.to_string_lossy();
    for _ in 0..32 {
        let temp = parent.join(format!(
            ".{name}.{}.{}.part",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        match OpenOptions::new().write(true).create_new(true).open(&temp) {
            Ok(file) => return Ok((PendingFile(temp), file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("无法创建 TIFF 临时文件：{e}")),
        }
    }
    Err("无法分配 TIFF 临时文件".into())
}

fn area_keys(geo: &GeoReference) -> Result<Vec<u16>, String> {
    let mut keys = geo.keys.clone();
    if keys.len() < 4 || keys[0] != 1 || keys.len() < 4 + usize::from(keys[3]) * 4 {
        return Err("源图像缺少有效 GeoTIFF 坐标系定义；仅有 WKT 的坐标系暂不能导出".into());
    }
    let entries_end = 4 + usize::from(keys[3]) * 4;
    let has_crs = keys[4..entries_end]
        .chunks_exact(4)
        .any(|k| matches!(k[0], 2048 | 3072) && k[1] == 0 && k[2] == 1 && k[3] != 0);
    if !has_crs {
        return Err("源图像未定义可写出的地理坐标系".into());
    }
    if let Some(key) = keys[4..entries_end]
        .chunks_exact_mut(4)
        .find(|k| k[0] == 1025)
    {
        key.copy_from_slice(&[1025, 0, 1, 1]);
    } else {
        keys[3] = keys[3].checked_add(1).ok_or("GeoTIFF 键数量超出限制")?;
        // A directory may contain SHORT parameter arrays after its entries.
        // Inserting an entry shifts those arrays and their directory offsets.
        let mut entries: Vec<[u16; 4]> = keys[4..entries_end]
            .chunks_exact(4)
            .map(|k| [k[0], k[1], k[2], k[3]])
            .collect();
        for key in &mut entries {
            if key[1] == 34735 && usize::from(key[3]) >= entries_end {
                key[3] = key[3]
                    .checked_add(4)
                    .ok_or("GeoTIFF SHORT 参数偏移超出限制")?;
            }
        }
        entries.push([1025, 0, 1, 1]);
        entries.sort_by_key(|k| k[0]);
        keys.splice(entries_end..entries_end, [0u16; 4]);
        for (dst, entry) in keys[4..entries_end + 4].chunks_exact_mut(4).zip(entries) {
            dst.copy_from_slice(&entry);
        }
    }
    Ok(keys)
}

fn matrix(geo: &GeoReference) -> [f64; 16] {
    let t = geo.transform;
    [
        t[1], t[2], 0., t[0], t[4], t[5], 0., t[3], 0., 0., 1., 0., 0., 0., 0., 1.,
    ]
}

fn validate_depth(depth: u8) -> Result<usize, String> {
    match depth {
        8 => Ok(1),
        16 => Ok(2),
        _ => Err("TIFF 位深必须为 8 或 16".into()),
    }
}


fn strip_layout_depth(width: u32, height: u32, depth: u8) -> Result<(usize, u32), String> {
    let sample_bytes = validate_depth(depth)?;
    if width == 0 || height == 0 {
        return Err("正射 TIFF 尺寸必须大于零".into());
    }
    let row_bytes = usize::try_from(width)
        .ok()
        .and_then(|w| w.checked_mul(4 * sample_bytes))
        .filter(|bytes| *bytes <= MAX_STRIP_BYTES)
        .ok_or("正射 TIFF 单行大小超过条带内存限制")?;
    let rows = STRIP_HEIGHT
        .min((MAX_STRIP_BYTES / row_bytes) as u32)
        .max(1);
    // Keep derivative quads aligned between strips for screen-space contours.
    let rows=if rows>1 {rows & !1} else {1};
    Ok((row_bytes, rows))
}


fn requires_bigtiff_depth(
    width: u32,
    height: u32,
    geo: &GeoReference,
    depth: u8,
) -> Result<bool, String> {
    let sample_bytes = validate_depth(depth)?;
    let (_, rows) = strip_layout_depth(width, height, depth)?;
    let payload = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(4 * sample_bytes as u64))
        .ok_or("正射 TIFF 文件大小超出限制")?;
    // Reserve directory/tag payload, strip offsets/counts and alignment before choosing classic TIFF.
    let metadata = 65_536u64
        + u64::from(height.div_ceil(rows)) * 16
        + geo.keys.len() as u64 * 2
        + geo.doubles.len() as u64 * 8
        + geo.ascii.len() as u64;
    Ok(payload
        .checked_add(metadata)
        .ok_or("正射 TIFF 文件大小超出限制")?
        > u64::from(u32::MAX))
}


fn write_grid_depth<F>(
    path: &Path,
    width: u32,
    height: u32,
    geo: &GeoReference,
    depth: u8,
    mut render_tile: F,
) -> Result<(), String>
where
    F: FnMut(u32, u32, u32, u32) -> Result<Vec<u8>, String>,
{
    let (_, rows) = strip_layout_depth(width, height, depth)?;
    if !geo.valid() {
        return Err("地理仿射变换无效".into());
    }
    let keys = area_keys(geo)?;
    let big = requires_bigtiff_depth(width, height, geo, depth)?;
    let (pending, file) = temporary_file(path)?;
    let mut writer = BufWriter::new(file);
    if big {
        let mut tiff = TiffEncoder::new_big(&mut writer).map_err(|e| e.to_string())?;
        encode_grid_depth(
            &mut tiff,
            width,
            height,
            rows,
            geo,
            &keys,
            depth,
            &mut render_tile,
        )?;
    } else {
        let mut tiff = TiffEncoder::new(&mut writer).map_err(|e| e.to_string())?;
        encode_grid_depth(
            &mut tiff,
            width,
            height,
            rows,
            geo,
            &keys,
            depth,
            &mut render_tile,
        )?;
    }
    writer
        .flush()
        .map_err(|e| format!("刷新 TIFF 文件失败：{e}"))?;
    writer
        .get_ref()
        .sync_all()
        .map_err(|e| format!("保存 TIFF 文件失败：{e}"))?;
    drop(writer);
    fs::rename(&pending.0, path).map_err(|e| format!("完成 TIFF 文件失败：{e}"))?;
    Ok(())
}


fn encode_grid_depth<W: Write + Seek, K: TiffKind, F>(
    tiff: &mut TiffEncoder<W, K>,
    width: u32,
    height: u32,
    rows: u32,
    geo: &GeoReference,
    keys: &[u16],
    depth: u8,
    render_tile: &mut F,
) -> Result<(), String>
where
    F: FnMut(u32, u32, u32, u32) -> Result<Vec<u8>, String>,
{
    match depth {
        8 => encode_grid_samples::<W, K, F, RGBA8>(
            tiff,
            width,
            height,
            rows,
            geo,
            keys,
            render_tile,
            |b| b[0],
        ),
        16 => encode_grid_samples::<W, K, F, RGBA16>(
            tiff,
            width,
            height,
            rows,
            geo,
            keys,
            render_tile,
            |b| u16::from_ne_bytes([b[0], b[1]]),
        ),
        _ => Err("TIFF 位深必须为 8 或 16".into()),
    }
}
fn encode_grid_samples<W: Write + Seek, K: TiffKind, F, C: ColorType>(
    tiff: &mut TiffEncoder<W, K>,
    width: u32,
    height: u32,
    rows: u32,
    geo: &GeoReference,
    keys: &[u16],
    render_tile: &mut F,
    convert: fn(&[u8]) -> C::Inner,
) -> Result<(), String>
where
    F: FnMut(u32, u32, u32, u32) -> Result<Vec<u8>, String>,
    C::Inner: Copy + Default,
    [C::Inner]: tiff::encoder::TiffValue,
{
    let sample_bytes = std::mem::size_of::<C::Inner>();
    let mut image = tiff
        .new_image::<C>(width, height)
        .map_err(|e| e.to_string())?;
    image.rows_per_strip(rows).map_err(|e| e.to_string())?;
    let dir = image.encoder();
    dir.write_tag(Tag::Unknown(34675), &include_bytes!("srgb.icc")[..])
        .map_err(|e| e.to_string())?;
    dir.write_tag(Tag::Orientation, 1u16)
        .map_err(|e| e.to_string())?;
    dir.write_tag(Tag::ExtraSamples, &[2u16][..])
        .map_err(|e| e.to_string())?;
    dir.write_tag(Tag::ModelTransformationTag, &matrix(geo)[..])
        .map_err(|e| e.to_string())?;
    dir.write_tag(Tag::GeoKeyDirectoryTag, keys)
        .map_err(|e| e.to_string())?;
    if !geo.doubles.is_empty() {
        dir.write_tag(Tag::GeoDoubleParamsTag, &geo.doubles[..])
            .map_err(|e| e.to_string())?;
    }
    if !geo.ascii.is_empty() {
        dir.write_tag(Tag::GeoAsciiParamsTag, geo.ascii.as_str())
            .map_err(|e| e.to_string())?;
    }
    let row_samples = width as usize * 4;
    for y in (0..height).step_by(rows as usize) {
        let h = rows.min(height - y);
        let mut strip = vec![C::Inner::default(); row_samples * h as usize];
        for x in (0..width).step_by(TILE_WIDTH as usize) {
            let w = TILE_WIDTH.min(width - x);
            let tile = render_tile(x, y, w, h)?;
            let tile_row = w as usize * 4 * sample_bytes;
            if tile.len() != tile_row * h as usize {
                return Err(format!("正射渲染块 ({x}, {y}) 的像素数量不正确"));
            }
            for row in 0..h as usize {
                let start = row * row_samples + x as usize * 4;
                for (dst, bytes) in strip[start..start + w as usize * 4]
                    .iter_mut()
                    .zip(tile[row * tile_row..(row + 1) * tile_row].chunks_exact(sample_bytes))
                {
                    *dst = convert(bytes);
                }
            }
        }
        image
            .write_strip(&strip)
            .map_err(|e| format!("写入 TIFF 条带失败：{e}"))?;
    }
    image.finish().map_err(|e| e.to_string())?;
    Ok(())
}
