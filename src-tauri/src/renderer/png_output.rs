//! Bounded-memory PNG output. Tiles are rendered at their final pixel coordinates.
use std::{
    fs::{self, File, OpenOptions},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
const TILE_WIDTH: u32 = 1024;
const STRIP_HEIGHT: u32 = 128;
pub const MAX_EDGE: u32 = 243600;
const MAX_STRIP_BYTES: usize = 24 * 1024 * 1024;
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
#[derive(Clone, Copy)]
enum Encoding {
    
    
    LevelOne,
}
struct PendingFile(PathBuf);
impl Drop for PendingFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

pub fn write<F>(path: &Path, width: u32, height: u32, tile: F) -> Result<(), String>
where
    F: FnMut(u32, u32, u32, u32) -> Result<Vec<u8>, String>,
{
    write_with_compression(path, width, height, tile, Encoding::LevelOne)
}

fn write_with_compression<F>(
    path: &Path,
    width: u32,
    height: u32,
    mut tile: F,
    compression: Encoding,
) -> Result<(), String>
where
    F: FnMut(u32, u32, u32, u32) -> Result<Vec<u8>, String>,
{
    if width == 0 || height == 0 || width > MAX_EDGE || height > MAX_EDGE {
        return Err("PNG 输出尺寸超出范围".into());
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path.file_name().ok_or("导出文件名无效")?.to_string_lossy();
    let (pending, file): (PendingFile, File) = (0..32)
        .find_map(|_| {
            let p = parent.join(format!(
                ".{name}.{}.{}.part",
                std::process::id(),
                NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
            ));
            match OpenOptions::new().write(true).create_new(true).open(&p) {
                Ok(f) => Some(Ok((PendingFile(p), f))),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => None,
                Err(e) => Some(Err(e.to_string())),
            }
        })
        .ok_or("无法分配 PNG 临时文件")??;
    let mut encoder = png::Encoder::new(BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    match compression {
        
                // Adaptive row filtering is retained. Deflate level 1 preserves all
        // RGBA bytes while avoiding the slower default level-6 search.
        Encoding::LevelOne => encoder.set_deflate_compression(png::DeflateCompression::Level(1)),
    }
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    let mut stream = writer.stream_writer().map_err(|e| e.to_string())?;
    let stride = width as usize * 4;
    let strip_rows = STRIP_HEIGHT.min((MAX_STRIP_BYTES / stride) as u32).max(1);
    for y in (0..height).step_by(strip_rows as usize) {
        let rows = strip_rows.min(height - y);
        let mut strip = vec![0; stride * rows as usize];
        for x in (0..width).step_by(TILE_WIDTH as usize) {
            let cols = TILE_WIDTH.min(width - x);
            let bytes = tile(x, y, cols, rows)?;
            let tile_stride = cols as usize * 4;
            if bytes.len() != tile_stride * rows as usize {
                return Err("PNG 渲染分块尺寸无效".into());
            }
            for row in 0..rows as usize {
                strip[row * stride + x as usize * 4..row * stride + (x + cols) as usize * 4]
                    .copy_from_slice(&bytes[row * tile_stride..(row + 1) * tile_stride]);
            }
        }
        stream.write_all(&strip).map_err(|e| e.to_string())?;
    }
    stream.finish().map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())?;
    fs::rename(&pending.0, path).map_err(|e| format!("完成 PNG 文件失败：{e}"))?;
    Ok(())
}


