//! Export-only original DEM bricks. All hierarchy coordinates are global Y-up.
//! Retains no disk cache and never normalizes individual bricks independently.
use super::source_grid::{ExportGrid as SourceGrid, SourceMetadata};
use std::{collections::HashMap, fs::File, io::BufReader, path::Path};
use tiff::{decoder::Decoder, tags::Tag};

pub(super) const CORE: u32 = 512;
pub(super) const HALO: u32 = 64;
pub(super) const BRICK_EDGE: u32 = CORE + 2 * HALO;
pub(super) const COARSE_LEVEL: u32 = 5;

#[derive(Clone, Debug)]
pub(super) struct MipLevel {
    pub level: u32,
    pub edge: u32,
    pub values: Vec<f32>,
}
pub(super) struct Brick {
    /// Global base-texel coordinate of the lower-left halo, possibly negative.
    pub origin: [i32; 2],
    pub page: [u32; 2],
    pub levels: Vec<MipLevel>,
}
pub(super) struct PagedSource {
    source: SourceGrid,
    pub factor: u32,
    pub logical_size: [u32; 2],
    pub padded_size: u32,
    /// GEO_HEIGHT_SAMPLER shifts world lookup by one before clamping. This is
    /// a sampler transform, not a shift in the stored original samples.
    pub geo_halo: u32,
    curve: f32,
    resident_budget: usize,
    resident: Option<std::sync::Arc<Vec<f32>>>,
    tight_coarse: Option<Vec<MipLevel>>,
    visibility_coarse: Option<Vec<super::visibility_bounds::BoundsLevel>>,
}
fn valid(m: &SourceMetadata, v: f64) -> bool {
    v.is_finite() && !m.nodata.is_some_and(|n| n == v)
}
fn transform(m: &SourceMetadata, value: f64, curve: f32) -> f32 {
    let value = m.normalized(value);
    if value == 0. || curve == 1. {
        return value;
    }
    let q = ((value - 1e-6) / (1. - 1e-6)).clamp(0., 1.);
    q / (curve * (1. - q) + q) * (1. - 1e-6) + 1e-6
}
/// Exact dense 3x3 stride-two maximum, including the final clamped sample.
fn reduce(data: &[f32], edge: u32) -> Vec<f32> {
    let n = edge / 2;
    let mut out = vec![0.; (n * n) as usize];
    for y in 0..n {
        for x in 0..n {
            let mut maximum = 0f32;
            for dy in 0..3 {
                for dx in 0..3 {
                    maximum = maximum.max(
                        data[((2 * y + dy).min(edge - 1) * edge + (2 * x + dx).min(edge - 1))
                            as usize],
                    );
                }
            }
            out[(y * n + x) as usize] = maximum;
        }
    }
    out
}
impl PagedSource {
    /// Revalidate identity against the imported image before accessing export
    /// samples. Existing f64 extrema are authoritative; only an unreferenced
    /// UInt32 raster needs a new range scan because f32 display extrema cannot
    /// recover all original integer bits.
    pub fn open_for_map(
        preview: &super::HeightMap,
        factor: u32,
        curve: f32,
        geo_halo: u32,
        cache_budget: usize,
    ) -> Result<Self, String> {
        Self::open_for_map_with_progress(preview, factor, curve, geo_halo, cache_budget, |_, _| {
            Ok(())
        })
    }
    pub fn open_for_map_with_progress(
        preview: &super::HeightMap,
        factor: u32,
        curve: f32,
        geo_halo: u32,
        cache_budget: usize,
        mut progress: impl FnMut(u64, u64) -> Result<(), String>,
    ) -> Result<Self, String> {
        progress(0, 0)?;
        let original = preview.source.as_ref().ok_or("源精度导出缺少原始文件")?;
        if !super::source_grid::is_tiff(&original.path) {
            let result=Self::open(&original.path,preview.display_range.map(f64::from),factor,curve,geo_halo,cache_budget)?;
            if [result.source.metadata.width,result.source.metadata.height]!=[original.width,original.height] {
                return Err("源文件尺寸已改变，请重新导入".into());
            }
            let geo=super::georef::read_sidecars(&original.path)?;
            if serde_json::to_value(&geo).map_err(|e|e.to_string())?
                !=serde_json::to_value(&original.geo).map_err(|e|e.to_string())? {
                return Err("源文件地理参考已改变，请重新导入".into());
            }
            return Ok(result);
        }
        let mut decoder = Decoder::new(BufReader::new(
            File::open(&original.path).map_err(|e| e.to_string())?,
        ))
        .map_err(|e| e.to_string())?;
        let dimensions = decoder.dimensions().map_err(|e| e.to_string())?;
        if dimensions != (original.width, original.height) {
            return Err("源文件尺寸已改变，请重新导入".into());
        }
        let geo = super::georef::read_tiff_with_sidecars(&mut decoder,&original.path)?;
        if serde_json::to_value(&geo).map_err(|e| e.to_string())?
            != serde_json::to_value(&original.geo).map_err(|e| e.to_string())?
        {
            return Err("源文件地理参考已改变，请重新导入".into());
        }
        let color = decoder.colortype().map_err(|e| e.to_string())?;
        if !matches!(color, tiff::ColorType::Gray(_))
            || decoder.get_tag_u32(Tag::SamplesPerPixel).unwrap_or(1) != 1
        {
            return Err("源分页目前仅支持单波段 DEM TIFF".into());
        }
        let unsigned = decoder
            .get_tag_u32_vec(Tag::SampleFormat)
            .ok()
            .and_then(|formats| formats.first().copied())
            .unwrap_or(1)
            == 1;
        let bits = color.bit_depth();
        let range = if let Some(range) = preview.source_elevation_range {
            Some(range)
        } else if unsigned && matches!(bits, 8 | 16) {
            let divisor = if bits == 8 { 255. } else { 65535. };
            let range = preview
                .display_range
                .map(|value| (f64::from(value) * divisor).round());
            if range
                .iter()
                .any(|value| !value.is_finite() || *value < 0. || *value > divisor)
            {
                return Err("源高程范围无效".into());
            }
            Some(range)
        } else if unsigned && bits == 32 {
            None
        } else {
            return Err("源高程范围缺失，请重新导入".into());
        };
        drop(decoder);
        let mut result = Self::open(
            &original.path,
            range.unwrap_or([0., 1.]),
            factor,
            curve,
            geo_halo,
            cache_budget,
        )?;
        if range.is_none() {
            let metadata = result.source.metadata.clone();
            let mut minimum = f64::INFINITY;
            let mut maximum = f64::NEG_INFINITY;
            let total = u64::from(metadata.width) * u64::from(metadata.height);
            let mut scanned = 0;
            result.source.stream_rows(|_, _, values| {
                scanned += values.len() as u64;
                progress(scanned, total)?;
                for &value in values {
                    if valid(&metadata, value) {
                        minimum = minimum.min(value);
                        maximum = maximum.max(value);
                    }
                }
                Ok(())
            })?;
            if !minimum.is_finite() {
                return Err("TIFF 不含有效高程样本".into());
            }
            result.source.metadata.elevation_range = [minimum, maximum];
        }
        Ok(result)
    }
    pub fn open(
        path: &Path,
        range: [f64; 2],
        factor: u32,
        curve: f32,
        geo_halo: u32,
        cache_budget: usize,
    ) -> Result<Self, String> {
        if !matches!(factor, 1 | 2 | 4) || !curve.is_finite() || curve <= 0. || geo_halo > 1 {
            return Err("源分页参数无效".into());
        }
        let source = SourceGrid::open(path, range, cache_budget)?;
        let logical_size = [
            source.metadata.width.div_ceil(factor),
            source.metadata.height.div_ceil(factor),
        ];
        let padded_size = logical_size[0]
            .max(logical_size[1])
            .checked_next_power_of_two()
            .ok_or("源分页尺寸溢出")?
            .max(32);
        Ok(Self {
            source,
            factor,
            logical_size,
            padded_size,
            geo_halo,
            curve,
            resident_budget: 0,
            resident: None,
            tight_coarse: None,
            visibility_coarse: None,
        })
    }
    pub fn set_resident_budget(&mut self, bytes: usize) { self.resident_budget = bytes; }
    pub fn resident_samples(&self) -> Option<std::sync::Arc<Vec<f32>>> { self.resident.clone() }
    pub fn restore_resident(&mut self, samples: Option<std::sync::Arc<Vec<f32>>>) {
        self.resident = samples;
    }
    pub fn resident_bytes(&self) -> usize { self.resident.as_ref().map_or(0, |v| v.len()*4) }
    pub fn tight_coarse_levels(&self) -> Result<Vec<MipLevel>, String> {
        self.tight_coarse.clone().ok_or_else(|| "紧层级尚未完成源扫描".into())
    }
    pub fn visibility_coarse_levels(&self) -> Result<Vec<super::visibility_bounds::BoundsLevel>, String> {
        self.visibility_coarse.clone().ok_or_else(|| "可见性粗层尚未完成源扫描".into())
    }

    pub fn metadata(&self) -> &SourceMetadata {
        &self.source.metadata
    }

    /// Reads bounded source strips into a logical, top-down rectangle. The
    /// factor's tail cells average only existing valid samples, never zero pad.
    fn logical_rect(
        &mut self,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    ) -> Result<Vec<f32>, String> {
        if let Some(data) = &self.resident {
            let mut out = vec![0.; (width*height) as usize];
            let stride = self.logical_size[0] as usize;
            for row in 0..height as usize {
                let start = (y as usize+row)*stride+x as usize;
                out[row*width as usize..(row+1)*width as usize]
                    .copy_from_slice(&data[start..start+width as usize]);
            }
            return Ok(out);
        }
        let f = self.factor;
        let m = self.source.metadata.clone();
        let raw_x = x * f;
        let raw_y = y * f;
        let raw_w = (width * f).min(m.width - raw_x);
        let raw_h = (height * f).min(m.height - raw_y);
        if f == 1 {
            return Ok(self.source.read_window(raw_x, raw_y, raw_w, raw_h)?.values
                .into_iter().map(|v| transform(&m,v,self.curve)).collect());
        }
        let mut sums = vec![0f64; (width * height) as usize];
        let mut counts = vec![0u32; sums.len()];
        let row_batch = (1024 * 1024 / raw_w).max(1);
        for dy in (0..raw_h).step_by(row_batch as usize) {
            let rows = row_batch.min(raw_h - dy);
            let window = self.source.read_window(raw_x, raw_y + dy, raw_w, rows)?;
            for ry in 0..rows {
                for rx in 0..raw_w {
                    let v = window.values[(ry * raw_w + rx) as usize];
                    if valid(&m, v) {
                        let i = (((dy + ry) / f) * width + rx / f) as usize;
                        sums[i] += v;
                        counts[i] += 1;
                    }
                }
            }
        }
        Ok(sums
            .into_iter()
            .zip(counts)
            .map(|(s, n)| {
                transform(
                    &m,
                    if n > 0 { s / f64::from(n) } else { f64::NAN },
                    self.curve,
                )
            })
            .collect())
    }
    pub fn read_brick(&mut self, page_x: u32, page_y: u32) -> Result<Brick, String> {
        let px = page_x.checked_mul(CORE).ok_or("源分页坐标溢出")?;
        let py = page_y.checked_mul(CORE).ok_or("源分页坐标溢出")?;
        if px >= self.padded_size || py >= self.padded_size {
            return Err("源分页超出范围".into());
        }
        let origin = [
            i32::try_from(px).map_err(|_| "源分页坐标溢出")? - HALO as i32,
            i32::try_from(py).map_err(|_| "源分页坐标溢出")? - HALO as i32,
        ];
        let [w, h] = self.logical_size;
        let clamp_x = |x: i32| x.clamp(0, w as i32 - 1) as u32;
        let clamp_y = |y: i32| y.clamp(0, h as i32 - 1) as u32;
        let left = clamp_x(origin[0]);
        let right = clamp_x(origin[0] + BRICK_EDGE as i32 - 1);
        let bottom = clamp_y(origin[1]);
        let top = clamp_y(origin[1] + BRICK_EDGE as i32 - 1);
        let rw = right - left + 1;
        let values = self.logical_rect(left, h - 1 - top, rw, top - bottom + 1)?;
        let mut data = vec![0.; (BRICK_EDGE * BRICK_EDGE) as usize];
        let left_pad = (left as i32-origin[0]).clamp(0,BRICK_EDGE as i32) as usize;
        for y in 0..BRICK_EDGE {
            let gy = clamp_y(origin[1]+y as i32);
            let start = ((top-gy)*rw) as usize;
            let input = &values[start..start+rw as usize];
            let output = &mut data[(y*BRICK_EDGE) as usize..((y+1)*BRICK_EDGE) as usize];
            output[..left_pad].fill(input[0]);
            output[left_pad..left_pad+rw as usize].copy_from_slice(input);
            output[left_pad+rw as usize..].fill(input[input.len()-1]);
        }
        let mut levels = vec![MipLevel {
            level: 0,
            edge: BRICK_EDGE,
            values: data,
        }];
        for level in 1..COARSE_LEVEL {
            let previous = levels.last().unwrap();
            levels.push(MipLevel {
                level,
                edge: previous.edge / 2,
                values: reduce(&previous.values, previous.edge),
            });
        }
        Ok(Brick {
            origin,
            page: [page_x, page_y],
            levels,
        })
    }

    /// Sequential source scan, with at most native tile-band pending rows.
    /// Each L5 cell exactly covers [32*i,32*i+62] of dense base texels.
    pub fn build_coarse(&mut self) -> Result<Vec<MipLevel>, String> {
        self.build_coarse_with_progress(|_, _| Ok(()))
    }
    /// Progress counts original samples decoded; returning an error cancels
    /// between physical row segments without publishing a partial hierarchy.
    pub fn build_coarse_with_progress(
        &mut self,
        mut progress: impl FnMut(u64, u64) -> Result<(), String>,
    ) -> Result<Vec<MipLevel>, String> {
        let m = self.source.metadata.clone();
        let [w, h] = self.logical_size;
        let f = self.factor;
        let curve = self.curve;
        let n = self.padded_size >> COARSE_LEVEL;
        let mut coarse = vec![0f32; (n * n) as usize];
        let mut tight = if std::env::var("DEMSTUDIO_TIGHT_BOUNDS").as_deref()==Ok("on") {
            let padded=self.logical_size[0].max(self.logical_size[1]).checked_add(self.geo_halo*2)
                .and_then(u32::checked_next_power_of_two).ok_or("紧层级尺寸溢出")?.max(32);
            Some(super::tight_bounds::CoarseAccumulator::new(self.logical_size,padded,self.geo_halo)?)
        }else{None};
        let mut visibility = if std::env::var("DEMSTUDIO_VISIBILITY_PROBE").as_deref()==Ok("on") {
            let padded=self.logical_size[0].max(self.logical_size[1]).checked_add(self.geo_halo*2)
                .and_then(u32::checked_next_power_of_two).ok_or("可见性粗层尺寸溢出")?.max(32);
            Some(super::visibility_bounds::CoarseAccumulator::new(self.logical_size,padded,self.geo_halo)?)
        }else{None};
        // Capture the already normalized logical source during this mandatory
        // scan. Bounded by available RAM; never creates a disk cache.
        let samples = (u64::from(w)*u64::from(h)) as usize;
        let aligned = self.source.reduced_cells_align_with_chunks(f);
        let mut resident = if aligned && samples.checked_mul(4).is_some_and(|b| b <= self.resident_budget) {
            let mut data = Vec::new();
            if data.try_reserve_exact(samples).is_ok() { data.resize(samples, 0.); Some(data) }
            else { None }
        } else { None };
        let mut pending: HashMap<u32, PendingRow> = HashMap::new();
        let total = u64::from(m.width) * u64::from(m.height);
        let mut scanned = 0u64;
        progress(0, total)?;
        self.source.stream_rows(|x, y, values| {
            scanned += values.len() as u64;
            progress(scanned, total)?;
            let ly = y / f;
            let row = pending.entry(ly).or_insert_with(|| PendingRow {
                sums: vec![0.; w as usize],
                counts: vec![0; w as usize],
                seen: 0,
            });
            for (i, &v) in values.iter().enumerate() {
                let lx = (x + i as u32) / f;
                if valid(&m, v) {
                    row.sums[lx as usize] += v;
                    row.counts[lx as usize] += 1;
                }
            }
            row.seen += values.len() as u64;
            let expected = u64::from(m.width) * u64::from(f.min(m.height - ly * f));
            if row.seen == expected {
                let row = pending.remove(&ly).unwrap();
                let gy = h - 1 - ly;
                for lx in 0..w {
                    let count = row.counts[lx as usize];
                    let value = transform(
                        &m,
                        if count > 0 {
                            row.sums[lx as usize] / f64::from(count)
                        } else {
                            f64::NAN
                        },
                        curve,
                    );
                    if let Some(data) = resident.as_mut() { data[(ly*w+lx) as usize] = value; }
                    project_max(&mut coarse, n, lx, gy, w, h, value);
                    if let Some(builder)=tight.as_mut(){builder.project_sample(lx,gy,value)?;}
                    if let Some(builder)=visibility.as_mut(){builder.project_sample(lx,gy,value)?;}
                }
            } else if row.seen > expected {
                return Err("源分页扫描行重复".into());
            }
            Ok(())
        })?;
        if !pending.is_empty() {
            return Err("源分页扫描行不完整".into());
        }
        self.resident = resident.map(std::sync::Arc::new);
        self.tight_coarse = tight.map(|builder|builder.finish()).transpose()?;
        self.visibility_coarse = visibility.map(|builder|builder.finish()).transpose()?;
        let mut levels = vec![MipLevel {
            level: COARSE_LEVEL,
            edge: n,
            values: coarse,
        }];
        while levels.last().unwrap().edge > 1 {
            let p = levels.last().unwrap();
            levels.push(MipLevel {
                level: p.level + 1,
                edge: p.edge / 2,
                values: reduce(&p.values, p.edge),
            });
        }
        Ok(levels)
    }
}
struct PendingRow {
    sums: Vec<f64>,
    counts: Vec<u32>,
    seen: u64,
}
fn project_max(out: &mut [f32], n: u32, x: u32, y: u32, w: u32, h: u32, value: f32) {
    if value == 0. {
        return;
    }
    // Samples on the last real row/column extend to the padded square's edge.
    let start_x = x.saturating_sub(62).div_ceil(32);
    let start_y = y.saturating_sub(62).div_ceil(32);
    let end_x = if x == w - 1 {
        n - 1
    } else {
        (x / 32).min(n - 1)
    };
    let end_y = if y == h - 1 {
        n - 1
    } else {
        (y / 32).min(n - 1)
    };
    for cy in start_y..=end_y {
        for cx in start_x..=end_x {
            let target = &mut out[(cy * n + cx) as usize];
            *target = target.max(value);
        }
    }
}


