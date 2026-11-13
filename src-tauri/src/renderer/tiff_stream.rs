//! Scanline TIFF decoding: allocation depends on row width, never strip height.
use std::{
    fs::File,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
};
use tiff::{decoder::Decoder, tags::Tag};

type Tiff = Decoder<BufReader<File>>;
struct Layout {
    width: u32,
    height: u32,
    cw: u32,
    ch: u32,
    tiled: bool,
    bands: usize,
    bytes: usize,
    format: u32,
    predictor: u32,
    compression: u32,
    little: bool,
    rgb: bool,
    white: bool,
    offsets: Vec<u64>,
    counts: Vec<u64>,
}

fn layout(d: &mut Tiff) -> Result<Layout, String> {
    let (width, height) = d.dimensions().map_err(|e| e.to_string())?;
    let color = d.colortype().map_err(|e| e.to_string())?;
    let bits = d
        .get_tag_u16_vec(Tag::BitsPerSample)
        .map_err(|e| e.to_string())?;
    let bit = *bits.first().ok_or("TIFF 缺少采样位深")?;
    let format = d
        .get_tag_u32_vec(Tag::SampleFormat)
        .unwrap_or_else(|_| vec![1]);
    let fmt = *format.first().unwrap_or(&1);
    let predictor = d.get_tag_u32(Tag::Predictor).unwrap_or(1);
    let compression = d.get_tag_u32(Tag::Compression).unwrap_or(1);
    let photo = d.get_tag_u32(Tag::PhotometricInterpretation).unwrap_or(1);
    if !matches!(bit, 8 | 16 | 32 | 64)
        || bits.iter().any(|v| *v != bit)
        || !matches!(fmt, 1 | 2 | 3)
        || format.iter().any(|v| *v != fmt)
        || (fmt == 3 && !matches!(bit, 32 | 64))
        || !matches!(predictor, 1 | 2 | 3)
        || (predictor == 3 && fmt != 3)
        || (predictor == 2 && fmt == 3)
        || !matches!(compression, 1 | 5 | 8 | 32946 | 32773)
        || !matches!(photo, 0 | 1 | 2)
        || (photo == 0 && fmt == 2)
    {
        return Err("unsupported scanline TIFF layout".into());
    }
    let planar = d.get_tag_u32(Tag::PlanarConfiguration).unwrap_or(1);
    let samples = d.get_tag_u32(Tag::SamplesPerPixel).unwrap_or(1) as usize;
    if !matches!(planar, 1 | 2) || samples == 0 || samples > 64 {
        return Err("unsupported scanline TIFF bands".into());
    }
    let bands = if planar == 2 { 1 } else { samples };
    let tiled = d.get_tag_u64_vec(Tag::TileOffsets).is_ok();
    let (cw, ch) = if tiled {
        (
            d.get_tag_u32(Tag::TileWidth),
            d.get_tag_u32(Tag::TileLength),
        )
    } else {
        (
            Ok(width),
            Ok(d.get_tag_u32(Tag::RowsPerStrip).unwrap_or(height)),
        )
    };
    let cw = cw.map_err(|e| e.to_string())?;
    let ch = ch.map_err(|e| e.to_string())?;
    if cw == 0 || ch == 0 || width == 0 || height == 0 {
        return Err("TIFF 分块尺寸无效".into());
    }
    let offsets = d
        .get_tag_u64_vec(if tiled {
            Tag::TileOffsets
        } else {
            Tag::StripOffsets
        })
        .map_err(|e| e.to_string())?;
    let counts = d
        .get_tag_u64_vec(if tiled {
            Tag::TileByteCounts
        } else {
            Tag::StripByteCounts
        })
        .map_err(|e| e.to_string())?;
    let chunks = u64::from(width.div_ceil(cw)) * u64::from(height.div_ceil(ch));
    if chunks > offsets.len() as u64 || chunks > counts.len() as u64 {
        return Err("TIFF 分块索引不完整".into());
    }
    let file_len = d
        .inner()
        .get_ref()
        .metadata()
        .map_err(|e| e.to_string())?
        .len();
    for (&offset, &count) in offsets.iter().zip(&counts).take(chunks as usize) {
        if count == 0 || offset.checked_add(count).is_none_or(|end| end > file_len) {
            return Err("TIFF 分块数据超出文件范围".into());
        }
    }
    let bytes = usize::from(bit / 8);
    let row_bytes = u64::from(cw)
        .checked_mul(bands as u64)
        .and_then(|v| v.checked_mul(bytes as u64))
        .ok_or("TIFF 行大小溢出")?;
    if row_bytes > 64 * 1024 * 1024 {
        return Err("TIFF 单行超过 64 MiB".into());
    }
    // byte_order's type is private; compare against the known file header instead.
    let position = d.inner().stream_position().map_err(|e| e.to_string())?;
    d.inner()
        .seek(SeekFrom::Start(0))
        .map_err(|e| e.to_string())?;
    let mut header = [0; 2];
    d.inner()
        .read_exact(&mut header)
        .map_err(|e| e.to_string())?;
    d.inner()
        .seek(SeekFrom::Start(position))
        .map_err(|e| e.to_string())?;
    Ok(Layout {
        width,
        height,
        cw,
        ch,
        tiled,
        bands,
        bytes,
        format: fmt,
        predictor,
        compression,
        little: header == *b"II",
        rgb: planar == 1 && matches!(color, tiff::ColorType::RGB(_) | tiff::ColorType::RGBA(_)),
        white: photo == 0,
        offsets,
        counts,
    })
}

/// Metadata-only eligibility check. No callback or raster allocation occurs here.
pub fn supported(d: &mut Tiff) -> bool {
    layout(d).is_ok()
}

/// Emit x, y, grayscale/first-plane values, preserving source numeric values.
pub fn read_rows(
    d: &mut Tiff,
    mut emit: impl FnMut(u32, u32, &[f64]) -> Result<(), String>,
) -> Result<(), String> {
    let reader = ChunkReader::new(d)?;
    let [width, height, cw, ch] = reader.grid();
    for index in 0..width.div_ceil(cw) * height.div_ceil(ch) {
        reader.read(d, index, &mut emit)?;
    }
    Ok(())
}

/// Parsed once: random source windows must not reread every strip offset per page.
pub(super) struct ChunkReader {
    layout: Layout,
}
impl ChunkReader {
    pub(super) fn new(d: &mut Tiff) -> Result<Self, String> {
        Ok(Self { layout: layout(d)? })
    }
    pub(super) fn grid(&self) -> [u32; 4] {
        let l = &self.layout;
        [l.width, l.height, l.cw, l.ch]
    }
    /// Decode exactly one native strip/tile, retaining the scanline memory bound
    /// and every predictor/compression/byte-order rule used by whole-image import.
    pub(super) fn read(
        &self,
        d: &mut Tiff,
        index: u32,
        mut emit: impl FnMut(u32, u32, &[f64]) -> Result<(), String>,
    ) -> Result<(), String> {
        let l = &self.layout;
        let nx = l.width.div_ceil(l.cw);
        if index >= nx * l.height.div_ceil(l.ch) {
            return Err("TIFF 分块索引超出范围".into());
        }
        let (cx, cy) = (index % nx, index / nx);
        let index = index as usize;
        let rows = if l.tiled {
            l.ch
        } else {
            l.ch.min(l.height - cy * l.ch)
        };
        let visible = l.cw.min(l.width - cx * l.cw) as usize;
        let row_size = l.cw as usize * l.bands * l.bytes;
        let mut row = vec![0u8; row_size];
        let mut previous = vec![0u64; l.bands];
        let mut values = vec![0f64; visible];
        let mut filled = 0usize;
        let mut row_y = 0u32;
        let mut seen = 0u64;
        let expected = row_size as u64 * u64::from(rows);
        let mut sink = |data: &[u8]| -> Result<(), String> {
            seen = seen
                .checked_add(data.len() as u64)
                .ok_or("TIFF 解码字节数溢出")?;
            if seen > expected {
                return Err("TIFF 解码数据超过分块声明尺寸".into());
            }
            let mut data = data;
            while !data.is_empty() {
                let n = data.len().min(row_size - filled);
                row[filled..filled + n].copy_from_slice(&data[..n]);
                filled += n;
                data = &data[n..];
                if filled == row_size {
                    if cy * l.ch + row_y < l.height {
                        decode_row(&mut row, l, &mut previous, &mut values);
                        emit(cx * l.cw, cy * l.ch + row_y, &values)?;
                    }
                    row_y += 1;
                    filled = 0;
                }
            }
            Ok(())
        };
        d.inner()
            .seek(SeekFrom::Start(l.offsets[index]))
            .map_err(|e| e.to_string())?;
        let mut source = d.inner().take(l.counts[index]);
        decode_chunk(&mut source, l.compression, &mut sink)?;
        if seen != expected || filled != 0 {
            return Err(format!(
                "TIFF 分块数据不完整：需要 {expected} 字节，实际 {seen} 字节"
            ));
        }
        Ok(())
    }
}

fn decode_row(row: &mut [u8], l: &Layout, previous: &mut [u64], values: &mut [f64]) {
    let count = row.len() / l.bytes;
    if l.predictor == 3 {
        for i in l.bands..row.len() {
            row[i] = row[i].wrapping_add(row[i - l.bands]);
        }
    }
    previous.fill(0);
    let mask = if l.bytes == 8 {
        u64::MAX
    } else {
        (1u64 << (8 * l.bytes)) - 1
    };
    let mut pixel = [0f64; 3];
    for i in 0..count {
        let mut raw = 0u64;
        if l.predictor == 3 {
            for b in 0..l.bytes {
                raw = (raw << 8) | u64::from(row[b * count + i]);
            }
        } else {
            for b in 0..l.bytes {
                let shift = if l.little { b } else { l.bytes - 1 - b };
                raw |= u64::from(row[i * l.bytes + b]) << (shift * 8);
            }
            if l.predictor == 2 {
                let band = i % l.bands;
                raw = raw.wrapping_add(previous[band]) & mask;
                previous[band] = raw;
            }
        }
        let mut value = match l.format {
            2 => {
                let shift = 64 - l.bytes * 8;
                ((raw << shift) as i64 >> shift) as f64
            }
            3 => {
                if l.bytes == 4 {
                    f32::from_bits(raw as u32) as f64
                } else {
                    f64::from_bits(raw)
                }
            }
            _ => raw as f64,
        };
        if l.white {
            value = if l.format == 3 {
                1. - value
            } else {
                (mask - raw) as f64
            };
        }
        let x = i / l.bands;
        let band = i % l.bands;
        if x < values.len() {
            if l.rgb {
                if band < 3 {
                    pixel[band] = value;
                }
                if band == 2 {
                    values[x] = 0.2126 * pixel[0] + 0.7152 * pixel[1] + 0.0722 * pixel[2];
                }
            } else if band == 0 {
                values[x] = value;
            }
        }
    }
}

fn copy_decoded(
    source: &mut impl Read,
    sink: &mut impl FnMut(&[u8]) -> Result<(), String>,
) -> Result<(), String> {
    let mut output = [0u8; 65536];
    loop {
        let n = source.read(&mut output).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        sink(&output[..n])?;
    }
    Ok(())
}
fn decode_chunk(
    source: &mut impl Read,
    compression: u32,
    sink: &mut impl FnMut(&[u8]) -> Result<(), String>,
) -> Result<(), String> {
    match compression {
        1 => copy_decoded(source, sink),
        8 | 32946 => copy_decoded(&mut flate2::read::ZlibDecoder::new(source), sink),
        32773 => {
            let mut header = [0u8; 1];
            let mut bytes = [0u8; 128];
            loop {
                if source.read(&mut header).map_err(|e| e.to_string())? == 0 {
                    break;
                }
                let code = header[0] as i8;
                if code >= 0 {
                    let n = code as usize + 1;
                    source
                        .read_exact(&mut bytes[..n])
                        .map_err(|e| e.to_string())?;
                    sink(&bytes[..n])?;
                } else if code != -128 {
                    source.read_exact(&mut header).map_err(|e| e.to_string())?;
                    let n = (1i16 - i16::from(code)) as usize;
                    bytes[..n].fill(header[0]);
                    sink(&bytes[..n])?;
                }
            }
            Ok(())
        }
        5 => decode_lzw(source, sink),
        _ => Err("unsupported scanline TIFF compression".into()),
    }
}
fn decode_lzw(
    source: &mut impl Read,
    sink: &mut impl FnMut(&[u8]) -> Result<(), String>,
) -> Result<(), String> {
    let mut input = BufReader::with_capacity(65536, source);
    // Pre-TIFF-6 libtiff uses LSB/late-change. Its initial nine-bit CLEAR code
    // has a distinct signature, so select once before emitting any samples.
    // Never retry an arbitrary coding variant after partial raster callbacks.
    let start = input.fill_buf().map_err(|e| e.to_string())?;
    let legacy = start.len() >= 2 && start[0] == 0 && start[1] & 1 == 1;
    let mut decoder = if legacy {
        weezl::decode::Decoder::new(weezl::BitOrder::Lsb, 8)
    } else {
        weezl::decode::Decoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8)
    };
    let mut output = [0u8; 65536];
    loop {
        let bytes = input.fill_buf().map_err(|e| e.to_string())?;
        let exhausted = bytes.is_empty();
        let result = decoder.decode_bytes(bytes, &mut output);
        // consumed_out remains valid even when the accompanying status reports no progress/error.
        sink(&output[..result.consumed_out])?;
        input.consume(result.consumed_in);
        match result.status {
            Ok(weezl::LzwStatus::Done) => return Ok(()),
            Err(e) => return Err(format!("TIFF LZW 编码损坏：{e}")),
            Ok(weezl::LzwStatus::NoProgress) if exhausted => return Ok(()),
            _ => {}
        }
        if result.consumed_in == 0 && result.consumed_out == 0 && !exhausted {
            return Err("TIFF LZW 解码无法继续".into());
        }
    }
}


