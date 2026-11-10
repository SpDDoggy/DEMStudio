//! Bounded original-resolution export. Source bricks share a bounded GPU cache; completed ray queries are memoized so a page fault never restarts a ray.
use super::*;
use crate::renderer::paged_source::{Brick, MipLevel, PagedSource};
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};

const PAGE_SLOTS: usize = 64;
const PAGE_BATCH: usize = 16;
const CPU_BRICK_BUDGET: usize = 512 * 1024 * 1024;

const COMMIT: &str = "#version 330 compatibility\nuniform sampler2DArray state_image;void main(){gl_FragColor=texelFetch(state_image,ivec3(ivec2(gl_FragCoord.xy),6),0);}";

#[derive(PartialEq)]
struct CoarseKey {
    path: std::path::PathBuf,
    modified: std::time::SystemTime,
    bytes: u64,
    dimensions: [u32; 2],
    range: [u64; 2],
    nodata: Option<u64>,
    factor: u32,
    curve: u32,
}
pub(super) struct CoarseCache {
    key: CoarseKey,
    levels: Vec<MipLevel>,
    resident: Option<std::sync::Arc<Vec<f32>>>,
}

pub struct PagedExport {
    source: PagedSource,
    path: glow::Program,
    direct: Option<glow::Program>,
    commit: glow::Program,
    coarse: glow::Texture,
    low: glow::Texture,
    state: [glow::Texture; 2],
    memo: [glow::Texture; 2],
    fbo: glow::Framebuffer,
    size: [u32; 2],
    layers: u32,
    page_table: glow::Texture,
    page_map: Vec<f32>,
    slots: Vec<Option<i32>>,
    ages: Vec<u64>,
    clock: u64,
    wide_mrt: bool,
    all_pages_resident: bool,
    residency_reason: &'static str,
    available_gpu_bytes: u64,
    gpu_layer_limit: u32,
    stats: ExportStats,
    bricks: VecDeque<(i32, Brick)>,
}

struct ExportStats {
    started: std::time::Instant,
    preparation_seconds: f64,
    draws: u64,
    feedback_reads: u64,
    feedback_seconds: f64,
    page_reads: u64,
    brick_cache_hits: u64,
    page_read_seconds: f64,
    page_uploads: u64,
    page_upload_seconds: f64,
    pixel_samples: u64,
}
impl ExportStats {
    fn new(started: std::time::Instant) -> Self {
        Self { started, preparation_seconds: started.elapsed().as_secs_f64(),
            draws: 0, feedback_reads: 0, feedback_seconds: 0., page_reads: 0,
            page_read_seconds: 0., brick_cache_hits: 0, page_uploads: 0, page_upload_seconds: 0., pixel_samples: 0 }
    }
}

// GlobalMemoryStatusEx is available on the existing Windows baseline. No
// dependency or disk caching is introduced. Unknown memory uses bounded paging.
fn source_resident_budget() -> usize {
    #[repr(C)]
    struct Memory { length: u32, load: u32, total: u64, available: u64,
        total_page: u64, available_page: u64, total_virtual: u64, available_virtual: u64, extended: u64 }
    #[link(name="kernel32")]
    extern "system" { fn GlobalMemoryStatusEx(info: *mut Memory) -> i32; }
    let mut info = Memory { length: std::mem::size_of::<Memory>() as u32, load: 0,
        total: 0, available: 0, total_page: 0, available_page: 0,
        total_virtual: 0, available_virtual: 0, extended: 0 };
    if unsafe { GlobalMemoryStatusEx(&mut info) } == 0 { return 0; }
    (info.available / 4).min(2 * 1024 * 1024 * 1024) as usize
}

fn cancelled(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("已取消导出".into())
    } else {
        Ok(())
    }
}

// Five packed R32F core levels; cross-page taps address their own resident page.
// Reserve at
// least half of currently available dedicated VRAM for render targets, driver
// overhead and other applications. Unknown capacity retains bounded paging.
fn full_residency_fits(pages: u64, layer_limit: u32, available_bytes: u64) -> bool {
    let page_bytes = (0..5).map(|level| u64::from(512u32 >> level).pow(2) * 4).sum::<u64>();
    pages > PAGE_SLOTS as u64 && pages <= u64::from(layer_limit)
        && pages.checked_mul(page_bytes).is_some_and(|bytes| bytes <= available_bytes / 2)
}

unsafe fn available_vram(gl: &glow::Context) -> u64 {
    // GL_NVX_gpu_memory_info reports approximate currently available KB.
    // Allocation is still checked; the extension is never a requirement.
    if !gl.supported_extensions().contains("GL_NVX_gpu_memory_info") { return 0; }
    const CURRENT_AVAILABLE_VIDMEM_NVX: u32 = 0x9049;
    gl.get_parameter_i32(CURRENT_AVAILABLE_VIDMEM_NVX).max(0) as u64 * 1024
}

impl GlRenderer {
    pub unsafe fn begin_source_export(
        &mut self,
        preview: &HeightMap,
        desired: &RenderParams,
        factor: u32,
        geo: bool,
        cancel: &AtomicBool,
        mut progress: impl FnMut(u64, u64),
    ) -> Result<PagedExport, String> {
        let started = std::time::Instant::now();
        cancelled(cancel)?;
        let original = preview.source.as_ref().ok_or("源精度导出缺少原始文件")?;
        let mut source = PagedSource::open_for_map_with_progress(
            preview,
            factor,
            effective_height_curve(desired),
            u32::from(geo),
            512 * 1024 * 1024,
            |_, _| cancelled(cancel),
        )?;
        if [source.metadata().width, source.metadata().height] != [original.width, original.height]
        {
            return Err("源文件尺寸已改变，请重新导入".into());
        }
        let pages = u64::from(source.logical_size[0].div_ceil(512))
            * u64::from(source.logical_size[1].div_ceil(512));
        if pages >= 1 << 24 {
            return Err("源分页数量超出精确寻址范围".into());
        }
        if source.padded_size / 32 > self.gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE) as u32 {
            return Err("源高程层级超过当前 GPU 上限，请选择较低精度".into());
        }
        let file = std::fs::metadata(&original.path).map_err(|e| e.to_string())?;
        let key = CoarseKey {
            path: original.path.clone(),
            modified: file.modified().map_err(|e| e.to_string())?,
            bytes: file.len(),
            dimensions: [source.metadata().width, source.metadata().height],
            range: source.metadata().elevation_range.map(f64::to_bits),
            nodata: source.metadata().nodata.map(f64::to_bits),
            factor,
            curve: effective_height_curve(desired).to_bits(),
        };
        source.set_resident_budget(source_resident_budget());
        // One RAM-only hierarchy survives repeated exports. No .ds.cache or
        // decoded source bricks persist; changing geometry/source invalidates it.
        let cache = match self.paged_coarse_cache.take() {
            Some(cache) if cache.key == key => {
                progress(1, 1);
                cache
            }
            _ => {
                let levels = source.build_coarse_with_progress(|scanned, total| {
                    cancelled(cancel)?;
                    progress(scanned, total);
                    Ok(())
                })?;
                CoarseCache { key, levels, resident: source.resident_samples() }
            }
        };
        source.restore_resident(cache.resident.clone());
        let layers = crate::renderer::paged_shader::query_capacity(desired.max_bounces)?;
        if layers > self.gl.get_parameter_i32(glow::MAX_ARRAY_TEXTURE_LAYERS) as u32 {
            return Err("当前 GPU 的光线查询存储层数不足".into());
        }
        // Track each successfully allocated object so every construction error
        // releases its resources without touching the interactive preview.
        let mut textures = Vec::new();
        let mut programs = Vec::new();
        let mut fbos = Vec::new();
        let result = (|| {
            let wide_mrt = self.gl.get_parameter_i32(glow::MAX_DRAW_BUFFERS) >= 7
                && self.gl.get_parameter_i32(glow::MAX_COLOR_ATTACHMENTS) >= 7;
            let path = program(
                &self.gl,
                &crate::renderer::paged_shader::source_with_mrt(&path_source(desired, geo), wide_mrt)?,
            )?;
            programs.push(path);
            let commit = program(&self.gl, COMMIT)?;
            programs.push(commit);
            for _ in 0..7 {
                textures.push(self.gl.create_texture()?);
            }
            let coarse = textures[0];
            let mut low = textures[1];
            upload_levels(&self.gl, coarse, &cache.levels, 5);
            self.check("上传源高程层级")?;
            let available_gpu_bytes = available_vram(&self.gl);
            let gpu_layer_limit = self.gl.get_parameter_i32(glow::MAX_ARRAY_TEXTURE_LAYERS).max(0) as u32;
            let want_full = source.resident_bytes() != 0 && full_residency_fits(pages,
                gpu_layer_limit, available_gpu_bytes);
            let mut residency_reason = if want_full { "packed_full_residency" }
                else if source.resident_bytes() == 0 { "source_exceeds_ram_budget" }
                else if pages <= PAGE_SLOTS as u64 { "small_paged_source" }
                else if pages > u64::from(gpu_layer_limit) { "array_layer_limit" }
                else if available_gpu_bytes == 0 { "gpu_budget_unknown" }
                else { "gpu_memory_headroom" };
            let mut slots = if want_full { pages as usize } else { PAGE_SLOTS };
            if allocate_page_cache(&self.gl, low, slots, want_full).is_err() {
                if !want_full { return Err("源分页显存分配失败".into()); }
                let fallback = self.gl.create_texture()?;
                self.gl.delete_texture(low);
                low = fallback;
                textures[1] = low;
                slots = PAGE_SLOTS;
                residency_reason = "gpu_allocation_failed";
                allocate_page_cache(&self.gl, low, slots, false)?;
            }
            let across = source.logical_size[0].div_ceil(512);
            let down = source.logical_size[1].div_ceil(512);
            let page_map = vec![0f32; (across * down) as usize];
            upload_page_table(&self.gl, textures[6], across, down, &page_map, true);
            let fbo = self.gl.create_framebuffer()?;
            fbos.push(fbo);
            self.check("创建源分页导出")?;
            let mut export = PagedExport {
                source,
                path,
                direct: None,
                commit,
                coarse,
                low,
                state: [textures[2], textures[3]],
                memo: [textures[4], textures[5]],
                fbo,
                size: [0, 0],
                layers,
                page_table: textures[6],
                page_map,
                slots: vec![None; slots],
                ages: vec![0; slots],
                clock: 0,
                wide_mrt,
                all_pages_resident: want_full && slots == pages as usize,
                residency_reason,
                available_gpu_bytes,
                gpu_layer_limit,
                stats: ExportStats::new(started),
                bricks: VecDeque::new(),
            };
            if export.all_pages_resident {
                for first in (0..pages as i32).step_by(PAGE_BATCH) {
                    cancelled(cancel)?;
                    let batch: Vec<_> = (first..(first + PAGE_BATCH as i32).min(pages as i32)).collect();
                    export.load_pages(&self.gl, &batch)?;
                    if self.check("驻留源高程页").is_err() {
                        // Some drivers commit backing memory lazily on the
                        // first upload. Treat that failure like allocation
                        // failure, not a reason to reject a valid source DEM.
                        let fallback = self.gl.create_texture()?;
                        self.gl.delete_texture(export.low);
                        export.low = fallback;
                        textures[1] = fallback;
                        for _ in 0..16 {
                            if self.gl.get_error() == glow::NO_ERROR { break; }
                        }
                        allocate_page_cache(&self.gl, fallback, PAGE_SLOTS, false)?;
                        export.page_map.fill(0.);
                        export.slots = vec![None; PAGE_SLOTS];
                        export.ages = vec![0; PAGE_SLOTS];
                        export.clock = 0;
                        export.all_pages_resident = false;
                        export.residency_reason = "gpu_upload_failed";
                        upload_page_table(&self.gl, export.page_table, across, down, &export.page_map, false);
                        self.check("恢复有界源分页缓存")?;
                        export.bricks.clear();
                        break;
                    }
                    // GPU contains every page: no duplicate CPU brick LRU is needed.
                    export.bricks.clear();
                    progress((first as u64 + batch.len() as u64).min(pages), pages);
                }
            }
            if export.all_pages_resident {
                let direct = program(&self.gl,
                    &crate::renderer::resident_shader::source(&path_source(desired, geo))?)?;
                programs.push(direct);
                export.direct = Some(direct);
            }
            export.stats.preparation_seconds = started.elapsed().as_secs_f64();
            Ok(export)
        })();
        self.paged_coarse_cache = Some(cache);
        if result.is_err() {
            for tex in textures {
                self.gl.delete_texture(tex);
            }
            for p in programs {
                self.gl.delete_program(p);
            }
            for f in fbos {
                self.gl.delete_framebuffer(f);
            }
        } else if let Ok(ref export) = result {
            let size = export.source.logical_size;
            self.map = Some(MapGeometry {
                width: size[0],
                height: size[1],
                span: size[0].max(size[1]) as f32 * 0.2,
                source_elevation: source_elevation_range(preview),
                source_size: [original.width, original.height],
            });
            self.padded = export.source.padded_size;
            self.levels = self.padded.ilog2();
            self.paged_geometry = true;
            self.reset();
        }
        result
    }

    pub unsafe fn end_source_export(&mut self, export: PagedExport) {
        // No source/cache files are written. This diagnostic report records
        // CPU-observed waits, not GPU timer-query measurements.
        let s = &export.stats;
        let report = serde_json::json!({
            "version": 3, "source_size": export.source.logical_size,
            "resident_source_bytes": export.source.resident_bytes(),
            "wide_mrt": export.wide_mrt, "gpu_page_slots": export.slots.len(),
            "all_pages_resident": export.all_pages_resident,
            "direct_resident_integrator": export.direct.is_some(),
            "residency_reason": export.residency_reason,
            "available_gpu_bytes_at_preparation": export.available_gpu_bytes,
            "gpu_array_layer_limit": export.gpu_layer_limit,
            "gpu_renderer": self.gl.get_parameter_string(glow::RENDERER),
            "elapsed_seconds": s.started.elapsed().as_secs_f64(),
            "preparation_seconds": s.preparation_seconds,
            "state_draws": s.draws, "feedback_reads": s.feedback_reads,
            "feedback_wait_seconds": s.feedback_seconds,
            "source_page_reads": s.page_reads, "source_page_read_seconds": s.page_read_seconds,
            "cpu_brick_cache_hits": s.brick_cache_hits,
            "cpu_brick_cache_bytes": brick_cache_bytes(&export.bricks),
            "cpu_brick_cache_budget_bytes": CPU_BRICK_BUDGET,
            "gpu_page_uploads": s.page_uploads, "page_upload_submission_seconds": s.page_upload_seconds,
            "completed_pixel_samples": s.pixel_samples,
            "timing_scope": "CPU wall time; feedback includes GPU execution and transfer; may include pilot or cancelled work"
        });
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default().as_millis();
        let name = format!("DEMStudio-export-performance-{}-{stamp}.json", std::process::id());
        if let Ok(data) = serde_json::to_vec_pretty(&report) {
            let _ = std::fs::write(std::env::temp_dir().join(name), data);
        }
        for t in [
            export.coarse,
            export.low,
            export.page_table,
            export.state[0],
            export.state[1],
            export.memo[0],
            export.memo[1],
        ] {
            self.gl.delete_texture(t);
        }
        self.gl.delete_program(export.path);
        if let Some(direct) = export.direct { self.gl.delete_program(direct); }
        self.gl.delete_program(export.commit);
        self.gl.delete_framebuffer(export.fbo);
    }

    pub unsafe fn render_source_sample(
        &mut self,
        export: &mut PagedExport,
        cancel: &AtomicBool,
    ) -> Result<bool, String> {
        self.source_pass(export, cancel, false)?;
        export.stats.pixel_samples += u64::from(self.params.width) * u64::from(self.params.height);
        self.count += 1;
        self.output_dirty.set(true);
        Ok(true)
    }

    pub unsafe fn finish_source_surface(
        &mut self,
        export: &mut PagedExport,
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        if needs_surface(&self.params) && !self.surface_ready.get() {
            self.source_pass(export, cancel, true)?;
            self.surface_ready.set(true);
        }
        Ok(())
    }

    unsafe fn source_pass(
        &mut self,
        e: &mut PagedExport,
        cancel: &AtomicBool,
        surface: bool,
    ) -> Result<(), String> {
        cancelled(cancel)?;
        let size = [self.params.width, self.params.height];
        if u64::from(size[0]) * u64::from(size[1]) > 65536 {
            return Err("源分页导出工作块过大".into());
        }
        if let Some(direct) = e.direct {
            return self.resident_source_pass(e, direct, cancel, surface);
        }
        if e.size != size {
            e.allocate(&self.gl, size)?;
            self.check("分配源分页光线状态")?;
        }
        // prepare_sample advances the random sequence exactly once. All retries,
        // banks and query replays use these unchanged uniforms.
        let dense = self.path;
        self.path = e.path;
        let prepared = self.prepare_sample();
        self.path = dense;
        prepared?;
        self.gl.use_program(Some(e.path));
        self.bind_tex(e.path, "tex_sky_model", 3, self.sky_tex);
        self.i(e.path, "surface_pass", i32::from(surface));
        if surface {
            self.v2(e.path, "pixel_jitter", [0., 0.]);
        }
        self.gl.uniform_2_i32(
            self.gl
                .get_uniform_location(e.path, "pg_source_size")
                .as_ref(),
            e.source.logical_size[0] as i32,
            e.source.logical_size[1] as i32,
        );
        self.i(e.path, "pg_iteration_budget", 256);
        self.i(e.path, "pg_primary_cached", i32::from(self.geo_tile.is_some() && self.count > 0));
        e.bind_array(&self.gl, e.path, "pg_low_height", 4, e.low);
        self.bind_tex(e.path, "pg_page_table", 9, e.page_table);
        self.bind_tex(e.path, "pg_coarse_height", 5, e.coarse);
        e.bind_array(&self.gl, e.path, "pg_memo_a", 7, e.memo[0]);
        e.bind_array(&self.gl, e.path, "pg_memo_b", 8, e.memo[1]);
        self.gl.disable(glow::BLEND);
        self.gl.disable(glow::SCISSOR_TEST);
        let mut state = 0usize;
        for query in 0..e.layers {
            let mut first = true;
            loop {
                cancelled(cancel)?;
                self.gl.use_program(Some(e.path));
                e.bind_array(&self.gl, e.path, "pg_state_tex", 6, e.state[state]);
                self.i(e.path, "pg_query_index", query as i32);
                self.i(e.path, "pg_resume", i32::from(query != 0 || !first));
                self.i(e.path, "pg_reset_query", i32::from(first && query != 0));
                for bank in 0..if e.wide_mrt { 1 } else { 2 } {
                    e.attach(&self.gl, e.state[1 - state], bank)?;
                    self.i(e.path, "pg_bank", bank);
                    self.gl.draw_arrays(glow::TRIANGLES, 0, 3);
                    e.stats.draws += 1;
                }
                state = 1 - state;
                first = false;
                let feedback_started = std::time::Instant::now();
                let status = e.status(&self.gl, e.state[state]);
                e.stats.feedback_reads += 1;
                e.stats.feedback_seconds += feedback_started.elapsed().as_secs_f64();
                let decision = inspect_status(&status)?;
                // Cache the first deterministic world query even if the whole
                // path escapes immediately. Without this, all-miss tiles would
                // reuse uninitialized results on sample two.
                if query == 0 && decision.query_complete {
                    e.memoize(&self.gl, e.state[state], query);
                }
                if decision.all_paths_done {
                    self.gl.bind_framebuffer(
                        glow::FRAMEBUFFER,
                        Some(if surface {
                            self.surface_fbo
                        } else {
                            self.accum_fbo
                        }),
                    );
                    self.gl.draw_buffers(&[glow::COLOR_ATTACHMENT0]);
                    self.gl.use_program(Some(e.commit));
                    e.bind_array(&self.gl, e.commit, "state_image", 6, e.state[state]);
                    if !surface {
                        self.gl.enable(glow::BLEND);
                        let alpha = 1. / (self.count + 1) as f32;
                        self.gl.blend_color(alpha, alpha, alpha, alpha);
                        self.gl.blend_equation(glow::FUNC_ADD);
                        self.gl
                            .blend_func(glow::CONSTANT_ALPHA, glow::ONE_MINUS_CONSTANT_ALPHA);
                    }
                    self.gl.draw_arrays(glow::TRIANGLES, 0, 3);
                    self.gl.disable(glow::BLEND);
                    return self.check("源分页积分");
                }
                if decision.query_complete {
                    if query != 0 { e.memoize(&self.gl, e.state[state], query); }
                    break;
                }
                if !decision.pages.is_empty() {
                    e.load_pages(&self.gl, &decision.pages)?;
                    e.bind_array(&self.gl, e.path, "pg_low_height", 4, e.low);
                    self.bind_tex(e.path, "pg_page_table", 9, e.page_table);
                }
                self.check("继续源分页光线")?;
            }
        }
        Err("源分页光线查询超过已配置反弹上限".into())
    }

    unsafe fn resident_source_pass(&mut self, e: &mut PagedExport, direct: glow::Program,
        cancel: &AtomicBool, surface: bool) -> Result<(), String> {
        // Exactly the shared integrator, once per sample. The complete height
        // hierarchy is resident, so neither query replays nor CPU status reads
        // are necessary. RNG is advanced once, as in the paged fallback.
        if self.geo_tile.is_some() && e.size != [self.params.width,self.params.height] {
            e.allocate_primary_cache(&self.gl,[self.params.width,self.params.height])?;
        }
        let preview_path = self.path;
        self.path = direct;
        let prepared = if self.count == 0 || surface { self.prepare_sample() }
            else {
                self.gl.use_program(Some(direct));
                let params = self.params.clone();
                self.advance_sample_uniforms(direct, &params);
                Ok(())
            };
        if let Err(error) = prepared { self.path = preview_path; return Err(error); }
        self.begin_sample_batch();
        self.path = preview_path;
        let primary_cache = self.geo_tile.is_some();
        let write_primary = primary_cache && self.count == 0 && !surface;

        self.i(direct, "rs_primary_cached", i32::from(primary_cache && !write_primary));
        // Do not leave a texture sampled while it is attached for writing.
        for (name, unit, texture) in [
            ("rs_primary_hit",6, if primary_cache && !write_primary {e.state[0]} else {self.height_tex}),
            ("rs_primary_id",7, if primary_cache && !write_primary {e.state[1]} else {self.height_tex}),
        ] { self.bind_tex(direct, name, unit, texture); }
        if write_primary {
            // First sample replaces every target. Avoid blending undefined
            // newly allocated cache contents (including possible NaNs).
            self.gl.disable(glow::BLEND);
            for (attachment, texture) in [(glow::COLOR_ATTACHMENT1,e.state[0]),(glow::COLOR_ATTACHMENT2,e.state[1])] {
                self.gl.framebuffer_texture_2d(glow::FRAMEBUFFER,attachment,glow::TEXTURE_2D,Some(texture),0);
            }
            self.gl.draw_buffers(&[glow::COLOR_ATTACHMENT0,glow::COLOR_ATTACHMENT1,glow::COLOR_ATTACHMENT2]);
        }
        e.bind_array(&self.gl, direct, "rs_low", 4, e.low);
        self.bind_tex(direct, "rs_coarse", 5, e.coarse);
        self.gl.uniform_2_i32(self.gl.get_uniform_location(direct, "rs_source_size").as_ref(),
            e.source.logical_size[0] as i32, e.source.logical_size[1] as i32);
        if surface {
            self.i(direct, "surface_pass", 1);
            self.v2(direct, "pixel_jitter", [0., 0.]);
            self.gl.disable(glow::BLEND);
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.surface_fbo));
        }
        let result = (|| {
            // Limit each draw's coverage while preserving full-tile coordinates
            // and the same random sample. Cancellation remains between strips.
            for y in (0..self.params.height).step_by(16) {
                cancelled(cancel)?;
                self.draw_sample_strip(y, (self.params.height-y).min(16));
                e.stats.draws += 1;
            }
            self.check("常驻源光线积分")
        })();
        if write_primary {
            for attachment in [glow::COLOR_ATTACHMENT1,glow::COLOR_ATTACHMENT2] {
                self.gl.framebuffer_texture_2d(glow::FRAMEBUFFER,attachment,glow::TEXTURE_2D,None,0);
            }
            self.gl.draw_buffers(&[glow::COLOR_ATTACHMENT0]);
        }
        self.end_sample_batch();
        result
    }
}

impl PagedExport {
    unsafe fn allocate_primary_cache(&mut self, gl: &glow::Context, size: [u32;2]) -> Result<(),String> {
        gl.active_texture(glow::TEXTURE6);
        for texture in self.state {
            gl.bind_texture(glow::TEXTURE_2D,Some(texture));
            gl.tex_parameter_i32(glow::TEXTURE_2D,glow::TEXTURE_MIN_FILTER,glow::NEAREST as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D,glow::TEXTURE_MAG_FILTER,glow::NEAREST as i32);
            gl.tex_image_2d(glow::TEXTURE_2D,0,glow::RGBA32F as i32,
                size[0] as i32,size[1] as i32,0,glow::RGBA,glow::FLOAT,glow::PixelUnpackData::Slice(None));
            if gl.get_error()!=glow::NO_ERROR { return Err("主射线缓存分配失败".into()); }
        }
        self.size=size;
        Ok(())
    }
    unsafe fn bind_array(
        &self,
        gl: &glow::Context,
        p: glow::Program,
        name: &str,
        unit: u32,
        t: glow::Texture,
    ) {
        gl.active_texture(glow::TEXTURE0 + unit);
        gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(t));
        gl.uniform_1_i32(gl.get_uniform_location(p, name).as_ref(), unit as i32);
    }
    unsafe fn allocate(&mut self, gl: &glow::Context, size: [u32; 2]) -> Result<(), String> {
        for (tex, layers) in [
            (self.state[0], 7),
            (self.state[1], 7),
            (self.memo[0], self.layers),
            (self.memo[1], self.layers),
        ] {
            gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(tex));
            for (n, v) in [
                (glow::TEXTURE_MIN_FILTER, glow::NEAREST),
                (glow::TEXTURE_MAG_FILTER, glow::NEAREST),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
            ] {
                gl.tex_parameter_i32(glow::TEXTURE_2D_ARRAY, n, v as i32);
            }
            gl.tex_image_3d(
                glow::TEXTURE_2D_ARRAY,
                0,
                glow::RGBA32F as i32,
                size[0] as i32,
                size[1] as i32,
                layers as i32,
                0,
                glow::RGBA,
                glow::FLOAT,
                glow::PixelUnpackData::Slice(None),
            );
        }
        self.size = size;
        Ok(())
    }
    unsafe fn memoize(&self, gl: &glow::Context, state: glow::Texture, query: u32) {
        for (layer, tex) in [(3, self.memo[0]), (4, self.memo[1])] {
            gl.framebuffer_texture_layer(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, Some(state), 0, layer);
            gl.read_buffer(glow::COLOR_ATTACHMENT0);
            gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(tex));
            gl.copy_tex_sub_image_3d(glow::TEXTURE_2D_ARRAY, 0, 0, 0, query as i32,
                0, 0, self.size[0] as i32, self.size[1] as i32);
        }
    }
    unsafe fn attach(
        &self,
        gl: &glow::Context,
        tex: glow::Texture,
        bank: i32,
    ) -> Result<(), String> {
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbo));
        for k in 0..if self.wide_mrt { 7 } else { 4 } {
            let layer = if self.wide_mrt { k } else { bank * 4 + k };
            gl.framebuffer_texture_layer(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0 + k as u32,
                if layer < 7 { Some(tex) } else { None },
                0,
                layer.min(6),
            );
        }
        gl.draw_buffers(if self.wide_mrt {
            &[glow::COLOR_ATTACHMENT0, glow::COLOR_ATTACHMENT1, glow::COLOR_ATTACHMENT2,
              glow::COLOR_ATTACHMENT3, glow::COLOR_ATTACHMENT4, glow::COLOR_ATTACHMENT5, glow::COLOR_ATTACHMENT6]
        } else if bank == 0 {
            &[
                glow::COLOR_ATTACHMENT0,
                glow::COLOR_ATTACHMENT1,
                glow::COLOR_ATTACHMENT2,
                glow::COLOR_ATTACHMENT3,
            ]
        } else {
            &[
                glow::COLOR_ATTACHMENT0,
                glow::COLOR_ATTACHMENT1,
                glow::COLOR_ATTACHMENT2,
                glow::NONE,
            ]
        });
        if gl.check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE {
            return Err("源分页状态缓冲创建失败".into());
        }
        Ok(())
    }
    unsafe fn status(&self, gl: &glow::Context, tex: glow::Texture) -> Vec<f32> {
        gl.framebuffer_texture_layer(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, Some(tex), 0, 5);
        gl.read_buffer(glow::COLOR_ATTACHMENT0);
        let mut values = vec![0f32; (self.size[0] * self.size[1] * 4) as usize];
        let bytes =
            std::slice::from_raw_parts_mut(values.as_mut_ptr() as *mut u8, values.len() * 4);
        gl.read_pixels(
            0,
            0,
            self.size[0] as i32,
            self.size[1] as i32,
            glow::RGBA,
            glow::FLOAT,
            glow::PixelPackData::Slice(Some(bytes)),
        );
        values
    }
    unsafe fn load_pages(&mut self, gl: &glow::Context, pages: &[i32]) -> Result<(), String> {
        // Protect every page requested by this dispatch while selecting eviction
        // victims. If all slots are requested, oldest slots still make progress:
        // rays retain their traversal cursor on a fault.
        for &page in pages {
            if page < 0 || page as usize >= self.page_map.len() {
                return Err("源分页请求超出范围".into());
            }
            if let Some(slot) = self.slots.iter().position(|id| *id == Some(page)) {
                self.clock += 1;
                self.ages[slot] = self.clock;
            }
        }
        let mut loaded = 0;
        for &page in pages {
            if self.page_map[page as usize] > 0. { continue; }
            if loaded >= PAGE_BATCH { break; }
            let slot = eviction_slot(&self.slots, &self.ages, pages);
            let brick = if let Some(i) = self.bricks.iter().position(|(id, _)| *id == page) {
                self.stats.brick_cache_hits += 1;
                self.bricks.remove(i).unwrap().1
            } else {
                let across = self.source.logical_size[0].div_ceil(512);
                let read_started = std::time::Instant::now();
                let brick = self.source.read_brick(page as u32 % across, page as u32 / across)?;
                self.stats.page_reads += 1;
                self.stats.page_read_seconds += read_started.elapsed().as_secs_f64();
                brick
            };
            let upload_started = std::time::Instant::now();
            gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(self.low));
            for level in &brick.levels {
                let packed;
                let (edge, values) = if self.all_pages_resident {
                    packed = crate::renderer::resident_shader::packed_level(level);
                    (512 >> level.level, packed.as_slice())
                } else { (level.edge, level.values.as_slice()) };
                gl.tex_sub_image_3d(glow::TEXTURE_2D_ARRAY, level.level as i32,
                    0, 0, slot as i32, edge as i32, edge as i32, 1,
                    glow::RED, glow::FLOAT,
                    glow::PixelUnpackData::Slice(Some(bytes(values))));
            }
            self.stats.page_uploads += 1;
            self.stats.page_upload_seconds += upload_started.elapsed().as_secs_f64();
            if let Some(old) = self.slots[slot] { self.page_map[old as usize] = 0.; }
            self.slots[slot] = Some(page);
            self.page_map[page as usize] = (slot + 1) as f32;
            self.clock += 1;
            self.ages[slot] = self.clock;
            retain_brick(&mut self.bricks, page, brick, CPU_BRICK_BUDGET);
            loaded += 1;
        }
        let across = self.source.logical_size[0].div_ceil(512);
        let down = self.source.logical_size[1].div_ceil(512);
        upload_page_table(gl, self.page_table, across, down, &self.page_map, false);
        Ok(())
    }

}

fn brick_bytes(brick: &Brick) -> usize {
    brick.levels.iter().map(|level| level.values.len() * std::mem::size_of::<f32>()).sum()
}

fn brick_cache_bytes(bricks: &VecDeque<(i32, Brick)>) -> usize {
    bricks.iter().map(|(_, brick)| brick_bytes(brick)).sum()
}

fn retain_brick(bricks: &mut VecDeque<(i32, Brick)>, page: i32, brick: Brick, budget: usize) {
    // CPU retention must outlive the 64-page GPU working set. A shorter CPU
    // list only duplicates GPU-resident pages and misses again after eviction.
    let incoming = brick_bytes(&brick);
    if incoming > budget { return; }
    let mut retained = brick_cache_bytes(bricks);
    while retained > budget - incoming {
        let Some((_, oldest)) = bricks.pop_front() else { break; };
        retained -= brick_bytes(&oldest);
    }
    bricks.push_back((page, brick));
}

fn eviction_slot(slots: &[Option<i32>], ages: &[u64], requested: &[i32]) -> usize {
    slots.iter().position(Option::is_none).unwrap_or_else(|| {
        (0..slots.len()).min_by_key(|&i| (requested.contains(&slots[i].unwrap()), ages[i])).unwrap()
    })
}

unsafe fn allocate_page_cache(gl: &glow::Context, tex: glow::Texture, slots: usize, packed: bool) -> Result<(), String> {
    gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(tex));
    gl.tex_parameter_i32(glow::TEXTURE_2D_ARRAY, glow::TEXTURE_MIN_FILTER, glow::NEAREST_MIPMAP_NEAREST as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D_ARRAY, glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D_ARRAY, glow::TEXTURE_MAX_LEVEL, 4);
    for level in 0..5 {
        let edge = if packed { 512 } else { 640 } >> level;
        gl.tex_image_3d(glow::TEXTURE_2D_ARRAY, level, glow::R32F as i32,
            edge, edge, slots as i32, 0, glow::RED, glow::FLOAT,
            glow::PixelUnpackData::Slice(None));
        if gl.get_error() != glow::NO_ERROR { return Err("源分页显存分配失败".into()); }
    }
    Ok(())
}

unsafe fn upload_page_table(gl: &glow::Context, tex: glow::Texture, across: u32, down: u32, values: &[f32], allocate: bool) {
    gl.bind_texture(glow::TEXTURE_2D, Some(tex));
    if allocate {
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32);
        gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::R32F as i32, across as i32, down as i32,
            0, glow::RED, glow::FLOAT, glow::PixelUnpackData::Slice(Some(bytes(values))));
    } else {
        gl.tex_sub_image_2d(glow::TEXTURE_2D, 0, 0, 0, across as i32, down as i32,
            glow::RED, glow::FLOAT, glow::PixelUnpackData::Slice(Some(bytes(values))));
    }
}

unsafe fn upload_levels(gl: &glow::Context, tex: glow::Texture, levels: &[MipLevel], base: u32) {
    gl.bind_texture(glow::TEXTURE_2D, Some(tex));
    for (n, v) in [
        (glow::TEXTURE_MIN_FILTER, glow::NEAREST_MIPMAP_NEAREST),
        (glow::TEXTURE_MAG_FILTER, glow::NEAREST),
        (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
        (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
    ] {
        gl.tex_parameter_i32(glow::TEXTURE_2D, n, v as i32);
    }
    gl.tex_parameter_i32(
        glow::TEXTURE_2D,
        glow::TEXTURE_MAX_LEVEL,
        (levels.len() - 1) as i32,
    );
    for level in levels {
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            (level.level - base) as i32,
            glow::R32F as i32,
            level.edge as i32,
            level.edge as i32,
            0,
            glow::RED,
            glow::FLOAT,
            glow::PixelUnpackData::Slice(Some(bytes(&level.values))),
        );
    }
}

struct DispatchDecision {
    all_paths_done: bool,
    query_complete: bool,
    pages: Vec<i32>,
}
fn inspect_status(status: &[f32]) -> Result<DispatchDecision, String> {
    let mut done = 0;
    let mut complete = 0;
    let mut pages = BTreeMap::<i32, usize>::new();
    for p in status.chunks_exact(4) {
        if p.iter().any(|v| !v.is_finite()) {
            return Err("源分页状态包含无效数值".into());
        }
        match p[1] as i32 {
            4 => {
                done += 1;
                complete += 1;
            }
            3 => complete += 1,
            1 if p[0] >= 0. => {
                *pages.entry(p[0] as i32).or_default() += 1;
            }
            2 => {}
            _ => return Err("源分页状态无效".into()),
        }
    }
    let total = status.len() / 4;
    let mut pages: Vec<_> = pages.into_iter().collect();
    pages.sort_by_key(|&(page, count)| (std::cmp::Reverse(count), page));
    Ok(DispatchDecision {
        all_paths_done: done == total,
        query_complete: complete == total,
        pages: pages.into_iter().map(|(page, _)| page).collect(),
    })
}
