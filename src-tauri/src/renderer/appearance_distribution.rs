//! Bounded preview statistics. These are never advertised as source-raster statistics.
use super::HeightMap;
use serde::Serialize;

const BIN_COUNT: usize = 256;
const MAX_INSPECTED: usize = 65_536;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppearanceDistribution {
    pub range: [f64; 2],
    pub bins: Vec<u32>,
    /// Sorted source values, bounded to MAX_INSPECTED preview cells.
    pub samples: Vec<f64>,
    pub sample_count: usize,
    pub inspected_count: usize,
    pub preview_pixel_count: usize,
    pub preview_size: [u32; 2],
    pub map_name: String,
    pub source: &'static str,
}

pub(super) fn distribution(map: &HeightMap) -> Result<AppearanceDistribution, String> {
    let [low, span] = super::gl_renderer::source_elevation_range(map);
    if !low.is_finite() || !span.is_finite() || span < 0. {
        return Err("高程范围无效".into());
    }
    let total = map.values.len();
    let inspected = total.min(MAX_INSPECTED);
    let mut bins = vec![0; BIN_COUNT];
    let mut samples = Vec::with_capacity(inspected);
    let mut sample_count = 0;
    let [minimum, maximum] = map.display_range.map(f64::from);
    // One deterministic midpoint per equal index stratum; work stays bounded even
    // for a full-resolution preview. Never scan or reopen the original raster.
    for i in 0..inspected {
        let index = (((2 * i as u128 + 1) * total as u128) / (2 * inspected as u128)) as usize;
        let value = f64::from(map.values[index]);
        // The import/rendering contract reserves zero for NoData/empty terrain.
        if !value.is_finite() || value <= 0. { continue; }
        let t = if maximum > minimum { ((value - minimum) / (maximum - minimum)).clamp(0., 1.) } else { 0. };
        let bin = ((t * BIN_COUNT as f64) as usize).min(BIN_COUNT - 1);
        bins[bin] += 1;
        samples.push(low + t * span);
        sample_count += 1;
    }
    samples.sort_unstable_by(f64::total_cmp);
    Ok(AppearanceDistribution {
        range: [low, low + span], bins, samples, sample_count, inspected_count: inspected,
        preview_pixel_count: total, preview_size: [map.width, map.height],
        map_name: map.name.clone(), source: "preview",
    })
}
