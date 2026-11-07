//! Pixel-aligned GeoTIFF extent expansion. Terrain lighting can have arbitrarily
//! long shadows near the horizon, so the export margin is explicitly chosen by
//! the user rather than inferred as a supposedly complete shadow boundary.

use super::{HeightMap, RenderParams};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExportExtent {
    pub width: u32,
    pub height: u32,
    /// Original image's column offset inside the expanded image.
    pub left: u32,
    /// Original image's row offset inside the expanded image (top-to-bottom).
    pub top: u32,
}

impl ExportExtent {
    /// Compute on the actual export grid. In particular, do not round a preview
    /// margin first and then multiply it: that can add a whole source pixel.
    pub fn for_source(width: u32, height: u32, p: &RenderParams) -> Result<Self, String> {
        if width == 0 || height == 0 {
            return Err("导出栅格尺寸无效".into());
        }
        let padding = if p.export_shadow {
            if !p.shadow_padding.is_finite() || !(1. ..=200.).contains(&p.shadow_padding) {
                return Err("阴影外扩范围必须在 1% 至 200% 之间".into());
            }
            // f64 preserves every u32 dimension exactly and avoids f32 integer
            // rounding before ceil. The source pixel size remains unchanged.
            let margin = (f64::from(width.max(height)) * f64::from(p.shadow_padding) / 100.).ceil();
            if margin > f64::from(u32::MAX) {
                return Err("外扩后的栅格尺寸过大".into());
            }
            margin as u32
        } else {
            0
        };
        let added = padding.checked_mul(2).ok_or("外扩后的栅格尺寸过大")?;
        Ok(Self {
            width: width.checked_add(added).ok_or("外扩后的栅格尺寸过大")?,
            height: height.checked_add(added).ok_or("外扩后的栅格尺寸过大")?,
            left: padding,
            top: padding,
        })
    }
}

pub fn shadow_extent(map: &HeightMap, p: &RenderParams) -> Result<ExportExtent, String> {
    ExportExtent::for_source(map.width, map.height, p)
}


