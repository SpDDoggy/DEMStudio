//! Additional image formats with bounded rendering strips and atomic publication.
use image::{GenericImageView, Rgb};
use std::{
    cell::RefCell,
    fs::{self, OpenOptions},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
const MAX_STRIP_BYTES: usize = 24 * 1024 * 1024;
#[derive(Clone, Copy)]
enum Format {
    Jpeg,
    Bmp,
    Tga,
}
fn format(path: &Path) -> Result<Format, String> {
    match path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => Ok(Format::Jpeg),
        "bmp" => Ok(Format::Bmp),
        "tga" => Ok(Format::Tga),
        _ => Err("不支持的图像导出格式".into()),
    }
}
pub fn validate(path: &Path, width: u32, height: u32, depth: u8) -> Result<(), String> {
    let f = format(path)?;
    if depth != 8 {
        return Err("此图像格式仅支持 8 位导出".into());
    }
    let max = match f {
        Format::Bmp => super::png_output::MAX_EDGE,
        _ => 65535,
    };
    if width == 0 || height == 0 || width > max || height > max {
        return Err(format!("图像输出尺寸超出范围（最大边长 {max}）"));
    }
    if matches!(f, Format::Bmp)
        && (u64::from(width) * 3).div_ceil(4) * 4 * u64::from(height) + 54 > u64::from(u32::MAX)
    {
        return Err("BMP 文件大小超出 4 GB 格式限制".into());
    }
    Ok(())
}
struct Pending(PathBuf);
impl Drop for Pending {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
struct Strip {
    y: u32,
    rows: u32,
    bytes: Vec<u8>,
    error: Option<String>,
}
struct CheckedWriter<'a, W> {
    writer: W,
    strip: &'a RefCell<Strip>,
}
impl<W: Write> Write for CheckedWriter<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if let Some(error) = &self.strip.borrow().error {
            return Err(std::io::Error::other(error.clone()));
        }
        self.writer.write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        if let Some(error) = &self.strip.borrow().error {
            return Err(std::io::Error::other(error.clone()));
        }
        self.writer.flush()
    }
}
struct Tiled<F> {
    width: u32,
    height: u32,
    rows: u32,
    tile: RefCell<F>,
    strip: RefCell<Strip>,
}
impl<F> Tiled<F>
where
    F: FnMut(u32, u32, u32, u32) -> Result<Vec<u8>, String>,
{
    fn new(width: u32, height: u32, tile: F) -> Self {
        // JPEG visits 8x8 blocks. Cache a multiple of eight rows so no block
        // straddles strips; two-row alignment also preserves derivative quads.
        let rows = (MAX_STRIP_BYTES / (width as usize * 4)).min(128).max(8) as u32 & !7;
        Self {
            width,
            height,
            rows,
            tile: RefCell::new(tile),
            strip: RefCell::new(Strip {
                y: 0,
                rows: 0,
                bytes: vec![],
                error: None,
            }),
        }
    }
    fn load(&self, y: u32) -> Result<(), String> {
        let mut s = self.strip.borrow_mut();
        if let Some(e) = &s.error {
            return Err(e.clone());
        }
        if s.rows > 0 && y >= s.y && y < s.y + s.rows {
            return Ok(());
        }
        let top = y / self.rows * self.rows;
        let rows = self.rows.min(self.height - top);
        s.bytes.resize(self.width as usize * rows as usize * 4, 0);
        for x in (0..self.width).step_by(1024) {
            let cols = 1024.min(self.width - x);
            let result = (self.tile.borrow_mut())(x, top, cols, rows);
            let bytes = match result {
                Ok(b) if b.len() == cols as usize * rows as usize * 4 => b,
                Ok(_) => {
                    s.error = Some("图像渲染分块尺寸无效".into());
                    return Err(s.error.clone().unwrap());
                }
                Err(e) => {
                    s.error = Some(e.clone());
                    return Err(e);
                }
            };
            for row in 0..rows as usize {
                let start = (row * self.width as usize + x as usize) * 4;
                let count = cols as usize * 4;
                s.bytes[start..start + count]
                    .copy_from_slice(&bytes[row * count..(row + 1) * count]);
            }
        }
        s.y = top;
        s.rows = rows;
        Ok(())
    }
}
fn opaque(p: &[u8]) -> [u8; 3] {
    let a = u32::from(p[3]);
    [0, 1, 2].map(|c| ((u32::from(p[c]) * a + 255 * (255 - a) + 127) / 255) as u8)
}
impl<F> GenericImageView for Tiled<F>
where
    F: FnMut(u32, u32, u32, u32) -> Result<Vec<u8>, String>,
{
    type Pixel = Rgb<u8>;
    fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
    fn get_pixel(&self, x: u32, y: u32) -> Rgb<u8> {
        if self.load(y).is_err() {
            return Rgb([0, 0, 0]);
        }
        let s = self.strip.borrow();
        let i = ((y - s.y) * self.width + x) as usize * 4;
        Rgb(opaque(&s.bytes[i..i + 4]))
    }
}
pub fn write_depth<F>(
    path: &Path,
    width: u32,
    height: u32,
    depth: u8,
    tile: F,
) -> Result<(), String>
where
    F: FnMut(u32, u32, u32, u32) -> Result<Vec<u8>, String>,
{
    validate(path, width, height, depth)?;
    let f = format(path)?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path.file_name().ok_or("导出文件名无效")?.to_string_lossy();
    let (pending, file) = (0..32)
        .find_map(|_| {
            let p = parent.join(format!(
                ".{name}.{}.{}.part",
                std::process::id(),
                NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
            ));
            match OpenOptions::new().create_new(true).write(true).open(&p) {
                Ok(f) => Some(Ok((Pending(p), f))),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => None,
                Err(e) => Some(Err(e.to_string())),
            }
        })
        .ok_or("无法分配图像临时文件")??;
    let view = Tiled::new(width, height, tile);
    let mut writer = BufWriter::new(file);
    match f {
        Format::Jpeg => {
            let encoded = image::codecs::jpeg::JpegEncoder::new_with_quality(
                CheckedWriter {
                    writer: &mut writer,
                    strip: &view.strip,
                },
                95,
            )
            .encode_image(&view);
            if let Some(e) = &view.strip.borrow().error {
                return Err(e.clone());
            }
            encoded.map_err(|e| e.to_string())?;
        }
        Format::Bmp | Format::Tga => {
            let bpp = if matches!(f, Format::Bmp) { 3usize } else { 4 };
            let stride = if bpp == 3 {
                (width as usize * 3).div_ceil(4) * 4
            } else {
                width as usize * 4
            };
            if bpp == 3 {
                let mut h = vec![0u8; 54];
                h[..2].copy_from_slice(b"BM");
                h[2..6].copy_from_slice(&(54 + stride as u32 * height).to_le_bytes());
                h[10..14].copy_from_slice(&54u32.to_le_bytes());
                h[14..18].copy_from_slice(&40u32.to_le_bytes());
                h[18..22].copy_from_slice(&(width as i32).to_le_bytes());
                h[22..26].copy_from_slice(&(-(height as i32)).to_le_bytes());
                h[26..28].copy_from_slice(&1u16.to_le_bytes());
                h[28..30].copy_from_slice(&24u16.to_le_bytes());
                writer.write_all(&h).map_err(|e| e.to_string())?;
            } else {
                let mut h = [0u8; 18];
                h[2] = 2;
                h[12..14].copy_from_slice(&(width as u16).to_le_bytes());
                h[14..16].copy_from_slice(&(height as u16).to_le_bytes());
                h[16] = 32;
                h[17] = 0x28;
                writer.write_all(&h).map_err(|e| e.to_string())?;
            }
            let mut row = vec![0u8; stride];
            for y in 0..height {
                view.load(y)?;
                let s = view.strip.borrow();
                let start = ((y - s.y) * width) as usize * 4;
                for (p, out) in s.bytes[start..start + width as usize * 4]
                    .chunks_exact(4)
                    .zip(row.chunks_exact_mut(bpp))
                {
                    let rgb = if bpp == 3 {
                        opaque(p)
                    } else {
                        [p[0], p[1], p[2]]
                    };
                    out[..3].copy_from_slice(&[rgb[2], rgb[1], rgb[0]]);
                    if bpp == 4 {
                        out[3] = p[3];
                    }
                }
                writer.write_all(&row).map_err(|e| e.to_string())?;
            }
        }
    }
    writer.flush().map_err(|e| e.to_string())?;
    drop(writer);
    fs::rename(&pending.0, path).map_err(|e| format!("完成图像文件失败：{e}"))?;
    Ok(())
}
