//! Color imports use top-to-bottom, left-to-right, straight-alpha RGBA8 pixels.
//! PNG/TIFF decoding has a 1 GiB allocation budget; the source image can briefly
//! occupy that budget before reduction. This is not a process-wide memory cap.
use image::{imageops::FilterType, DynamicImage, ImageFormat, ImageReader, Limits, RgbaImage};
use std::{fs::File, io::BufReader, path::Path};

const MAX_EDGE: u32 = 16_384;
const PREVIEW_EDGE: u32 = 4_096;
const DECODE_BUDGET: u64 = 1024 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct ColorTexture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub name: String,
}

fn dimensions(width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 || width > MAX_EDGE || height > MAX_EDGE {
        return Err("贴图尺寸应介于 1×1 和 16384×16384 之间".into());
    }
    Ok(())
}

fn preview_size(width: u32, height: u32) -> (u32, u32) {
    let ratio = (PREVIEW_EDGE as f64 / width.max(height) as f64).min(1.);
    (
        ((width as f64 * ratio).round() as u32).max(1),
        ((height as f64 * ratio).round() as u32).max(1),
    )
}

pub fn load_texture(path: &Path) -> Result<ColorTexture, String> {
    let reader = ImageReader::open(path)
        .map_err(|e| e.to_string())?
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let associated_alpha = if reader.format() == Some(ImageFormat::Tiff) {
        let mut metadata = tiff::decoder::Decoder::new(BufReader::new(
            File::open(path).map_err(|e| e.to_string())?,
        ))
        .map_err(|e| e.to_string())?;
        metadata
            .find_tag_unsigned_vec::<u16>(tiff::tags::Tag::ExtraSamples)
            .map_err(|e| e.to_string())?
            .is_some_and(|samples| samples.first() == Some(&1))
    } else {
        false
    };
    let image = match reader.format() {
        Some(ImageFormat::Jpeg) => jpeg(path)?,
        Some(ImageFormat::Png | ImageFormat::Tiff) => {
            // Read dimensions before allocating a pixel buffer, then reopen to decode.
            let (width, height) = reader.into_dimensions().map_err(|e| e.to_string())?;
            dimensions(width, height)?;
            let mut reader = ImageReader::open(path)
                .map_err(|e| e.to_string())?
                .with_guessed_format()
                .map_err(|e| e.to_string())?;
            let mut limits = Limits::default();
            limits.max_image_width = Some(MAX_EDGE);
            limits.max_image_height = Some(MAX_EDGE);
            limits.max_alloc = Some(DECODE_BUDGET);
            reader.limits(limits);
            reader.decode().map_err(|e| e.to_string())?
        }
        _ => return Err("贴图仅支持 JPG、PNG、TIF".into()),
    };
    // TIFF AssociatedAlpha samples are already multiplied by alpha. The GPU
    // contract is straight alpha, so undo that multiplication before upload.
    let image = if associated_alpha {
        let mut rgba = image.into_rgba8();
        unpremultiply(&mut rgba);
        DynamicImage::ImageRgba8(rgba)
    } else {
        image
    };
    let (width, height) = preview_size(image.width(), image.height());
    let image = if (width, height) == (image.width(), image.height()) {
        image
    } else {
        image.resize_exact(width, height, FilterType::Triangle)
    };
    Ok(ColorTexture {
        width,
        height,
        rgba: image.into_rgba8().into_raw(),
        name: path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
    })
}

fn unpremultiply(image: &mut RgbaImage) {
    for pixel in image.pixels_mut() {
        let alpha = u32::from(pixel[3]);
        for channel in &mut pixel.0[..3] {
            *channel = if alpha == 0 {
                0
            } else {
                ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8
            };
        }
    }
}

fn jpeg(path: &Path) -> Result<DynamicImage, String> {
    let mut decoder =
        jpeg_decoder::Decoder::new(BufReader::new(File::open(path).map_err(|e| e.to_string())?));
    decoder.set_max_decoding_buffer_size(DECODE_BUDGET as usize);
    decoder.read_info().map_err(|e| e.to_string())?;
    let info = decoder.info().ok_or("JPEG 缺少尺寸")?;
    dimensions(info.width.into(), info.height.into())?;
    let (width, height) = preview_size(info.width.into(), info.height.into());
    decoder
        .scale(width as u16, height as u16)
        .map_err(|e| e.to_string())?;
    let data = decoder.decode().map_err(|e| e.to_string())?;
    let info = decoder.info().ok_or("JPEG 缺少图像信息")?;
    let mut rgba = Vec::with_capacity(info.width as usize * info.height as usize * 4);
    for pixel in data.chunks_exact(info.pixel_format.pixel_bytes()) {
        let rgb = match info.pixel_format {
            jpeg_decoder::PixelFormat::L8 => [pixel[0]; 3],
            jpeg_decoder::PixelFormat::L16 => {
                [(u16::from_ne_bytes([pixel[0], pixel[1]]) / 257) as u8; 3]
            }
            jpeg_decoder::PixelFormat::RGB24 => [pixel[0], pixel[1], pixel[2]],
            jpeg_decoder::PixelFormat::CMYK32 => {
                let channel =
                    |v: u8| ((255 - u16::from(v)) * (255 - u16::from(pixel[3])) / 255) as u8;
                [channel(pixel[0]), channel(pixel[1]), channel(pixel[2])]
            }
        };
        rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
    }
    Ok(DynamicImage::ImageRgba8(
        RgbaImage::from_raw(info.width.into(), info.height.into(), rgba)
            .ok_or("JPEG 像素数量与尺寸不一致")?,
    ))
}


