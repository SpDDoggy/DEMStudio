use super::{source_grid::ExportGrid, HeightMap, RenderParams};
use std::sync::atomic::{AtomicBool, Ordering};
/// Normalize once with the imported source-wide range; never substitute preview
/// samples when the user requested original geometry.
pub(super) fn prepare(
    preview: &HeightMap,
    p: &RenderParams,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u32),
) -> Result<HeightMap, String> {
    if cancel.load(Ordering::Acquire) {
        return Err("已取消导出".into());
    }
    let factor = match p.export_terrain_scale.as_str() {
        "preview" => return Ok(preview.clone()),
        "source" => 1,
        "half" => 2,
        "quarter" => 4,
        _ => return Err("无效的模型精度".into()),
    };
    let Some(source) = &preview.source else {
        if factor == 1 {
            return Ok(preview.clone());
        }
        let w = preview.width.div_ceil(factor);
        let h = preview.height.div_ceil(factor);
        if w < 2 || h < 2 {
            return Err("模型至少需要 2×2 个采样点".into());
        }
        if preview.values.len() != preview.width as usize * preview.height as usize {
            return Err("地形网格尺寸无效".into());
        }
        let mut sums = vec![0f64; (w * h) as usize];
        let mut counts = vec![0u32; (w * h) as usize];
        for y in 0..preview.height {
            if cancel.load(Ordering::Acquire) {
                return Err("已取消导出".into());
            }
            for x in 0..preview.width {
                let value = preview.values[(y * preview.width + x) as usize];
                if !value.is_finite() {
                    return Err("模型参数无效".into());
                }
                let value = super::gl_renderer::display_sample(value, preview.display_range);
                if value > 0. {
                    let i = ((y / factor) * w + x / factor) as usize;
                    sums[i] += f64::from(value);
                    counts[i] += 1;
                }
            }
            progress((u64::from(y + 1) * 15 / u64::from(preview.height)) as u32);
        }
        if cancel.load(Ordering::Acquire) {
            return Err("已取消导出".into());
        }
        let color_range = super::gl_renderer::source_elevation_range(preview);
        let values = sums
            .into_iter()
            .zip(counts)
            .map(|(sum, n)| {
                if n == 0 {
                    0.
                } else {
                    (sum / f64::from(n)) as f32
                }
            })
            .collect();
        return Ok(HeightMap {
            width: w,
            height: h,
            values,
            display_range: [1e-6, 1.],
            source_elevation_range: Some([color_range[0], color_range[0] + color_range[1]]),
            span: preview.span * w.max(h) as f32 / preview.width.max(preview.height) as f32,
            ..preview.clone()
        });
    };
    // Share the paged renderer's format-specific source validation and range
    // resolution, including legacy integer TIFF units and f64 extrema scans.
    let metadata = super::paged_source::PagedSource::open_for_map_with_progress(
        preview,
        factor,
        1.,
        0,
        32 * 1024 * 1024,
        |_, _| {
            if cancel.load(Ordering::Acquire) {
                Err("已取消导出".into())
            } else {
                Ok(())
            }
        },
    )?
    .metadata()
    .clone();
    let mut grid = ExportGrid::open(&source.path, metadata.elevation_range, 32 * 1024 * 1024)?;
    let w = source.width.div_ceil(factor);
    let h = source.height.div_ceil(factor);
    let count = u64::from(w) * u64::from(h);
    if w < 2 || h < 2 {
        return Err("模型至少需要 2×2 个采样点".into());
    }
    if count > 16_777_216 {
        return Err("模型采样超过 1677 万点，请降低网格精度".into());
    }
    let mut sums = vec![0f64; count as usize];
    let mut weights = vec![0u32; count as usize];
    let mut scanned = 0u64;
    grid.stream_rows(|x, y, row| {
        if cancel.load(Ordering::Acquire) {
            return Err("已取消导出".into());
        }
        for (dx, &v) in row.iter().enumerate() {
            let value = metadata.normalized(v);
            if value > 0. {
                let i = ((y / factor) * w + (x + dx as u32) / factor) as usize;
                sums[i] += value as f64;
                weights[i] += 1;
            }
        }
        scanned += row.len() as u64;
        progress((scanned * 15 / (u64::from(source.width) * u64::from(source.height))) as u32);
        Ok(())
    })?;
    if cancel.load(Ordering::Acquire) {
        return Err("已取消导出".into());
    }
    let color_range = super::gl_renderer::source_elevation_range(preview);
    let values = sums
        .into_iter()
        .zip(weights)
        .map(|(s, n)| if n == 0 { 0. } else { (s / n as f64) as f32 })
        .collect();
    Ok(HeightMap {
        width: w,
        height: h,
        values,
        display_range: [1e-6, 1.],
        source_elevation_range: Some([color_range[0], color_range[0] + color_range[1]]),
        span: preview.span * w.max(h) as f32 / preview.width.max(preview.height) as f32,
        ..preview.clone()
    })
}
