//! Bounded original-resolution export. Source bricks share a bounded GPU cache; completed ray queries are memoized so a page fault never restarts a ray.
use super::*;
use crate::renderer::paged_source::{Brick, MipLevel, PagedSource};
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};



const PAGE_SLOTS: usize = 64;
const PAGE_BATCH: usize = 16;
const CPU_BRICK_BUDGET: usize = 512 * 1024 * 1024;
include!("source_batch.rs");
include!("source_async.rs");
include!("source_wavefront.rs");

#[derive(Clone, Copy, PartialEq)]
enum GpuStage { Integrate = 0, Feedback = 1, Upload = 2, Commit = 3, Memo = 4, Output = 5, Batch = 6,
    BatchIntegrate = 7, BatchReduction = 8, BatchController = 9, BatchCommit = 10, BatchQueue = 11, LeafBounds = 12, GeoMerged = 13, AncestorSeed = 14, BicubicRows = 15, DirectionalSkip = 16, PrimarySunCert = 17 }
#[derive(Clone, Copy, PartialEq, Debug)]
enum GpuTimerMode { Off, Coarse, Detailed }
impl GpuTimerMode {
    fn parse(value:Option<&str>)->Self {
        match value {Some("off")=>Self::Off,Some("detailed")=>Self::Detailed,_=>Self::Coarse}
    }
    fn name(self)->&'static str {match self {Self::Off=>"off",Self::Coarse=>"coarse",Self::Detailed=>"detailed"}}
    fn measures(self,stage:GpuStage)->bool {
        match self {Self::Off=>false,Self::Coarse=>(stage as usize)<=GpuStage::Batch as usize||matches!(stage,GpuStage::LeafBounds|GpuStage::GeoMerged|GpuStage::AncestorSeed|GpuStage::BicubicRows|GpuStage::DirectionalSkip|GpuStage::PrimarySunCert),Self::Detailed=>true}
    }
}
const GPU_STAGE_COUNT: usize = 18;
const TIMER_QUERY_LIMIT: usize = 1024;
/// All intervals contribute to a bounded histogram; reported quantiles are bin upper
/// bounds with at most 1% rounding (plus 1 ns), rather than a biased sample window.
#[derive(Clone)]
struct GpuIntervalDistribution { bins: Vec<u64>, count: u64, max_ns: u64, latest_ns: u64 }
impl GpuIntervalDistribution {
    fn new() -> Self { Self { bins: vec![0;4096], count:0, max_ns:0, latest_ns:0 } }
    fn add(&mut self, ns:u64) {
        let bin=if ns==0 {0} else {((ns as f64).ln()/1.01f64.ln()).ceil() as usize+1};
        let bin=bin.min(self.bins.len()-1);self.bins[bin]+=1;self.count+=1;
        self.max_ns=self.max_ns.max(ns);self.latest_ns=ns;
    }
    fn quantile_ms(&self,p:f64)->Option<f64> {
        if self.count==0{return None;}
        let rank=(p*self.count as f64).ceil().max(1.) as u64;let mut seen=0;
        for (bin,n) in self.bins.iter().enumerate(){seen+=n;if seen>=rank {
            return Some(if bin==0 {0.} else {1.01f64.powi(bin as i32-1).min(self.max_ns as f64)/1e6});
        }}None
    }
}
struct GpuTimers {
    pending: VecDeque<(glow::Query,glow::Query,GpuStage)>, free: Vec<glow::Query>,
    active: Vec<(glow::Query,glow::Query,GpuStage)>,
    ns: [u64; GPU_STAGE_COUNT], measured: [u64; GPU_STAGE_COUNT], skipped: [u64; GPU_STAGE_COUNT], created: usize,
    distributions: [GpuIntervalDistribution;GPU_STAGE_COUNT],
    latest_query: [Option<glow::Query>;GPU_STAGE_COUNT],
    enabled: bool, mode:GpuTimerMode, mode_skipped:[u64;GPU_STAGE_COUNT], drain_seconds: f64,
}
impl GpuTimers {
    fn new(gl: &glow::Context) -> Self {
        let v = gl.version();
        let mode=GpuTimerMode::parse(std::env::var("DEMSTUDIO_GPU_TIMERS").ok().as_deref());
        Self { pending: VecDeque::new(), free: Vec::new(), active:Vec::new(), ns: [0;GPU_STAGE_COUNT], measured: [0;GPU_STAGE_COUNT],
            skipped: [0;GPU_STAGE_COUNT], distributions:std::array::from_fn(|_|GpuIntervalDistribution::new()), latest_query:[None;GPU_STAGE_COUNT], created: 0,
            mode,mode_skipped:[0;GPU_STAGE_COUNT],
            enabled: (v.major > 3 || (v.major == 3 && v.minor >= 3)
                || gl.supported_extensions().contains("GL_ARB_timer_query"))
                && mode!=GpuTimerMode::Off, drain_seconds: 0. }
    }
    unsafe fn collect(&mut self, gl: &glow::Context, wait: bool) {
        while let Some(&(start,end,stage)) = self.pending.front() {
            if !wait && gl.get_query_parameter_u32(end, glow::QUERY_RESULT_AVAILABLE) == 0 { break; }
            self.pending.pop_front();
            let ns=gl.get_query_parameter_u64(end,glow::QUERY_RESULT).saturating_sub(gl.get_query_parameter_u64(start,glow::QUERY_RESULT));
            self.ns[stage as usize] = self.ns[stage as usize].saturating_add(ns);
            self.measured[stage as usize] += 1;self.distributions[stage as usize].add(ns);self.latest_query[stage as usize]=Some(start);
            self.free.push(start);self.free.push(end);
        }
    }
    unsafe fn allocate(&mut self,gl:&glow::Context)->Option<glow::Query>{
        if let Some(q)=self.free.pop(){Some(q)}else if self.created<TIMER_QUERY_LIMIT {
            match gl.create_query(){Ok(q)=>{self.created+=1;Some(q)},Err(_)=>None}
        }else{None}
    }
    unsafe fn begin(&mut self, gl: &glow::Context, stage: GpuStage) -> Option<glow::Query> {
        // Policy exclusion precedes collection: ordinary Draws must not poll
        // query availability or allocate timestamp objects in coarse mode.
        if !self.mode.measures(stage) {
            self.skipped[stage as usize]+=1;self.mode_skipped[stage as usize]+=1;return None;
        }
        if !self.enabled { self.skipped[stage as usize] += 1; return None; }
        self.collect(gl, false);
        let Some(start)=self.allocate(gl) else {self.skipped[stage as usize]+=1;return None;};
        let Some(end)=self.allocate(gl) else {self.free.push(start);self.skipped[stage as usize]+=1;return None;};
        gl.query_counter(start,glow::TIMESTAMP);self.active.push((start,end,stage));Some(start)
    }
    unsafe fn end(&mut self, gl: &glow::Context, stage: GpuStage, q: Option<glow::Query>) {
        if let Some(q)=q {if let Some(index)=self.active.iter().position(|&(start,_,s)|start==q&&s==stage){
            let (start,end,stage)=self.active.remove(index);gl.query_counter(end,glow::TIMESTAMP);self.pending.push_back((start,end,stage));
        }}
    }
    unsafe fn finish(&mut self, gl: &glow::Context) {
        // Cancellation/error closes every outstanding nested interval without leaking queries.
        for (start,end,stage) in self.active.drain(..).rev(){gl.query_counter(end,glow::TIMESTAMP);self.pending.push_back((start,end,stage));}
        let start = std::time::Instant::now();self.collect(gl, true);self.drain_seconds = start.elapsed().as_secs_f64();
        for q in self.free.drain(..) { gl.delete_query(q); }
    }
    fn interval_seconds(&self,stage:GpuStage,start:Option<glow::Query>)->Option<f64>{
        let start=start?;
        // An unresolved current query must never reuse a previous interval's value,
        // even when the pooled query object has the same native handle.
        if self.pending.iter().any(|&(q,_,_)|q==start)||self.active.iter().any(|&(q,_,_)|q==start){return None;}
        (self.latest_query[stage as usize]==Some(start)).then_some(self.distributions[stage as usize].latest_ns as f64/1e9)
    }
    fn report(&self) -> serde_json::Value {
        let names = ["integrator_draw", "state_readback", "height_upload", "commit_draw", "memo_copy", "output_conversion_readback","sample_batch",
            "batch_integration","batch_state_reduction","batch_controller","batch_commit","batch_queue","leaf_bounds_preparation","geo_merged_preparation","ancestor_seed_preparation","bicubic_rows_preparation","directional_skip_preparation","primary_sun_certificate_preparation"];
        let mut stages = serde_json::Map::new();
        for (i, name) in names.iter().enumerate() {let d=&self.distributions[i];stages.insert((*name).into(), serde_json::json!({
            "gpu_seconds":self.ns[i] as f64 / 1e9,"measured_intervals":self.measured[i],"skipped_intervals":self.skipped[i],
            "skipped_by_mode":self.mode_skipped[i],"enabled_by_mode":self.mode==GpuTimerMode::Detailed||(self.mode==GpuTimerMode::Coarse&&(i<=GpuStage::Batch as usize||i==GpuStage::LeafBounds as usize||i==GpuStage::GeoMerged as usize||i==GpuStage::AncestorSeed as usize||i==GpuStage::BicubicRows as usize||i==GpuStage::DirectionalSkip as usize||i==GpuStage::PrimarySunCert as usize)),
            "p50_ms":d.quantile_ms(0.50),"p95_ms":d.quantile_ms(0.95),"p99_ms":d.quantile_ms(0.99),
            "max_ms":if d.count>0 {Some(d.max_ns as f64/1e6)}else{None}})); }
        serde_json::json!({"supported":self.enabled,"mode":self.mode.name(),"stages":stages,"final_query_drain_cpu_seconds":self.drain_seconds,
            "max_query_objects":TIMER_QUERY_LIMIT,"query_mode":"timestamp_pairs",
            "percentile_method":"all measured intervals, fixed 4096-bin logarithmic histogram; upper-bound rounding <=1% plus 1 ns; max exact",
            "scope":"Coarse mode measures parent sample_batch and unbatched stages; batch_* children require explicit detailed mode. Mode-excluded intervals are counted as skipped_by_mode, not extrapolated. GPU command intervals, not instruction attribution. sample_batch contains its batch_* child stages; do not add parent and children. Batch integration includes traversal, roots, shading and bounces. CPU readback waits overlap GPU execution. Skipped intervals are not extrapolated. Allocation and shader compilation excluded."})
    }
}









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
    tight_levels: Option<Vec<MipLevel>>,
    visibility_levels: Option<Vec<crate::renderer::visibility_bounds::BoundsLevel>>,
    resident: Option<std::sync::Arc<Vec<f32>>>,
}

pub struct PagedExport {
    source: PagedSource,
    compact: Option<crate::renderer::compact_feedback::CompactFeedback>,
    scheduler: crate::renderer::page_scheduler::DemandScheduler,
    prefetch: Option<crate::renderer::page_prefetch::PagePrefetch>,
    last_checkpoint: std::time::Instant,
    progress_log: std::path::PathBuf,
    path: glow::Program,
    phase_paths: Option<[glow::Program;5]>,
    phase_compute: bool,
    async_path: Option<glow::Program>,
    async_phase_pair:Option<[glow::Program;2]>,
    async_fresh_pair:Option<[glow::Program;2]>,
    async_samples: Option<crate::renderer::async_samples::AsyncSamples>,
    async_active_queue:Option<crate::renderer::async_active_queue::AsyncActiveQueue>,
    async_active_queue_enabled:bool,
    async_queue_builds:u64,
    wavefront: Option<crate::renderer::wavefront_runtime::Runtime>,
    wavefront_reserved_bytes: u64,
    async_draws: u64,
    async_completed: u64,
    phase_queue: Option<crate::renderer::phase_queue::PhaseQueue>,
    phase_rounds: u64,
    raystate: bool,
    compute: bool,
    image_integrator: bool,
    interpolate: Option<glow::Program>,
    execution_steps: u32,
    max_batch_wait_seconds: f64,
    max_batch_wall_seconds: f64,
    budget_gpu_observations: u64, budget_wall_fallbacks: u64, budget_async_wall_observations:u64,
    bound_preparation_cpu_seconds: f64,
    boundary_done: u64, boundary_waiting: u64, boundary_active: u64, boundary_capacity: u64,
    execution_adjustments: u64,
    active_queue: Option<crate::renderer::active_queue::ActiveQueue>,
    batch: Option<crate::renderer::gpu_batch::GpuBatch>,
    bicubic_profile: Option<crate::renderer::bicubic_profile::BicubicProfile>,
    walk_profile: Option<crate::renderer::walk_profile::WalkProfile>,
    visibility_probe: Option<crate::renderer::visibility_diagnostic::Diagnostics>,
    batch_samples: u32,
    batch_steps: u32,
    batch_resume: bool,
    batch_syncs: u64,
    batch_completed: u64,
    batch_reductions: u64,
    activity_active: u64,
    activity_capacity: u64,
    activity_draws: u64,
    activity_zero: u64,
    state_layers: u32,
    commit: glow::Program,
    coarse: glow::Texture,
    leaf_bounds:Option<glow::Texture>,
    leaf_program:Option<glow::Program>,
    row_cache:Option<glow::Texture>,
    row_program:Option<glow::Program>,
    row_transform:Option<[f32;3]>,
    row_preparation_cpu_seconds:f64,
    tight_low: Option<glow::Texture>,
    tight_coarse: Option<glow::Texture>,
    tight_coarse_bytes: u64,
    geo_merged:Option<crate::renderer::geo_merged::GeoMerged>,
    geo_merged_preparation_cpu_seconds:f64,
    ancestor_seed:Option<crate::renderer::ancestor_seed::AncestorSeed>,
    ancestor_seed_preparation_cpu_seconds:f64,
    directional_skip:Option<crate::renderer::directional_skip::DirectionalSkip>,
    directional_skip_preparation_cpu_seconds:f64,
    
    
    
    
    
    
    
    query_only_reserved_bytes:u64,
    bicubic_bounds: bool,
    low: glow::Texture,
    state: [glow::Texture; 2],
    memo: [glow::Texture; 2],
    fbo: glow::Framebuffer,
    size: [u32; 2],
    layers: u32,
    page_table: glow::Texture,
    page_map: Vec<f32>,
    uploaded_pages: Vec<bool>,
    slots: Vec<Option<i32>>,
    ages: Vec<u64>,
    clock: u64,
    wide_mrt: bool,
    residency_reason: &'static str,
    available_gpu_bytes: u64,
    gpu_layer_limit: u32,
    stats: ExportStats,
    timers: GpuTimers,
    context: serde_json::Value,
    source_path: std::path::PathBuf,
    requested_mode: String,
    requested_params: serde_json::Value,
    bricks: VecDeque<(i32, CachedBrick)>,
}

struct ExportStats {
    started: std::time::Instant,
    preparation_seconds: f64,
    draws: u64,
    feedback_reads: u64,
    feedback_bytes: u64,
    feedback_seconds: f64,
    page_reads: u64,
    prefetch_reads: u64,
    brick_cache_hits: u64,
    page_read_seconds: f64,
    page_uploads: u64,
    unique_page_uploads: u64,
    repeated_page_uploads: u64,
    page_upload_seconds: f64,
    pixel_samples: u64,
    draw_submission_seconds: f64,
    status_inspection_seconds: f64,
    page_fault_rounds: u64,
    page_fault_requests: u64,
}
impl ExportStats {
    fn new(started: std::time::Instant) -> Self {
        Self { started, preparation_seconds: started.elapsed().as_secs_f64(),
            draws: 0, feedback_reads: 0, feedback_bytes: 0, feedback_seconds: 0., page_reads: 0, prefetch_reads: 0,
            page_read_seconds: 0., brick_cache_hits: 0, page_uploads: 0, unique_page_uploads: 0, repeated_page_uploads: 0, page_upload_seconds: 0., pixel_samples: 0, draw_submission_seconds: 0.,
            status_inspection_seconds: 0., page_fault_rounds: 0, page_fault_requests: 0 }
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

// Five halo R32F levels per slot. Keep at least three quarters of currently
// available VRAM outside the height cache for state/output/driver/desktop.
const PAGE_BYTES: u64 = (640*640 + 320*320 + 160*160 + 80*80 + 40*40) * 4;
fn page_cache_slots(pages: u64, layers: u32, available: u64, requested_mib: Option<u64>) -> usize {
    page_cache_slots_with_bounds(pages,layers,available,requested_mib,false,0)
}
fn page_cache_slots_with_bounds(pages:u64,layers:u32,available:u64,requested_mib:Option<u64>,tight:bool,fixed:u64)->usize {
    page_cache_slots_with_extra(pages,layers,available,requested_mib,if tight{crate::renderer::tight_bounds::LOW_BYTES}else{0},fixed)
}
fn page_cache_slots_with_extra(pages:u64,layers:u32,available:u64,requested_mib:Option<u64>,extra:u64,fixed:u64)->usize {
    let default_budget = if available == 0 { 256*1024*1024 } else { (available/4).min(1024*1024*1024) };
    let budget = requested_mib.and_then(|v|v.checked_mul(1024*1024)).unwrap_or(default_budget);
    let budget = if available == 0 { budget.min(256*1024*1024) } else { budget.min(available/4) };
    (budget.saturating_sub(fixed)/(PAGE_BYTES+extra)).max(1).min(pages.max(1)).min(u64::from(layers.max(1))) as usize
}

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
        let leaf_enabled=std::env::var("DEMSTUDIO_BICUBIC_LEAF_BOUNDS").as_deref()==Ok("on");
        let row_enabled=std::env::var("DEMSTUDIO_BICUBIC_ROW_CACHE").as_deref()==Ok("on");
        let async_fresh_enabled=std::env::var("DEMSTUDIO_ASYNC_FRESH_SPLIT").as_deref()==Ok("on");
        if async_fresh_enabled&&(!geo||std::env::var("DEMSTUDIO_ASYNC_SAMPLES").as_deref()!=Ok("on")||std::env::var("DEMSTUDIO_ASYNC_FUSED_COMMIT").as_deref()!=Ok("on")||["DEMSTUDIO_ASYNC_PHASE_PAIR","DEMSTUDIO_ASYNC_ACTIVE_QUEUE","DEMSTUDIO_ASYNC_PACKED_QUEUE","DEMSTUDIO_PHASE_QUEUE","DEMSTUDIO_PHASE_COMPUTE","DEMSTUDIO_ASYNC_ELIDE_COMPLETED_STORE"].iter().any(|v|std::env::var(v).as_deref()==Ok("on"))||std::env::var("DEMSTUDIO_SAMPLE_WAVEFRONT").as_deref()==Ok("4")){
            return Err("FreshSplit需要Geo AsyncSamples/FusedCommit，且不能与阶段对、队列、Wavefront或写回消除组合".into());
        }
        let async_pair_enabled=std::env::var("DEMSTUDIO_ASYNC_PHASE_PAIR").as_deref()==Ok("on");
        if std::env::var("DEMSTUDIO_ASYNC_MATTE_STATE_ELISION").as_deref()==Ok("on")&&(!geo||std::env::var("DEMSTUDIO_ASYNC_SAMPLES").as_deref()!=Ok("on")||std::env::var("DEMSTUDIO_ASYNC_FUSED_COMMIT").as_deref()!=Ok("on")||["DEMSTUDIO_ASYNC_PHASE_PAIR","DEMSTUDIO_ASYNC_FRESH_SPLIT","DEMSTUDIO_ASYNC_ACTIVE_QUEUE","DEMSTUDIO_ASYNC_ELIDE_COMPLETED_STORE","DEMSTUDIO_PHASE_QUEUE","DEMSTUDIO_PHASE_COMPUTE"].iter().any(|n|std::env::var(n).as_deref()==Ok("on"))||std::env::var("DEMSTUDIO_SAMPLE_WAVEFRONT").as_deref()==Ok("4")){
            return Err("接影状态省略仅支持Geo融合独立采样，不能组合阶段、队列、Fresh或完成写回省略".into());
        }
        let async_packed_queue=std::env::var("DEMSTUDIO_ASYNC_PACKED_QUEUE").as_deref()==Ok("on");
        let async_active_queue_enabled=std::env::var("DEMSTUDIO_ASYNC_ACTIVE_QUEUE").as_deref()==Ok("on");
        if async_packed_queue&&!async_active_queue_enabled{return Err("PackedQueue需要AsyncActiveQueue".into());}
        if async_active_queue_enabled{
            // Geo native exports use geo_tile, not output_tile. Their actual
            // varying is captured once per tile before any queued initialization.
            crate::renderer::async_active_queue::validate_mode(geo,true,std::env::var("DEMSTUDIO_ASYNC_SAMPLES").as_deref()==Ok("on"),std::env::var("DEMSTUDIO_ASYNC_FUSED_COMMIT").as_deref()==Ok("on"),async_pair_enabled,std::env::var("DEMSTUDIO_SAMPLE_WAVEFRONT").as_deref()==Ok("4"),async_packed_queue)?;
        }
        if async_pair_enabled&&(!geo||desired.surface!="bilinear"||std::env::var("DEMSTUDIO_ASYNC_SAMPLES").as_deref()!=Ok("on")||std::env::var("DEMSTUDIO_ASYNC_FUSED_COMMIT").as_deref()!=Ok("on")||std::env::var("DEMSTUDIO_SAMPLE_WAVEFRONT").as_deref()==Ok("4")){
            return Err("AsyncPhasePair需要Geo Bilinear、AsyncSamples=on、FusedCommit=on且不能启用SampleWavefront".into());
        }
        let root_rebuild_enabled=std::env::var("DEMSTUDIO_BICUBIC_ROOT_REBUILD").as_deref()==Ok("on");
        if std::env::var("DEMSTUDIO_BICUBIC_ROOT_STACKLESS").as_deref()==Ok("on")&&!root_rebuild_enabled{return Err("RootStackless需要RootRebuild=on".into());}
        if std::env::var("DEMSTUDIO_BICUBIC_ROOT_RIGHT_CACHE").as_deref()==Ok("on")&&!root_rebuild_enabled{return Err("RootRightCache需要RootRebuild=on".into());}
        if root_rebuild_enabled&&(desired.surface!="bicubic"||std::env::var("DEMSTUDIO_BICUBIC_STATIC_ROOT").as_deref()!=Ok("on")||std::env::var("DEMSTUDIO_BICUBIC_STACK").as_deref()==Ok("left")||std::env::var("DEMSTUDIO_BICUBIC_ROOT_KEYS").as_deref()==Ok("on")){
            return Err("BicubicRootRebuild需要Bicubic StaticRoot=on、RootStack=original、RootKeys=off".into());
        }
        let patch_gather_enabled=std::env::var("DEMSTUDIO_BICUBIC_PATCH_GATHER").as_deref()==Ok("on");
        if patch_gather_enabled&&(!geo||desired.surface!="bicubic"||!leaf_enabled||row_enabled||std::env::var("DEMSTUDIO_BICUBIC_STATIC_ROOT").as_deref()!=Ok("on")){
            return Err("BicubicPatchGather需要Geo Bicubic、LeafBounds/StaticRoot=on且RowCache=off".into());
        }
        if row_enabled&&(!geo||desired.surface!="bicubic"||!leaf_enabled){return Err("Bicubic行缓存仅支持Geo Bicubic且须开启LeafBounds".into());}
        if std::env::var("DEMSTUDIO_INTEGRATION_WORK").as_deref()==Ok("on")
            &&(!crate::renderer::walk_profile::enabled()||std::env::var("DEMSTUDIO_ASYNC_SAMPLES").as_deref()!=Ok("on")){
            return Err("IntegrationWork需要WalkProfile=on与AsyncSamples=on".into());
        }
        if leaf_enabled&&desired.surface!="bicubic" {return Err("叶界候选仅支持 Bicubic".into());}
        let tight_enabled=std::env::var("DEMSTUDIO_TIGHT_BOUNDS").as_deref()==Ok("on");
        let geo_merged_enabled=std::env::var("DEMSTUDIO_GEO_MERGED").as_deref()==Ok("on");
        let query_only_enabled=std::env::var("DEMSTUDIO_QUERY_ONLY").as_deref()==Ok("on");
        if query_only_enabled&&(!cfg!(test)||!geo||!["bilinear","bicubic"].contains(&desired.surface.as_str())
            ||std::env::var("DEMSTUDIO_ACTIVE_QUEUE").as_deref()!=Ok("off")
            ||["DEMSTUDIO_ASYNC_SAMPLES","DEMSTUDIO_PHASE_QUEUE","DEMSTUDIO_PHASE_COMPUTE","DEMSTUDIO_DIRECTIONAL_SKIP","DEMSTUDIO_PRIMARY_SUN_CERT","DEMSTUDIO_TIGHT_BOUNDS","DEMSTUDIO_GEO_MERGED","DEMSTUDIO_ANCESTOR_SEED","DEMSTUDIO_BILINEAR_LEAF_GATHER","DEMSTUDIO_VISIBILITY_PROBE","DEMSTUDIO_COHERENT_QUERY","DEMSTUDIO_BICUBIC_ROOT_V2","DEMSTUDIO_BICUBIC_ROOT_REBUILD","DEMSTUDIO_BICUBIC_LEAF_BOUNDS","DEMSTUDIO_BICUBIC_ROW_CACHE","DEMSTUDIO_BICUBIC_PATCH_GATHER"].iter().any(|n|std::env::var(n).as_deref()==Ok("on"))
            ||std::env::var("DEMSTUDIO_SAMPLE_WAVEFRONT").as_deref()==Ok("4")){return Err("QueryOnly requires test Geo Bilinear/Bicubic, ActiveQueue=off, other traversal/phase/async candidates disabled".into());}
        let primary_sun_cert_enabled=std::env::var("DEMSTUDIO_PRIMARY_SUN_CERT").as_deref()==Ok("on");
        if primary_sun_cert_enabled&&(!cfg!(test)||!geo||desired.surface!="bilinear"
            ||["DEMSTUDIO_ASYNC_SAMPLES","DEMSTUDIO_PHASE_QUEUE","DEMSTUDIO_PHASE_COMPUTE","DEMSTUDIO_DIRECTIONAL_SKIP","DEMSTUDIO_TIGHT_BOUNDS","DEMSTUDIO_GEO_MERGED","DEMSTUDIO_ANCESTOR_SEED","DEMSTUDIO_BILINEAR_LEAF_GATHER","DEMSTUDIO_VISIBILITY_PROBE","DEMSTUDIO_COHERENT_QUERY"].iter().any(|n|std::env::var(n).as_deref()==Ok("on"))
            ||std::env::var("DEMSTUDIO_SAMPLE_WAVEFRONT").as_deref()==Ok("4")){return Err("PrimarySunCert requires explicit test Geo Bilinear with other traversal/phase/async candidates disabled".into());}
        let directional_skip_enabled=std::env::var("DEMSTUDIO_DIRECTIONAL_SKIP").as_deref()==Ok("on");
        if directional_skip_enabled&&(!geo||desired.surface!="bilinear"||tight_enabled||geo_merged_enabled||leaf_enabled
            ||crate::renderer::directional_skip::memory_bytes(source.padded_size).is_none()
            ||["DEMSTUDIO_ASYNC_SAMPLES","DEMSTUDIO_PHASE_QUEUE","DEMSTUDIO_PHASE_COMPUTE"].iter().any(|n|std::env::var(n).as_deref()==Ok("on"))
            ||std::env::var("DEMSTUDIO_SAMPLE_WAVEFRONT").as_deref()==Ok("4")){return Err("DCSF requires Geo Bilinear image, padded32..32768, no tight/merged/leaf/async/phase/wavefront".into());}
        let ancestor_seed_enabled=std::env::var("DEMSTUDIO_ANCESTOR_SEED").as_deref()==Ok("on");
        let diagnostic_bicubic_seed=cfg!(test)&&desired.surface=="bicubic"
            &&std::env::var("DEMSTUDIO_BICUBIC_BOUNDS").as_deref()==Ok("precomputed")
            &&source.logical_size.iter().all(|&n|n.saturating_add(2)<=source.padded_size);
        if ancestor_seed_enabled&&(!geo||(desired.surface!="bilinear"&&!diagnostic_bicubic_seed)||tight_enabled||geo_merged_enabled){return Err("祖先播种仅支持Geo Bilinear或测试Bicubic显式precomputed，且与tight/merged互斥".into());}
        if geo_merged_enabled&&(!geo||desired.surface!="bilinear"||tight_enabled||leaf_enabled){return Err("Geo合并候选仅支持Geo Bilinear，且与tight/leaf互斥".into());}
        if tight_enabled && desired.surface!="bilinear" {return Err("紧界候选仅支持 Bilinear".into());}
        let probe_enabled=std::env::var("DEMSTUDIO_VISIBILITY_PROBE").as_deref()==Ok("on");
        let wavefront_enabled=std::env::var("DEMSTUDIO_SAMPLE_WAVEFRONT").as_deref()==Ok("4");
        if std::env::var("DEMSTUDIO_WAVEFRONT_PAIR").as_deref()==Ok("on")&&!wavefront_enabled{return Err("两阶段候选需要4路样本候选".into());}
        if std::env::var("DEMSTUDIO_WAVEFRONT_FUSED").as_deref()==Ok("on")&&(!wavefront_enabled||std::env::var("DEMSTUDIO_WAVEFRONT_PAIR").as_deref()==Ok("on")){return Err("全融合候选需要4路样本且与两阶段候选互斥".into());}
        if probe_enabled&&desired.surface!="bilinear" {return Err("可见性分类探针仅支持Bilinear".into());}
        let cache = match self.paged_coarse_cache.take() {
            Some(cache) if cache.key == key && (!tight_enabled || cache.tight_levels.is_some())
                &&(!probe_enabled||cache.visibility_levels.is_some()) => {
                progress(1, 1);
                cache
            }
            _ => {
                let levels = source.build_coarse_with_progress(|scanned, total| {
                    cancelled(cancel)?;
                    progress(scanned, total);
                    Ok(())
                })?;
                CoarseCache { key, levels, tight_levels:if tight_enabled{Some(source.tight_coarse_levels()?)}else{None},
                    visibility_levels:if probe_enabled{Some(source.visibility_coarse_levels()?)}else{None}, resident: source.resident_samples() }
            }
        };
        source.restore_resident(cache.resident.clone());
        let raystate = std::env::var("DEMSTUDIO_INTEGRATOR").as_deref() != Ok("replay");
        let state_layers = if raystate { crate::renderer::raystate_shader::STATE_LAYERS } else { crate::renderer::paged_shader::STATE_LAYERS };
        let layers = if raystate { 1 } else { crate::renderer::paged_shader::query_capacity(desired.max_bounces)? };
        if layers > self.gl.get_parameter_i32(glow::MAX_ARRAY_TEXTURE_LAYERS) as u32 {
            return Err("当前 GPU 的光线查询存储层数不足".into());
        }
        // Track each successfully allocated object so every construction error
        // releases its resources without touching the interactive preview.
        let mut textures = Vec::new();
        let mut programs = Vec::new();
        let mut fbos = Vec::new();
        let mut preparation_timers = GpuTimers::new(&self.gl);
        let result = (|| {
            let wide_mrt = self.gl.get_parameter_i32(glow::MAX_DRAW_BUFFERS) >= 7
                && self.gl.get_parameter_i32(glow::MAX_COLOR_ATTACHMENTS) >= 7;
            let version=self.gl.version();
            let requested_raystate=std::env::var("DEMSTUDIO_RAYSTATE").unwrap_or_else(|_|"image".into());
            let image_integrator=raystate && (version.major,version.minor)>=(4,3)
                && requested_raystate=="image";
            if crate::renderer::raystate_shader::compute_batch_enabled()&&(!raystate||requested_raystate!="compute"||!geo||desired.surface!="bilinear"||std::env::var("DEMSTUDIO_ASYNC_SAMPLES").as_deref()==Ok("on")||std::env::var("DEMSTUDIO_PHASE_QUEUE").as_deref()==Ok("on")||std::env::var("DEMSTUDIO_PHASE_COMPUTE").as_deref()==Ok("on")){
                return Err("ComputeBatch测试仅支持Geo Bilinear原Compute且关闭其他采样/阶段候选".into());
            }
            if async_active_queue_enabled&&!image_integrator{return Err("AsyncActiveQueue需要image积分器".into());}
            if wavefront_enabled && (!image_integrator || !geo || desired.surface!="bilinear"
                || ["DEMSTUDIO_ASYNC_SAMPLES","DEMSTUDIO_PHASE_QUEUE","DEMSTUDIO_PHASE_COMPUTE","DEMSTUDIO_VISIBILITY_PROBE","DEMSTUDIO_WALK_PROFILE"]
                    .iter().any(|name|std::env::var(name).as_deref()==Ok("on"))) {
                return Err("4路样本阶段候选仅支持地理Bilinear image路径，且须关闭其他队列/遍历诊断候选".into());
            }
            if probe_enabled&&(!image_integrator||std::env::var("DEMSTUDIO_PHASE_COMPUTE").as_deref()==Ok("on")){
                return Err("可见性分类探针仅支持image raystate且禁用PhaseCompute".into());
            }
            if crate::renderer::walk_profile::enabled()&&(!image_integrator||std::env::var("DEMSTUDIO_PHASE_COMPUTE").as_deref()==Ok("on")){
                return Err("遍历诊断仅支持image raystate（包括async/fused），不支持Compute或replay".into());
            }
            if std::env::var("DEMSTUDIO_WALK_CAPTURE").as_deref()==Ok("on")&&!crate::renderer::walk_profile::enabled(){
                return Err("射线捕获需要WalkProfile=on".into());
            }
            if crate::renderer::walk_profile::clock_enabled(){
                if !crate::renderer::walk_profile::enabled(){return Err("shader clock需要WalkProfile=on".into());}
                if !image_integrator||std::env::var("DEMSTUDIO_PHASE_COMPUTE").as_deref()==Ok("on"){return Err("shader clock仅支持image raystate且禁用PhaseCompute".into());}
                if !self.gl.supported_extensions().contains("GL_ARB_shader_clock"){return Err("当前GPU不支持GL_ARB_shader_clock".into());}
            }
            let diagnostic_compute=cfg!(test)&&raystate&&(version.major,version.minor)>=(4,3)&&requested_raystate=="compute";
            if ancestor_seed_enabled&&diagnostic_bicubic_seed&&!image_integrator{return Err("Bicubic祖先播种测试仅支持image/async积分".into());}
            if ancestor_seed_enabled&&(!(image_integrator||diagnostic_compute)||std::env::var("DEMSTUDIO_PHASE_COMPUTE").as_deref()==Ok("on")){return Err("祖先播种仅支持image raystate或测试Compute且禁用PhaseCompute".into());}
            if geo_merged_enabled&&(!image_integrator||std::env::var("DEMSTUDIO_PHASE_COMPUTE").as_deref()==Ok("on")){return Err("Geo合并候选仅支持image raystate且禁用PhaseCompute".into());}
            if patch_gather_enabled&&(!image_integrator||std::env::var("DEMSTUDIO_PHASE_COMPUTE").as_deref()==Ok("on")){
                return Err("BicubicPatchGather仅支持image raystate且禁用PhaseCompute".into());
            }
            if row_enabled&&(!image_integrator||std::env::var("DEMSTUDIO_PHASE_COMPUTE").as_deref()==Ok("on")){return Err("Bicubic行缓存仅支持image raystate且禁用PhaseCompute".into());}
            if std::env::var("DEMSTUDIO_BILINEAR_LEAF_GATHER").as_deref()==Ok("on")&&(!(image_integrator||diagnostic_compute)||desired.surface!="bilinear"||std::env::var("DEMSTUDIO_PHASE_COMPUTE").as_deref()==Ok("on")){
                return Err("LeafGather仅支持Bilinear image且禁用PhaseCompute".into());
            }
            if leaf_enabled&&!image_integrator{return Err("Bicubic叶界候选仅支持image raystate积分器".into());}
            if query_only_enabled&&(!raystate||!image_integrator){return Err("QueryOnly requires original image RayState".into());}
            ;
            ;
            
            if primary_sun_cert_enabled&&(!raystate||!image_integrator||!self.gl.supported_extensions().contains("GL_ARB_gpu_shader_fp64")||self.gl.get_parameter_i32(glow::MAX_TEXTURE_IMAGE_UNITS)<12){return Err("PrimarySunCert requires image RayState, FP64 and 12 samplers".into());}
            if directional_skip_enabled&&(!raystate||!image_integrator){return Err("DCSF requires image RayState".into());}
            if directional_skip_enabled&&(self.gl.get_parameter_i32(glow::MAX_TEXTURE_IMAGE_UNITS)<12||self.gl.get_parameter_i32(glow::MAX_COMBINED_TEXTURE_IMAGE_UNITS)<12||self.gl.get_parameter_i32(glow::MAX_COMPUTE_TEXTURE_IMAGE_UNITS)<6||self.gl.get_parameter_i32(glow::MAX_IMAGE_UNITS)<2){return Err("DCSF requires 12 fragment samplers and compute/image1".into());}
            let compute=raystate && (version.major,version.minor)>=(4,3)
                && requested_raystate=="compute";
            let path = if compute {compute_program(&self.gl,&crate::renderer::raystate_shader::source_compute(&path_source(desired,geo))?)?}
                else if image_integrator {program_with_vertex(&self.gl,
                    if std::env::var("DEMSTUDIO_ACTIVE_QUEUE").as_deref()==Ok("off") {VERT}else{crate::renderer::raystate_shader::IMAGE_QUEUE_VERTEX},
                    &{let source=crate::renderer::raystate_shader::source_image_fragment(&path_source(desired,geo))?;let source=if directional_skip_enabled{crate::renderer::directional_skip::transform(&source)?}else{source};
                    
                    source}).map_err(|error|if query_only_enabled{format!("QueryOnly integration: {error}")}else{error})?}
                else {program(&self.gl,&if raystate {crate::renderer::raystate_shader::source_with_mrt(&path_source(desired,geo),wide_mrt)?}
                else {crate::renderer::paged_shader::source_with_mrt(&path_source(desired,geo),wide_mrt)?})?};
            programs.push(path);
            let phase_compute=std::env::var("DEMSTUDIO_PHASE_COMPUTE").as_deref()==Ok("on");
            let async_path=if std::env::var("DEMSTUDIO_ASYNC_SAMPLES").as_deref()==Ok("on") {
                if std::env::var("DEMSTUDIO_SAMPLE_BATCH").as_deref()==Ok("off") {
                    return Err("独立像素采样需要启用采样批次".into());
                }
                if !image_integrator||!geo||std::env::var("DEMSTUDIO_PHASE_QUEUE").as_deref()==Ok("on") {
                    return Err("独立像素采样需要地理图像积分器，且不能同时启用阶段队列".into());
                }
                let source=if async_active_queue_enabled{if async_packed_queue{crate::renderer::async_active_queue::packed_fragment_source(&path_source(desired,geo))?}else{crate::renderer::async_active_queue::fragment_source(&path_source(desired,geo))?}}else{crate::renderer::raystate_shader::source_async_samples(&path_source(desired,geo))?};
                
                let p=program_with_vertex(&self.gl,crate::renderer::raystate_shader::IMAGE_QUEUE_VERTEX,&source)?;
                programs.push(p);Some(p)
            }else{None};
            let async_phase_pair=if async_pair_enabled{
                if !image_integrator{return Err("AsyncPhasePair需要image积分器".into());}
                let sources=crate::renderer::raystate_shader::source_async_phase_pair(&path_source(desired,geo))?;
                let mut pair=Vec::new();for source in sources{let p=program_with_vertex(&self.gl,crate::renderer::raystate_shader::IMAGE_QUEUE_VERTEX,&source)?;programs.push(p);pair.push(p);}
                Some(pair.try_into().map_err(|_|"AsyncPhasePair程序数量错误")?)
            }else{None};
            let async_fresh_pair=if async_fresh_enabled{
                if !image_integrator{return Err("FreshSplit需要image积分器".into());}
                let sources=crate::renderer::raystate_shader::source_async_fresh_split(&path_source(desired,geo))?;
                let mut pair=Vec::new();for source in sources{let p=program_with_vertex(&self.gl,crate::renderer::raystate_shader::IMAGE_QUEUE_VERTEX,&source)?;programs.push(p);pair.push(p);}
                Some(pair.try_into().map_err(|_|"FreshSplit程序数量错误")?)
            }else{None};
            let phase_paths=if image_integrator&&std::env::var("DEMSTUDIO_PHASE_QUEUE").as_deref()==Ok("on") {
                let dense=path_source(desired,geo);let mut compiled=Vec::new();
                for stage in 0..5 {
                    let p=if phase_compute&&stage>0 {
                        compute_program(&self.gl,&crate::renderer::raystate_shader::source_compute_phase(&dense,stage)?)?
                    }else{program_with_vertex(&self.gl,if stage==0{crate::renderer::raystate_shader::IMAGE_QUEUE_VERTEX}else{crate::renderer::phase_queue::VERTEX_SOURCE},
                        &crate::renderer::raystate_shader::source_image_phase(&dense,stage)?)?};
                    programs.push(p);compiled.push(p);
                }
                Some(compiled.try_into().map_err(|_|"阶段程序数量无效")?)
            }else{None};
            let interpolate=if compute {
                let p=program(&self.gl,crate::renderer::raystate_shader::INTERPOLATED_ST_FRAGMENT)?;
                programs.push(p);Some(p)
            }else{None};
            let commit_source=if image_integrator||(compute&&crate::renderer::raystate_shader::compute_batch_enabled()) {COMMIT.replace("uniform sampler2DArray",&format!("#extension GL_ARB_shader_storage_buffer_object : require\n{}\nuniform sampler2DArray",crate::renderer::gpu_batch::COMMIT_HEADER)).replace("void main(){","void main(){if(!rs_batch_commit_ready())discard;")}else{COMMIT.into()};
            let commit = program(&self.gl, &commit_source)?;
            programs.push(commit);
            for _ in 0..7 {
                textures.push(self.gl.create_texture()?);
            }
            let coarse = textures[0];
            let mut low = textures[1];
            if tight_enabled && (self.gl.get_parameter_i32(glow::MAX_TEXTURE_IMAGE_UNITS)<12
                ||self.gl.get_parameter_i32(glow::MAX_COMBINED_TEXTURE_IMAGE_UNITS)<12
                ||(compute&&self.gl.get_parameter_i32(glow::MAX_COMPUTE_TEXTURE_IMAGE_UNITS)<12)) {
                return Err("紧界候选需要至少12个纹理采样单元".into());
            }
            let source_sampler_units=if ancestor_seed_enabled&&leaf_enabled{13}else{12};
            if (geo_merged_enabled||ancestor_seed_enabled)&&(self.gl.get_parameter_i32(glow::MAX_TEXTURE_IMAGE_UNITS)<source_sampler_units
                ||self.gl.get_parameter_i32(glow::MAX_COMBINED_TEXTURE_IMAGE_UNITS)<source_sampler_units
                ||self.gl.get_parameter_i32(glow::MAX_COMPUTE_TEXTURE_IMAGE_UNITS)<6
                ||self.gl.get_parameter_i32(glow::MAX_IMAGE_UNITS)<2){return Err(format!("Geo合并/祖先播种需要{source_sampler_units}个片元纹理单元及Compute/image1支持"));}
            if row_enabled&&(self.gl.get_parameter_i32(glow::MAX_TEXTURE_IMAGE_UNITS)<12
                ||self.gl.get_parameter_i32(glow::MAX_COMBINED_TEXTURE_IMAGE_UNITS)<12){return Err("Bicubic行缓存需要12个纹理采样单元".into());}
            if leaf_enabled&&((version.major,version.minor)<(4,3)||version.is_embedded
                ||self.gl.get_parameter_i32(glow::MAX_TEXTURE_IMAGE_UNITS)<11
                ||self.gl.get_parameter_i32(glow::MAX_COMPUTE_TEXTURE_IMAGE_UNITS)<5
                ||self.gl.get_parameter_i32(glow::MAX_IMAGE_UNITS)<2){return Err("Bicubic叶界需要OpenGL4.3及纹理/图像单元支持".into());}
            let mut row_cache=if row_enabled{let t=self.gl.create_texture()?;textures.push(t);Some(t)}else{None};
            let row_started=std::time::Instant::now();
            let row_program=if row_enabled{let p=crate::renderer::bicubic_rows::create_program(&self.gl,desired.height_step>0.)?;programs.push(p);Some(p)}else{None};
            let mut row_preparation_cpu_seconds=if row_enabled{row_started.elapsed().as_secs_f64()}else{0.};
            let mut leaf_bounds=if leaf_enabled{let t=self.gl.create_texture()?;textures.push(t);Some(t)}else{None};
            let leaf_program=if leaf_enabled{let p=crate::renderer::bicubic_leaf::create_program(&self.gl)?;programs.push(p);Some(p)}else{None};
            let mut tight_low=if tight_enabled{let t=self.gl.create_texture()?;textures.push(t);Some(t)}else{None};
            let tight_coarse=if tight_enabled{let t=self.gl.create_texture()?;textures.push(t);Some(t)}else{None};
            let tight_coarse_bytes=cache.tight_levels.as_ref().filter(|_|tight_enabled).map_or(0,|v|v.iter().map(|l|l.values.len() as u64*4).sum());
            
            #[cfg(not(test))] let query_only_bytes=0u64;
            
            
            #[cfg(not(test))] let root_capture_bytes=0u64;
            
            #[cfg(not(test))] let root_only_cost_bytes=0u64;
            let primary_sun_cert_bytes=if primary_sun_cert_enabled{65536u64*16}else{0};
            let directional_skip_bytes=if directional_skip_enabled{crate::renderer::directional_skip::memory_bytes(source.padded_size).ok_or("DCSF budget invalid")?}else{0};
            let geo_merged_coarse_bytes=if geo_merged_enabled{crate::renderer::geo_merged::coarse_bytes(source.padded_size).ok_or("Geo合并粗层预算无效")?}else{0};
            let probe_bytes=cache.visibility_levels.as_ref().filter(|_|probe_enabled)
                .map_or(0,|v|v.iter().map(|l|l.values.len() as u64*8).sum::<u64>()+crate::renderer::visibility_diagnostic::COUNTER_BYTES as u64);
            // 4*65536 paths, at most 19 RGBA32F layers, four queues,
            // indirect commands and the largest supported CPU sample table.
            let wavefront_reserved_bytes=if wavefront_enabled{84*1024*1024}else{0};
            if let Some(t)=tight_coarse {
                let levels=cache.tight_levels.as_ref().ok_or("紧界粗层缓存缺失")?;
                if levels.first().is_none_or(|l|l.edge>self.gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE) as u32){return Err("紧界粗层超过纹理尺寸限制".into());}
                upload_levels(&self.gl,t,levels,5,None,&mut preparation_timers);
                self.check("上传紧界粗层")?;
            }
            // Geographic traversal has a two-cell logical border. Keep raw
            // mips if that border would exceed the existing coarse allocation.
            let bicubic_bounds=crate::renderer::bicubic_bounds::enabled(&desired.surface)
                &&source.logical_size.iter().all(|&n|n.saturating_add(2)<=source.padded_size);
            let bound_preparation_cpu_seconds=upload_levels(&self.gl, coarse, &cache.levels, 5,
                if bicubic_bounds{Some((source.logical_size,u32::from(geo)))}else{None}, &mut preparation_timers);
            self.check("上传源高程层级")?;
            let available_gpu_bytes = available_vram(&self.gl);
            let gpu_layer_limit = self.gl.get_parameter_i32(glow::MAX_ARRAY_TEXTURE_LAYERS).max(0) as u32;
            let requested_mode = std::env::var("DEMSTUDIO_DIAGNOSTIC_MODE").unwrap_or_else(|_| "auto".into());
            // D-053: bounded page cache, one paged integrator.
            let mut residency_reason = "unified_paged_budget";
            let mut slots = page_cache_slots_with_extra(pages, gpu_layer_limit, available_gpu_bytes,
                std::env::var("DEMSTUDIO_PAGE_CACHE_MIB").ok().and_then(|v|v.parse().ok()),if tight_enabled{crate::renderer::tight_bounds::LOW_BYTES}else{0}+if leaf_enabled{crate::renderer::bicubic_leaf::SLOT_BYTES}else{0}+if geo_merged_enabled{crate::renderer::geo_merged::LOW_BYTES}else{0}+if ancestor_seed_enabled{crate::renderer::ancestor_seed::SLOT_BYTES}else{0}+if row_enabled{crate::renderer::bicubic_rows::SLOT_BYTES}else{0},tight_coarse_bytes+probe_bytes+wavefront_reserved_bytes+geo_merged_coarse_bytes+directional_skip_bytes+primary_sun_cert_bytes+query_only_bytes+root_capture_bytes+root_only_cost_bytes+if async_active_queue_enabled{crate::renderer::async_active_queue::BYTES}else{0}+if crate::renderer::walk_profile::enabled(){crate::renderer::walk_profile::diagnostic_bytes()}else{0});
            loop {
                if allocate_page_cache(&self.gl, low, slots, false).is_ok()
                    && tight_low.is_none_or(|t|allocate_tight_page_cache(&self.gl,t,slots).is_ok())
                    &&leaf_bounds.is_none_or(|t|crate::renderer::bicubic_leaf::allocate(&self.gl,t,slots).is_ok())
                    &&row_cache.is_none_or(|t|{let started=std::time::Instant::now();let result=crate::renderer::bicubic_rows::allocate(&self.gl,t,slots);row_preparation_cpu_seconds+=started.elapsed().as_secs_f64();result.is_ok()}) { break; }
                if slots <= 1 { return Err("源分页显存分配失败".into()); }
                let fallback = self.gl.create_texture()?;
                self.gl.delete_texture(low);
                low = fallback; textures[1] = low;
                if let Some(old)=tight_low {let t=self.gl.create_texture()?;self.gl.delete_texture(old);let i=textures.iter().position(|&x|x==old).unwrap();textures[i]=t;tight_low=Some(t);}
                if let Some(old)=row_cache{let t=self.gl.create_texture()?;self.gl.delete_texture(old);let i=textures.iter().position(|&x|x==old).unwrap();textures[i]=t;row_cache=Some(t);}
                if let Some(old)=leaf_bounds{let t=self.gl.create_texture()?;self.gl.delete_texture(old);let i=textures.iter().position(|&x|x==old).unwrap();textures[i]=t;leaf_bounds=Some(t);}
                slots = (slots / 2).max(1);
                residency_reason = "paged_allocation_backoff";
            }
            let across = source.logical_size[0].div_ceil(512);
            let down = source.logical_size[1].div_ceil(512);
            let page_map = vec![0f32; (across * down) as usize];
            upload_page_table(&self.gl, textures[6], across, down, &page_map, true);
            let fbo = self.gl.create_framebuffer()?;
            fbos.push(fbo);
            self.check("创建源分页导出")?;
            let mut export = PagedExport {
                compact: None,
                scheduler: crate::renderer::page_scheduler::DemandScheduler::new(4096,8),
                prefetch: if std::env::var("DEMSTUDIO_PREFETCH").as_deref()==Ok("off") {None} else {
                    crate::renderer::page_prefetch::PagePrefetch::new(&original.path,source.metadata().elevation_range,
                        factor,effective_height_curve(desired),u32::from(geo),source.resident_samples(),32*1024*1024)
                },
                last_checkpoint: std::time::Instant::now(),
                progress_log: std::env::temp_dir().join(format!("DEMStudio-paging-progress-{}-{}.jsonl",std::process::id(),
                    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis())),
                source,
                path,
                phase_paths,phase_compute,phase_queue:None,phase_rounds:0,
                async_path,async_phase_pair,async_fresh_pair,async_samples:None,async_active_queue:None,async_active_queue_enabled,async_queue_builds:0,wavefront:None,wavefront_reserved_bytes,async_draws:0,async_completed:0,
                raystate,
                compute,
                image_integrator,
                interpolate,
                max_batch_wait_seconds: 0.,
                bound_preparation_cpu_seconds,budget_gpu_observations:0,budget_wall_fallbacks:0,budget_async_wall_observations:0,max_batch_wall_seconds: 0.,boundary_done:0,boundary_waiting:0,boundary_active:0,boundary_capacity:0,
                execution_adjustments: 0,
                execution_steps: std::env::var("DEMSTUDIO_COMPUTE_STEPS").ok().and_then(|v|v.parse::<u32>().ok()).unwrap_or(4).clamp(1,8),
                active_queue: None,
                bicubic_profile:None,walk_profile:None,visibility_probe:None,batch:None,batch_samples:2,batch_steps:1,batch_resume:false,batch_syncs:0,batch_completed:0,batch_reductions:0,
                activity_active:0,activity_capacity:0,activity_draws:0,activity_zero:0,
                state_layers,
                commit,
                coarse,
                tight_low,tight_coarse,tight_coarse_bytes,leaf_bounds,leaf_program,row_cache,row_program,row_transform:None,row_preparation_cpu_seconds,geo_merged:None,geo_merged_preparation_cpu_seconds:0.,ancestor_seed:None,ancestor_seed_preparation_cpu_seconds:0.,directional_skip:None,directional_skip_preparation_cpu_seconds:0.,
                
                
                
                
                
                
                
                query_only_reserved_bytes:query_only_bytes,
                bicubic_bounds,
                low,
                state: [textures[2], textures[3]],
                memo: [textures[4], textures[5]],
                fbo,
                size: [0, 0],
                layers,
                page_table: textures[6],
                page_map,
                uploaded_pages: vec![false;pages as usize],
                slots: vec![None; slots],
                ages: vec![0; slots],
                clock: 0,
                wide_mrt,
                residency_reason,
                available_gpu_bytes,
                gpu_layer_limit,
                stats: ExportStats::new(started),
                timers: std::mem::replace(&mut preparation_timers, GpuTimers::new(&self.gl)),
                context: serde_json::Value::Null,
                source_path: original.path.clone(),
                requested_mode,
                requested_params: serde_json::json!({"surface": desired.surface,"samples":desired.samples,
                    "max_bounces":desired.max_bounces,"factor":factor,"geo":geo}),
                bricks: VecDeque::new(),
            };
            if (compute||image_integrator) && std::env::var("DEMSTUDIO_ACTIVE_QUEUE").as_deref()!=Ok("off") {
                export.active_queue=match crate::renderer::active_queue::ActiveQueue::create(&self.gl,65536){
                    Ok(queue)=>queue,Err(error)=>{export.timers.finish(&self.gl);return Err(error);}
                };
            }
            if std::env::var("DEMSTUDIO_FEEDBACK").as_deref() != Ok("legacy") {
                export.compact = match crate::renderer::compact_feedback::CompactFeedback::create(&self.gl, pages as u32) {
                    Ok(value) => value,
                    Err(error) => { diagnostic_trace(&format!("compact_feedback_fallback {error}")); None }
                };
            }
            export.stats.preparation_seconds = started.elapsed().as_secs_f64();
            if (image_integrator||(compute&&crate::renderer::raystate_shader::compute_batch_enabled()))&&export.compact.is_some()&&std::env::var("DEMSTUDIO_SAMPLE_BATCH").as_deref()!=Ok("off") {
                export.batch=match crate::renderer::gpu_batch::GpuBatch::create(&self.gl){Ok(value)=>value,Err(error)=>{diagnostic_trace(&format!("sample_batch_fallback {error}"));None}};
            }
            if image_integrator&&desired.surface=="bicubic"&&crate::renderer::bicubic_profile::enabled(){
                export.bicubic_profile=match crate::renderer::bicubic_profile::BicubicProfile::create(&self.gl){
                    Ok(p)=>Some(p),Err(error)=>{
                        if let Some(q)=export.active_queue.take(){q.destroy(&self.gl);}
                        if let Some(b)=export.batch.take(){b.destroy(&self.gl);}
                        if let Some(c)=export.compact.take(){c.destroy(&self.gl);}
                        export.timers.finish(&self.gl);return Err(error);
                    }
                };
            }
            if export.phase_paths.is_some() {
                let created=if export.batch.is_none(){Err("阶段队列需要GPU批次控制".into())}
                    else{crate::renderer::phase_queue::PhaseQueue::create(&self.gl,65536)
                        .and_then(|q|q.ok_or_else(||"阶段队列硬件或容量不支持".into()))};
                match created {Ok(q)=>export.phase_queue=Some(q),Err(error)=>{
                    if let Some(q)=export.active_queue.take(){q.destroy(&self.gl);}
                    if let Some(b)=export.batch.take(){b.destroy(&self.gl);}
                    if let Some(c)=export.compact.take(){c.destroy(&self.gl);}
                    if let Some(p)=export.bicubic_profile.take(){p.destroy(&self.gl);}
                    export.timers.finish(&self.gl);return Err(error);
                }}
            }
            if crate::renderer::walk_profile::enabled(){
                export.walk_profile=match crate::renderer::walk_profile::WalkProfile::create(&self.gl){Ok(p)=>Some(p),Err(error)=>{
                    if let Some(q)=export.phase_queue.take(){q.destroy(&self.gl);}
                    if let Some(q)=export.active_queue.take(){q.destroy(&self.gl);}
                    if let Some(b)=export.batch.take(){b.destroy(&self.gl);}
                    if let Some(c)=export.compact.take(){c.destroy(&self.gl);}
                    if let Some(p)=export.bicubic_profile.take(){p.destroy(&self.gl);}
                    export.timers.finish(&self.gl);return Err(error);
                }};
            }
            if probe_enabled{
                let created=cache.visibility_levels.as_ref().ok_or("可见性诊断粗层缓存缺失".into())
                    .and_then(|levels|crate::renderer::visibility_diagnostic::Diagnostics::create(&self.gl,levels));
                export.visibility_probe=match created{Ok(value)=>Some(value),Err(error)=>{
                    if let Some(q)=export.phase_queue.take(){q.destroy(&self.gl);}
                    if let Some(q)=export.active_queue.take(){q.destroy(&self.gl);}
                    if let Some(b)=export.batch.take(){b.destroy(&self.gl);}
                    if let Some(c)=export.compact.take(){c.destroy(&self.gl);}
                    if let Some(p)=export.bicubic_profile.take(){p.destroy(&self.gl);}
                    if let Some(p)=export.walk_profile.take(){p.destroy(&self.gl);}
                    export.timers.finish(&self.gl);return Err(error);
                }};
            }
            if geo_merged_enabled{
                let started=std::time::Instant::now();
                let created=(||{
                    let merged=crate::renderer::geo_merged::GeoMerged::create(&self.gl,export.slots.len()as u32,export.source.padded_size)?;
                    let timer=export.timers.begin(&self.gl,GpuStage::GeoMerged);
                    let built=merged.prepare_coarse(&self.gl,export.low,export.coarse,export.source.logical_size,export.source.padded_size);
                    export.timers.end(&self.gl,GpuStage::GeoMerged,timer);
                    if let Err(error)=built{merged.destroy(&self.gl);return Err(error);}Ok(merged)
                })();
                export.geo_merged_preparation_cpu_seconds+=started.elapsed().as_secs_f64();
                export.geo_merged=match created{Ok(merged)=>Some(merged),Err(error)=>{
                    if let Some(p)=export.visibility_probe.take(){p.destroy(&self.gl);}
                    if let Some(q)=export.phase_queue.take(){q.destroy(&self.gl);}
                    if let Some(q)=export.active_queue.take(){q.destroy(&self.gl);}
                    if let Some(b)=export.batch.take(){b.destroy(&self.gl);}
                    if let Some(c)=export.compact.take(){c.destroy(&self.gl);}
                    if let Some(p)=export.bicubic_profile.take(){p.destroy(&self.gl);}
                    if let Some(p)=export.walk_profile.take(){p.destroy(&self.gl);}
                    export.timers.finish(&self.gl);return Err(error);
                }};
            }
            if ancestor_seed_enabled{
                let started=std::time::Instant::now();
                
                #[cfg(not(test))]
                let created=crate::renderer::ancestor_seed::AncestorSeed::create(&self.gl,export.slots.len()as u32,export.source.padded_size);
                export.ancestor_seed_preparation_cpu_seconds+=started.elapsed().as_secs_f64();
                export.ancestor_seed=match created{Ok(seed)=>Some(seed),Err(error)=>{
                    if let Some(m)=export.geo_merged.take(){m.destroy(&self.gl);}
                    if let Some(p)=export.visibility_probe.take(){p.destroy(&self.gl);}
                    if let Some(q)=export.phase_queue.take(){q.destroy(&self.gl);}
                    if let Some(q)=export.active_queue.take(){q.destroy(&self.gl);}
                    if let Some(b)=export.batch.take(){b.destroy(&self.gl);}
                    if let Some(c)=export.compact.take(){c.destroy(&self.gl);}
                    if let Some(p)=export.bicubic_profile.take(){p.destroy(&self.gl);}
                    if let Some(p)=export.walk_profile.take(){p.destroy(&self.gl);}
                    export.timers.finish(&self.gl);return Err(error);
                }};
            }
            if directional_skip_enabled{
                let started=std::time::Instant::now();
                let created=(||{
                    let skip=crate::renderer::directional_skip::DirectionalSkip::create(&self.gl,export.source.padded_size)?;
                    let timer=export.timers.begin(&self.gl,GpuStage::DirectionalSkip);
                    let built=skip.build(&self.gl,export.coarse,export.source.logical_size);
                    export.timers.end(&self.gl,GpuStage::DirectionalSkip,timer);
                    if let Err(error)=built{skip.destroy(&self.gl);return Err(error);}Ok(skip)
                })();
                export.directional_skip_preparation_cpu_seconds=started.elapsed().as_secs_f64();
                export.directional_skip=match created{Ok(skip)=>Some(skip),Err(error)=>{
                    if let Some(m)=export.ancestor_seed.take(){m.destroy(&self.gl);}
                    if let Some(m)=export.geo_merged.take(){m.destroy(&self.gl);}
                    if let Some(p)=export.visibility_probe.take(){p.destroy(&self.gl);}
                    if let Some(q)=export.phase_queue.take(){q.destroy(&self.gl);}
                    if let Some(q)=export.active_queue.take(){q.destroy(&self.gl);}
                    if let Some(b)=export.batch.take(){b.destroy(&self.gl);}
                    if let Some(c)=export.compact.take(){c.destroy(&self.gl);}
                    if let Some(p)=export.bicubic_profile.take(){p.destroy(&self.gl);}
                    if let Some(p)=export.walk_profile.take(){p.destroy(&self.gl);}
                    export.timers.finish(&self.gl);return Err(error);
                }};
            }
            
            
            
            export.checkpoint(&self.gl,"prepared",true);
            Ok(export)
        })();
        self.paged_coarse_cache = Some(cache);
        if result.is_err() {
            preparation_timers.finish(&self.gl);
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

    pub fn set_source_export_context(&mut self, export: &mut PagedExport, context: serde_json::Value) {
        export.context = context;
    }

    pub unsafe fn profile_source_output(&mut self, export: &mut PagedExport) -> Result<Vec<u8>, String> {
        let timer = export.timers.begin(&self.gl, GpuStage::Output);
        let result = self.export_pixels_for_output();
        export.timers.end(&self.gl, GpuStage::Output, timer);
        result
    }

    

    pub unsafe fn end_source_export(&mut self, mut export: PagedExport) {
        export.timers.finish(&self.gl);
        // Image integration disables color writes on the shared accumulation
        // FBO. Restore it even when a batch returns early on cancellation.
        self.gl.bind_framebuffer(glow::FRAMEBUFFER,Some(self.accum_fbo));
        self.gl.draw_buffers(&[glow::COLOR_ATTACHMENT0]);
        export.checkpoint(&self.gl,"cleanup",true);
        // No source/cache files are written. CPU wall and GPU command-interval
        // timers have different overlap semantics; retain both in the report.
        let s = &export.stats;
        let mut report = serde_json::json!({
            "version": 6, "build_identity":"D055-profile-20261110",
            "source_path":export.source_path,"requested_mode":export.requested_mode,
            "requested_params":export.requested_params,
            "gpu_timing": export.timers.report(), "native": export.context,
            "execution_mode": "paged", "source_size": export.source.logical_size,
            "resident_source_bytes": export.source.resident_bytes(),
            "wide_mrt": export.wide_mrt, "gpu_page_slots": export.slots.len(),
            "all_pages_resident": export.slots.iter().filter(|page|page.is_some()).count()==export.page_map.len(),
            "direct_resident_integrator": false,
            "residency_reason": export.residency_reason,
            "available_gpu_bytes_at_preparation": export.available_gpu_bytes,
            "gpu_array_layer_limit": export.gpu_layer_limit,
            "gpu_renderer": self.gl.get_parameter_string(glow::RENDERER),
            "elapsed_seconds": s.started.elapsed().as_secs_f64(),
            "preparation_seconds": s.preparation_seconds,
            "draw_submission_cpu_seconds": s.draw_submission_seconds,
            "status_inspection_cpu_seconds": s.status_inspection_seconds,
            "page_fault_rounds": s.page_fault_rounds, "page_fault_requests": s.page_fault_requests,
            "state_draws": s.draws, "feedback_reads": s.feedback_reads,
            "feedback_bytes":s.feedback_bytes,"compact_feedback":export.compact.is_some(),
            "height_cache_budget_bytes":export.slots.len() as u64 * (PAGE_BYTES+if export.tight_low.is_some(){crate::renderer::tight_bounds::LOW_BYTES}else{0}+if export.leaf_bounds.is_some(){crate::renderer::bicubic_leaf::SLOT_BYTES}else{0})+export.slots.len()as u64*if export.row_cache.is_some(){crate::renderer::bicubic_rows::SLOT_BYTES}else{0}+export.tight_coarse_bytes+export.geo_merged.as_ref().map_or(0,|m|m.memory_bytes())+export.ancestor_seed.as_ref().map_or(0,|m|m.memory_bytes())+export.directional_skip.as_ref().map_or(0,|m|m.bytes())+if std::env::var("DEMSTUDIO_PRIMARY_SUN_CERT").as_deref()==Ok("on"){65536u64*16}else{0}+export.query_only_reserved_bytes,
            "feedback_wait_seconds": s.feedback_seconds,
            "source_page_reads": s.page_reads, "source_page_read_seconds": s.page_read_seconds,
            "cpu_brick_cache_hits": s.brick_cache_hits,
            "cpu_brick_cache_bytes": brick_cache_bytes(&export.bricks),
            "cpu_brick_cache_budget_bytes": CPU_BRICK_BUDGET,
            "gpu_page_uploads": s.page_uploads, "page_upload_submission_seconds": s.page_upload_seconds,
            "unique_gpu_page_uploads":s.unique_page_uploads,"repeated_gpu_page_uploads":s.repeated_page_uploads,
            "completed_pixel_samples": s.pixel_samples,
            "timing_scope": "CPU wall time; feedback includes GPU execution and transfer; may include pilot or cancelled work"
        });
        report["bicubic_precomputed_bounds"]=serde_json::json!(export.bicubic_bounds);
        report["prefetch"]=serde_json::json!({"enabled":export.prefetch.is_some(),"page_reads":s.prefetch_reads,
            "decode_budget_bytes":32*1024*1024,"max_pending_pages":16});
        report["integrator_protocol"]=serde_json::json!(if export.raystate { "raystate_v1" } else { "query_replay" });
        report["status_inspection_scope"]=serde_json::json!("feedback acquisition plus CPU decoding; includes GPU wait");
        report["execution_adjustments"]=serde_json::json!(export.execution_adjustments);
        report["max_observed_feedback_wait_seconds"]=serde_json::json!(export.max_batch_wait_seconds);
        report["execution_budget_observations"]=serde_json::json!({"gpu":export.budget_gpu_observations,"wall_fallback":export.budget_wall_fallbacks,"async_wall":export.budget_async_wall_observations,
            "scope":"one observed controller interval per batch; async uses wall by design (included in wall_fallback), legacy batch attempts current GPU timestamps then wall; no synchronous query wait"});
        report["max_observed_feedback_wait_scope"]=serde_json::json!("maximum CPU wait for a feedback read group including unbatched, batch, async compact+progress and async end-tile diagnostic reads; overlaps GPU execution");
        report["async_gpu_timing_scope"]=serde_json::json!({"child_mode":"detailed only; default coarse excludes child queries before collection/allocation",
            "batch_integration":if export.async_fresh_pair.is_some(){"Fresh and Resume full-screen Fragment Draws; both may emit ordered fixed-function blend; image/storage/texture barrier between passes"}else if export.async_phase_pair.is_some(){"WORLD and OTHER full-screen Fragment Draws with ordered OTHER blend and image/storage/texture barriers"}else if std::env::var("DEMSTUDIO_ASYNC_FUSED_COMMIT").as_deref()==Ok("on"){"integrator Draw plus fused ordered blend and its image/storage/texture barrier"}else{"integrator Draw and its image/storage/texture barrier"},
            "batch_state_reduction":"compact state reduction dispatch",
            "batch_controller":"progress reduction dispatch plus sampled eligibility diagnostic dispatches",
            "batch_commit":"independent commit commands only; fused commit remains part of batch_integration",
            "parent_scope":"sample_batch includes children and intervening GPU command gaps; children are not an additive wall-clock decomposition"});
        report["async_elide_completed_store"]=serde_json::json!({"enabled":std::env::var("DEMSTUDIO_ASYNC_ELIDE_COMPLETED_STORE").as_deref()==Ok("on"),"scope":"fused non-final samples skip redundant DONE/radiance image stores; final sample retains state; resume skips overwritten layer5 load","extra_gpu_memory_bytes":0});
        report["async_matte_state_elision"]=serde_json::json!({"enabled":std::env::var("DEMSTUDIO_ASYNC_MATTE_STATE_ELISION").as_deref()==Ok("on"),"scope":"only MATTE phase5/6 loads and persists private layers15/16; ordinary paths retain prior image contents; original rs_run unchanged","extra_gpu_memory_bytes":0});
        report["bicubic_bound_preparation_cpu_seconds"]=serde_json::json!(export.bound_preparation_cpu_seconds);
        report["max_batch_cpu_wall_seconds"]=serde_json::json!(export.max_batch_wall_seconds);
        report["boundary_activity"]=serde_json::json!({"done":export.boundary_done,"waiting":export.boundary_waiting,"active":export.boundary_active,"capacity":export.boundary_capacity,"invalid":export.boundary_capacity.saturating_sub(export.boundary_done+export.boundary_waiting+export.boundary_active),"scope":"candidate-end GPU state counts; includes retries, not per-Draw activity"});
        report["compute_steps_per_feedback"]=serde_json::json!(export.execution_steps);
        report["bicubic_row_cache"]=serde_json::json!({"enabled":export.row_cache.is_some(),"extra_slot_bytes":if export.row_cache.is_some(){crate::renderer::bicubic_rows::SLOT_BYTES}else{0},
            "gpu_memory_bytes":export.slots.len()as u64*if export.row_cache.is_some(){crate::renderer::bicubic_rows::SLOT_BYTES}else{0},
            "height_transform":export.row_transform,"quantized":export.row_transform.is_some()&&self.params.height_step>0.,"step_scope":"actual uniform when quantized; unused positive placeholder otherwise","preparation_cpu_seconds":export.row_preparation_cpu_seconds,
            "preparation_cpu_scope":"shader compilation, texture allocation and build command submission; GPU intervals are bicubic_rows_preparation"});
        report["bicubic_patch_gather"]=serde_json::json!({"enabled":std::env::var("DEMSTUDIO_BICUBIC_PATCH_GATHER").as_deref()==Ok("on"),"scope":"integer internal raw 4x4 footprint via four gathers; boundary fallback original bc_raw; original height transform and both Bezier conversions remain local","extra_gpu_memory_bytes":0});
        report["bilinear_leaf_gather"]=serde_json::json!({"enabled":std::env::var("DEMSTUDIO_BILINEAR_LEAF_GATHER").as_deref()==Ok("on"),"scope":"LOD0 integer interior 2x2 raw footprint only; all other calls retain original four heightLod samples","extra_gpu_memory_bytes":0});
        report["bicubic_leaf_bounds"]=serde_json::json!({"enabled":export.leaf_bounds.is_some(),"extra_slot_bytes":if export.leaf_bounds.is_some(){crate::renderer::bicubic_leaf::SLOT_BYTES}else{0}});
        
        
        report["root_v2"]=serde_json::json!(std::env::var("DEMSTUDIO_BICUBIC_ROOT_V2").as_deref()==Ok("on"));
        
        report["directional_skip"]=serde_json::json!({"enabled":export.directional_skip.is_some(),"gpu_memory_bytes":export.directional_skip.as_ref().map_or(0,|m|m.bytes()),"preparation_cpu_seconds":export.directional_skip_preparation_cpu_seconds,"gpu_preparation_stage":"directional_skip_preparation","scope":"Geo Bilinear shadow coarse LOD>=5 directional rectangle cached across original microsteps; no direct cursor jump or new page demand","default":"off"});
        report["ancestor_seed"]=serde_json::json!({"enabled":export.ancestor_seed.is_some(),"extra_slot_bytes":if export.ancestor_seed.is_some(){crate::renderer::ancestor_seed::SLOT_BYTES}else{0},"texture_unit":if export.ancestor_seed.is_some(){Some(if export.leaf_bounds.is_some(){12}else{10})}else{None},"with_leaf_bounds":export.ancestor_seed.is_some()&&export.leaf_bounds.is_some(),
            "gpu_memory_bytes":export.ancestor_seed.as_ref().map_or(0,|m|m.memory_bytes()),"preparation_cpu_seconds":export.ancestor_seed_preparation_cpu_seconds,
            "scope":"new-query resident-page ancestor descent only; no additional rejection or page demand; quantify saved nodes with WalkProfile",
            "preparation_cpu_scope":"shader compilation, allocation and command submission; GPU execution overlaps; GPU intervals are ancestor_seed_preparation"});
        report["geo_merged"]=serde_json::json!({"enabled":export.geo_merged.is_some(),"semantics":"old_geo_max_only_no_extra_rejection","extra_slot_bytes":if export.geo_merged.is_some(){crate::renderer::geo_merged::LOW_BYTES}else{0},
            "coarse_bytes":if export.geo_merged.is_some(){crate::renderer::geo_merged::coarse_bytes(export.source.padded_size).unwrap_or(0)}else{0},"gpu_memory_bytes":export.geo_merged.as_ref().map_or(0,|m|m.memory_bytes()),
            "preparation_cpu_seconds":export.geo_merged_preparation_cpu_seconds,"preparation_cpu_scope":"shader compilation, allocation and command submission; GPU execution overlaps; GPU intervals are geo_merged_preparation"});
        report["tight_bounds"]=serde_json::json!({"enabled":export.tight_low.is_some(),"extra_slot_bytes":if export.tight_low.is_some(){crate::renderer::tight_bounds::LOW_BYTES}else{0},"coarse_bytes":export.tight_coarse_bytes});
        report["phase_queue"]=serde_json::json!({"enabled":export.phase_queue.is_some(),"rounds":export.phase_rounds,"bytes":export.phase_queue.as_ref().map(|q|q.memory_bytes()).unwrap_or(0),"integrator":if export.phase_compute{"compute_stage_specialized"}else{"fragment_stage_specialized"}});
        report["sample_wavefront"]=serde_json::json!({"enabled":export.wavefront.is_some(),"lanes":4,"reserved_bytes":export.wavefront_reserved_bytes,"actual_bytes":export.wavefront.as_ref().map_or(0,|w|w.memory_bytes()),"blocks":export.wavefront.as_ref().map_or(0,|w|w.blocks),"committed_samples":export.wavefront.as_ref().map_or(0,|w|w.committed_samples),"submitted_integration_draws":export.wavefront.as_ref().map_or(0,|w|w.integration_draws),"draw_scope":"includes initialization and zero-count indirect commands; excludes ordered commit","integrator":"fragment_stage_specialized","sample_commit":"original ordered constant alpha"});
        report["sample_wavefront"]["stage_partition"]=serde_json::json!(if std::env::var("DEMSTUDIO_WAVEFRONT_FUSED").as_deref()==Ok("on"){"fused original state machine"}else if std::env::var("DEMSTUDIO_WAVEFRONT_PAIR").as_deref()==Ok("on"){"WORLD / fused OTHER"}else{"WORLD / SURFACE / SHADOW / SCATTER"});
        if std::env::var("DEMSTUDIO_WAVEFRONT_FUSED").as_deref()==Ok("on"){report["sample_wavefront"]["integrator"]=serde_json::json!("fragment_fused_continuation");}
        report["independent_pixel_samples"]=serde_json::json!({
            "enabled":export.async_path.is_some(),"async_phase_pair":export.async_phase_pair.is_some(),"async_fresh_split":export.async_fresh_pair.is_some(),"integration_draws":export.async_draws,
            "committed_pixel_samples_after_first":export.async_completed,
            "bytes":export.async_samples.as_ref().map(|s|s.memory_bytes()).unwrap_or(0),
            "scope":"sample zero and primary memo retain original path; independent per-pixel cursors thereafter; original CPU random/alpha table and ordered dual-source fixed-function blending"
        });
        report["compute_integrator"]=serde_json::json!(export.compute);
        report["compute_workgroup_size"]=if export.compute{serde_json::json!(crate::renderer::raystate_shader::compute_group_size())}else{serde_json::Value::Null};
        report["image_fragment_integrator"]=serde_json::json!(export.image_integrator);
        report["active_queue_bytes"]=serde_json::json!(export.active_queue.as_ref().map_or(0,|q|q.memory_bytes()));
        report["async_active_queue"]=serde_json::json!({"enabled":export.async_active_queue_enabled,"reserved_bytes":if export.async_active_queue_enabled{crate::renderer::async_active_queue::BYTES}else{0},"actual_bytes":export.async_active_queue.as_ref().map_or(0,|q|q.memory_bytes()),"queue_builds":export.async_queue_builds,"dispatch_scope":"one command reset plus one eligibility scan per integration Draw; no CPU queue readback","packed":std::env::var("DEMSTUDIO_ASYNC_PACKED_QUEUE").as_deref()==Ok("on"),"integrator":if std::env::var("DEMSTUDIO_ASYNC_PACKED_QUEUE").as_deref()==Ok("on"){"packed eligible prefix on full TRIANGLES; original nonfused rs_run and independent fixed-function commit"}else{"unchanged async fused Fragment, one indirect POINTS Draw; original rs_run phase and traversal limits"},"coordinate_scope":"Geo only, output_tile absent; original raster varying captured once per tile in RG32F (original Geo jitter zero), fetched only for sample_ray initialization; original pixel centers","queue_bytes":crate::renderer::async_active_queue::QUEUE_BYTES,"interpolated_st_bytes":crate::renderer::async_active_queue::ST_BYTES,"capture_gpu_timing":"memo_copy stage, once per tile outside sample_batch","queue_gpu_timing":"sample_batch_queue only in detailed timer mode; parent sample_batch includes queue construction"});
        report["diagnostic_test"]=serde_json::json!(std::env::var_os("DEMSTUDIO_TEST_SOURCE").is_some());
        report["gpu_sample_batch_enabled"]=serde_json::json!(export.batch.is_some());
        report["gpu_sample_batch_syncs"]=serde_json::json!(export.batch_syncs);
        report["gpu_sample_batch_completed"]=serde_json::json!(export.batch_completed);
        report["gpu_sample_batch_device_reductions"]=serde_json::json!(export.batch_reductions);
        report["gpu_sample_batch_candidates"]=serde_json::json!(export.batch_samples);
        report["gpu_sample_batch_steps"]=serde_json::json!(export.batch_steps);
        report["gpu_execution_target_ms"]=serde_json::json!(std::env::var("DEMSTUDIO_GPU_BATCH_MS").ok().and_then(|v|v.parse::<f64>().ok()).filter(|v|v.is_finite()).unwrap_or(50.).clamp(5.,200.));
        report["gpu_sample_batch_stage_scope"]=serde_json::json!("sample_batch includes batched integration, device reduction, commit and cursor control; integrator_draw measures the unbatched path only");
        report["draw_activity"]=serde_json::json!({"mode":std::env::var("DEMSTUDIO_DRAW_ACTIVITY").unwrap_or_else(|_|"sample".into()),"sampling_probability":if export.phase_queue.is_some(){1.}else{1./64.},"sampling_method":if export.phase_queue.is_some(){"all four indirect phase commands per classification snapshot"}else{"first batch then deterministic hash of Draw ordinal"},"sampling_scope":if export.phase_queue.is_some(){"phase POINTS Draws; initialization triangles and unbatched first sample excluded"}else{"batch draws; unbatched first sample excluded"},"scope":if export.phase_queue.is_some(){"queued pixels / dense-equivalent capacity (4*tile_pixels per snapshot); mutually exclusive queues imply ratio <=25%; not hardware occupancy"}else{"GPU eligibility before sampled Draws; sample mode does not change draw topology; on measures every Draw and enables indirect resumed draws"},"measured_draws":export.activity_draws,"active_pixels":export.activity_active,"pixel_capacity":export.activity_capacity,"zero_active_draws":export.activity_zero,"ratio":if export.activity_capacity>0{Some(export.activity_active as f64/export.activity_capacity as f64)}else{None}});
        let activity_mode=std::env::var("DEMSTUDIO_DRAW_ACTIVITY").unwrap_or_else(|_|"sample".into());
        report["draw_activity"]["sampling_probability"]=serde_json::json!(if activity_mode=="off"{0.}else if activity_mode=="on"||export.phase_queue.is_some(){1.}else{1./64.});
        if export.async_samples.is_some(){
            report["draw_activity"]["sampling_method"]=serde_json::json!("first async Draw of each tile, then deterministic hash of Draw ordinal; on measures all, off measures none");
            report["draw_activity"]["sampling_scope"]=serde_json::json!("async integration/commit pairs; unbatched first sample excluded");
            report["draw_activity"]["scope"]=serde_json::json!("pre-Draw eligible pixels: unfinished cursor and uninitialized, ready, or pending commit; missing-page waits excluded. Diagnostic mode does not change topology; AsyncActiveQueue explicitly selects indirect POINTS, otherwise dense triangles; not hardware occupancy");
        }
        report["visibility_probe"]=export.visibility_probe.as_ref().map(|p|{
            let mut value=p.final_read(&self.gl);value["gpu_memory_bytes"]=serde_json::json!(p.memory_bytes());value["shader_clock_includes_probe_cost"]=serde_json::json!(crate::renderer::walk_profile::clock_enabled());value
        }).unwrap_or_else(||serde_json::json!({"enabled":false}));
        report["walk_profile"]=export.walk_profile.as_ref().map(|p|p.final_read(&self.gl)).unwrap_or_else(||serde_json::json!({"enabled":false}));
        report["bicubic_profile"]=export.bicubic_profile.as_ref().map(|p|p.final_read(&self.gl)).unwrap_or_else(||serde_json::json!({"enabled":false}));
        report["bicubic_root_precheck"]=serde_json::json!(std::env::var("DEMSTUDIO_BICUBIC_ROOT_PRECHECK").as_deref()==Ok("on"));
        report["bicubic_stack_variant"]=serde_json::json!(std::env::var("DEMSTUDIO_BICUBIC_STACK").unwrap_or_else(|_|"original".into()));
        report["bicubic_root_coefficient_rebuild"]=serde_json::json!({"enabled":std::env::var("DEMSTUDIO_BICUBIC_ROOT_REBUILD").as_deref()==Ok("on"),"scope":"isolator only; original monotone fast path and explicit ShadowAnyRoot candidate unchanged","dynamic_key_bytes":100,"removed_dynamic_coefficient_bytes":700,"production_split_counters":false,"rebuild_cost_bound":"extra split count E=sum(depth of popped right keys), <=24*R; no speed claim"});
        report["bicubic_root_dyadic_keys"]=serde_json::json!(std::env::var("DEMSTUDIO_BICUBIC_ROOT_KEYS").as_deref()==Ok("on")&&std::env::var("DEMSTUDIO_BICUBIC_STACK").as_deref()!=Ok("left"));
        
        
        report["state_layers"]=serde_json::json!(export.state_layers);
        report["primary_cache_enabled"]=serde_json::json!(true);
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default().as_millis();
        let name = format!("DEMStudio-export-performance-{}-{stamp}.json", std::process::id());
        if let Ok(data) = serde_json::to_vec_pretty(&report) {
            let _ = std::fs::write(std::env::temp_dir().join(name), data);
        }
        if let Some(queue) = export.active_queue.take() { queue.destroy(&self.gl); }
        if let Some(profile)=export.bicubic_profile.take(){profile.destroy(&self.gl);}
        if let Some(profile)=export.walk_profile.take(){profile.destroy(&self.gl);}
        if let Some(probe)=export.visibility_probe.take(){probe.destroy(&self.gl);}
        if let Some(wavefront)=export.wavefront.take(){wavefront.destroy(&self.gl);}
        if let Some(batch)=export.batch.take(){batch.destroy(&self.gl);}
        if let Some(compact) = export.compact.take() { compact.destroy(&self.gl); }
        
        
        
        
        if let Some(m)=export.directional_skip.take(){m.destroy(&self.gl);}
        if let Some(m)=export.ancestor_seed.take(){m.destroy(&self.gl);}
        if let Some(m)=export.geo_merged.take(){m.destroy(&self.gl);}
        if let Some(t)=export.row_cache{self.gl.delete_texture(t);}
        if let Some(p)=export.row_program{self.gl.delete_program(p);}
        if let Some(t)=export.leaf_bounds{self.gl.delete_texture(t);}
        if let Some(p)=export.leaf_program{self.gl.delete_program(p);}
        if let Some(t)=export.tight_low{self.gl.delete_texture(t);}
        if let Some(t)=export.tight_coarse{self.gl.delete_texture(t);}
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
        if let Some(q)=export.phase_queue.take(){q.destroy(&self.gl);}
        if let Some(samples)=export.async_samples.take(){samples.destroy(&self.gl);}
        if let Some(queue)=export.async_active_queue.take(){queue.destroy(&self.gl);}
        if let Some(pair)=export.async_fresh_pair.take(){for p in pair{self.gl.delete_program(p);}}
        if let Some(pair)=export.async_phase_pair.take(){for p in pair{self.gl.delete_program(p);}}
        if let Some(path)=export.async_path.take(){self.gl.delete_program(path);}
        if let Some(paths)=export.phase_paths.take(){for p in paths{self.gl.delete_program(p);}}
        self.gl.delete_program(export.path);
        if let Some(p)=export.interpolate {self.gl.delete_program(p);}
        self.gl.delete_program(export.commit);
        self.gl.delete_framebuffer(export.fbo);
    }

    pub unsafe fn render_source_sample(
        &mut self,
        export: &mut PagedExport,
        cancel: &AtomicBool,
    ) -> Result<bool, String> {
        if self.count==0 {
            export.batch_steps=1;
            
            
            
        }
        self.source_pass(export, cancel, false)?;
        
        export.batch_resume=false;
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

    unsafe fn prepare_source_pass(&mut self,e:&mut PagedExport,surface:bool)->Result<(),String>{
        if let Some(profile)=e.walk_profile.as_ref(){profile.bind(&self.gl);}

        let size=[self.params.width,self.params.height];
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
        let traversal_budget=256;
        
        self.i(e.path, "pg_iteration_budget", traversal_budget);
        self.i(e.path, "pg_precomputed_bounds", i32::from(e.bicubic_bounds));
        self.i(e.path, "pg_primary_cached", i32::from(self.geo_tile.is_some() && self.count > 0));
        e.bind_array(&self.gl, e.path, "pg_low_height", 4, e.low);
        self.bind_tex(e.path, "pg_page_table", 9, e.page_table);
        self.bind_tex(e.path, "pg_coarse_height", 5, e.coarse);
        self.i(e.path,"pg_tight_bounds",i32::from(e.tight_low.is_some()));
        if let Some(t)=e.row_cache{
            e.bind_array(&self.gl,e.path,"pg_bicubic_rows",11,t);
            if e.row_transform.is_none(){
                let location=self.gl.get_uniform_location(e.path,"mipmap_size").ok_or("Bicubic行缓存缺少原mipmap_size")?;
                let mut value=[0f32;4];self.gl.get_uniform_f32(e.path,&location,&mut value);
                let step=if self.params.height_step>0.{let location=self.gl.get_uniform_location(e.path,"mipmap_step").ok_or("Bicubic行缓存缺少原mipmap_step")?;let mut step=[0f32;1];self.gl.get_uniform_f32(e.path,&location,&mut step);step[0]}else{1.};
                if !value[2].is_finite()||!value[3].is_finite()||!step.is_finite()||(self.params.height_step>0.&&step<=0.){return Err("Bicubic行缓存高度变换无效".into());}
                e.row_transform=Some([value[2],value[3],step]);
            }
        }
        
        if let Some(m)=e.directional_skip.as_ref(){m.bind(&self.gl,e.path,true);}
        if let Some(m)=e.ancestor_seed.as_ref(){m.bind(&self.gl,e.path,if e.leaf_bounds.is_some(){12}else{10});}
        if let Some(m)=e.geo_merged.as_ref(){m.bind(&self.gl,e.path);}
        if let Some(t)=e.leaf_bounds {e.bind_array(&self.gl,e.path,"pg_leaf_bounds",10,t);}
        if let Some(t)=e.tight_low {e.bind_array(&self.gl,e.path,"pg_tight_low",10,t);}
        if let Some(t)=e.tight_coarse {self.bind_tex(e.path,"pg_tight_coarse",11,t);}
        e.bind_array(&self.gl, e.path, "pg_memo_a", 7, e.memo[0]);
        e.bind_array(&self.gl, e.path, "pg_memo_b", 8, e.memo[1]);
        if let Some(probe)=e.visibility_probe.as_ref().filter(|_|self.gl.get_uniform_location(e.path,"vp_bounds").is_some()){
            // Read the actual original shader uniforms after prepare_sample;
            // rebuilding these expressions on CPU could change f32 operation order.
            let mut mipmap=[0f32;4];let mut lower=[0f32;3];let mut upper=[0f32;3];
            for(name,values)in [("mipmap_size",mipmap.as_mut_slice()),("vol_bound[0]",lower.as_mut_slice()),("vol_bound[1]",upper.as_mut_slice())]{
                let location=self.gl.get_uniform_location(e.path,name).ok_or_else(||format!("可见性诊断缺少原uniform {name}"))?;
                self.gl.get_uniform_f32(e.path,&location,values);
            }
            let quant_step=if self.params.height_step>0.{
                let mut step=[0f32;1];let location=self.gl.get_uniform_location(e.path,"mipmap_step").ok_or("可见性诊断缺少原mipmap_step")?;
                self.gl.get_uniform_f32(e.path,&location,&mut step);step[0]
            }else{0.};
            let padded=e.source.logical_size[0].max(e.source.logical_size[1]).checked_add(e.source.geo_halo*2)
                .and_then(u32::checked_next_power_of_two).ok_or("可见性诊断逻辑尺寸溢出")?.max(32);
            let params=crate::renderer::visibility_probe::ProbeParams{
                padded,size:e.source.logical_size,geo_offset:e.source.geo_halo,
                height_scale:mipmap[2],height_offset:mipmap[3],quant_step,
                volume_min:lower,volume_max:upper,origin:[0.;3],direction:[0.;3],interval:[0.;2],node_budget:512,
            };
            probe.bind(&self.gl,e.path,&params);
        }
        self.gl.disable(glow::BLEND);
        self.gl.disable(glow::SCISSOR_TEST);
        if let Some(p)=e.interpolate {
            // Preserve the rasterizer's actual varying interpolation. Rebuilding
            // st algebraically in compute differs by one ULP on this driver.
            let (full,_)=output_tile_mapping(self.output_tile,size);
            let shortest=full[0].min(full[1]);
            let mut jitter=[0f32;2];
            if let Some(location)=self.gl.get_uniform_location(e.path,"pixel_jitter") {
                self.gl.get_uniform_f32(e.path,&location,&mut jitter);
            }
            self.gl.use_program(Some(p));
            self.v2(p,"aspect",[full[0]/shortest,full[1]/shortest]);
            self.v2(p,"pixel_jitter",jitter);
            self.gl.bind_framebuffer(glow::FRAMEBUFFER,Some(e.fbo));
            self.gl.framebuffer_texture_layer(glow::FRAMEBUFFER,glow::COLOR_ATTACHMENT0,Some(e.state[0]),0,18);
            self.gl.draw_buffers(&[glow::COLOR_ATTACHMENT0]);
            for attachment in 1..7 {self.gl.framebuffer_texture_layer(glow::FRAMEBUFFER,glow::COLOR_ATTACHMENT0+attachment,None,0,0);}
            if self.gl.check_framebuffer_status(glow::FRAMEBUFFER)!=glow::FRAMEBUFFER_COMPLETE {return Err("源分页插值缓冲创建失败".into());}
            let timer=e.timers.begin(&self.gl,GpuStage::Integrate);
            self.gl.draw_arrays(glow::TRIANGLES,0,3);
            self.gl.memory_barrier(glow::FRAMEBUFFER_BARRIER_BIT|glow::SHADER_IMAGE_ACCESS_BARRIER_BIT);
            e.timers.end(&self.gl,GpuStage::Integrate,timer);
            self.gl.use_program(Some(e.path));
        }
        
        Ok(())
    }

    unsafe fn draw_source_image(&self,e:&PagedExport)->Result<(),String>{
        
        self.gl.draw_arrays(glow::TRIANGLES,0,3);Ok(())
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
        if e.size != size {
            e.allocate(&self.gl, size)?;
            self.check("分配源分页光线状态")?;
        }
        self.prepare_source_pass(e,surface)?;
        let mut state = 0usize;
        let mut sparse_queue=false;
        for query in 0..if e.raystate {1} else {e.layers} {
            let mut first = true;
            loop {
                cancelled(cancel)?;
                self.gl.use_program(Some(e.path));
                e.bind_array(&self.gl, e.path, "pg_state_tex", 6, e.state[state]);
                self.i(e.path, "pg_query_index", query as i32);
                self.i(e.path, "pg_resume", i32::from(query != 0 || !first));
                self.i(e.path, "pg_reset_query", i32::from(first && query != 0));
                if e.image_integrator {
                    let submitted=std::time::Instant::now();
                    self.gl.bind_framebuffer(glow::FRAMEBUFFER,Some(self.accum_fbo));
                    self.gl.draw_buffers(&[glow::NONE]);
                    self.gl.bind_image_texture(0,Some(e.state[state]),0,true,0,glow::READ_WRITE,glow::RGBA32F);
                    let timer=e.timers.begin(&self.gl,GpuStage::Integrate);
                    for step in 0..e.execution_steps {
                        cancelled(cancel)?;
                        let resume=!first||step>0;
                        if let Some(queue)=e.active_queue.as_ref().filter(|_|sparse_queue||!resume){
                            queue.dispatch(&self.gl,e.state[state],e.page_table,e.size,e.source.logical_size[0],resume,false,false)?;
                        }
                        self.gl.use_program(Some(e.path));
                        self.i(e.path,"pg_resume",i32::from(!first||step>0));
                        self.i(e.path,"rs_use_point_queue",i32::from(resume&&sparse_queue&&e.active_queue.is_some()));
                        if let Some(queue)=e.active_queue.as_ref().filter(|_|resume&&sparse_queue){queue.draw(&self.gl,e.path,e.size)?;}
                        else{self.draw_source_image(e)?;}
                        
                        #[cfg(not(test))] self.gl.memory_barrier(glow::SHADER_IMAGE_ACCESS_BARRIER_BIT|glow::TEXTURE_FETCH_BARRIER_BIT);
                    }
                    e.timers.end(&self.gl,GpuStage::Integrate,timer);
                    e.stats.draw_submission_seconds+=submitted.elapsed().as_secs_f64();
                    e.stats.draws+=u64::from(e.execution_steps);
                    self.gl.bind_framebuffer(glow::FRAMEBUFFER,Some(e.fbo));
                }else if e.compute {
                    let submitted=std::time::Instant::now();
                    let timer=e.timers.begin(&self.gl,GpuStage::Integrate);
                    for step in 0..e.execution_steps {
                        cancelled(cancel)?;
                    if let Some(queue)=e.active_queue.as_ref() {
                        queue.dispatch(&self.gl,e.state[state],e.page_table,e.size,e.source.logical_size[0],!first || step>0,false,false)?;
                    }
                    self.gl.use_program(Some(e.path));
                    self.i(e.path,"pg_resume",i32::from(!first || step>0));
                    self.i(e.path,"rs_use_active_queue",i32::from(e.active_queue.is_some()));
                    self.gl.uniform_2_i32(self.gl.get_uniform_location(e.path,"rs_extent").as_ref(),e.size[0] as i32,e.size[1] as i32);
                    self.gl.bind_image_texture(0,Some(e.state[state]),0,true,0,glow::READ_WRITE,glow::RGBA32F);
                    self.gl.dispatch_compute((e.size[0]*e.size[1]).div_ceil(crate::renderer::raystate_shader::compute_group_size()),1,1);
                    self.gl.memory_barrier(glow::SHADER_IMAGE_ACCESS_BARRIER_BIT|glow::TEXTURE_FETCH_BARRIER_BIT|glow::TEXTURE_UPDATE_BARRIER_BIT);
                    }
                    e.timers.end(&self.gl,GpuStage::Integrate,timer);
                    e.stats.draw_submission_seconds+=submitted.elapsed().as_secs_f64();e.stats.draws+=u64::from(e.execution_steps);
                } else {
                for bank in 0..e.state_layers.div_ceil(if e.wide_mrt {7} else {4}) as i32 {
                    e.attach(&self.gl, e.state[1 - state], bank)?;
                    self.i(e.path, "pg_bank", bank);
                    self.i(e.path, "rs_state_bank", bank);
                    let submitted = std::time::Instant::now();
                    let timer = e.timers.begin(&self.gl, GpuStage::Integrate);
                    self.gl.draw_arrays(glow::TRIANGLES, 0, 3);
                    e.timers.end(&self.gl, GpuStage::Integrate, timer);
                    e.stats.draw_submission_seconds += submitted.elapsed().as_secs_f64();
                    e.stats.draws += 1;
                }
                state = 1 - state;
                }
                first = false;
                let feedback_started = std::time::Instant::now();
                let timer = e.timers.begin(&self.gl, GpuStage::Feedback);
                let inspection = std::time::Instant::now();
                let decision = if let Some(compact) = e.compact.as_mut() {
                    compact.dispatch(&self.gl, e.state[state], e.size)?;
                    let feedback = compact.read(&self.gl)?;
                    e.stats.feedback_bytes += compact.readback_bytes() as u64;
                    if feedback.invalid != 0 { return Err("源分页状态包含无效数值或页面".into()); }
                    DispatchDecision { all_paths_done: feedback.all_paths_done(), query_complete: feedback.query_complete(),done_pixels:feedback.done,
                        pages: feedback.requests.iter().map(|&(page,_)|page).collect(), requests: feedback.requests }
                } else {
                    let status = e.status(&self.gl, e.state[state]);
                    e.stats.feedback_bytes += (status.len()*4) as u64;
                    inspect_status(&status)?
                };
                e.timers.end(&self.gl, GpuStage::Feedback, timer);
                e.stats.feedback_reads += 1;
                e.stats.feedback_seconds += feedback_started.elapsed().as_secs_f64();
                e.stats.status_inspection_seconds += inspection.elapsed().as_secs_f64();
                let waited=feedback_started.elapsed().as_secs_f64();
                // Dense active sets cost less as one raster pass. Compact only
                // the sparse tail; completed counts are already in feedback.
                sparse_queue=u64::from(decision.done_pixels)*2>=u64::from(size[0])*u64::from(size[1])
                    ||std::env::var("DEMSTUDIO_ACTIVE_QUEUE").as_deref()==Ok("on");
                e.max_batch_wait_seconds=e.max_batch_wait_seconds.max(waited);
                if (e.compute||e.image_integrator) && std::env::var_os("DEMSTUDIO_COMPUTE_STEPS").is_none() {
                    let target=std::env::var("DEMSTUDIO_GPU_BATCH_MS").ok().and_then(|s|s.parse::<f64>().ok()).filter(|x|x.is_finite()).unwrap_or(50.).clamp(5.,200.)/1000.;
                    let previous=e.execution_steps;
                    if waited>target {e.execution_steps=(e.execution_steps/2).max(1);}
                    else if waited<target/4. && decision.pages.is_empty() {e.execution_steps=(e.execution_steps+1).min(8);}
                    e.execution_adjustments+=u64::from(previous!=e.execution_steps);
                }
                self.gl.use_program(Some(e.path));
                // Cache the first deterministic world query even if the whole
                // path escapes immediately. Without this, all-miss tiles would
                // reuse uninitialized results on sample two.
                if !e.raystate && query == 0 && decision.query_complete {
                    e.memoize(&self.gl, e.state[state], query);
                }
                if decision.all_paths_done {
                    if e.raystate && !surface && self.geo_tile.is_some() && self.count==0 {
                        e.memoize_primary(&self.gl,e.state[state]);
                    }
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
                    let timer = e.timers.begin(&self.gl, GpuStage::Commit);
                    self.gl.draw_arrays(glow::TRIANGLES, 0, 3);
                    e.timers.end(&self.gl, GpuStage::Commit, timer);
                    self.gl.disable(glow::BLEND);
                    e.checkpoint(&self.gl,"sample_complete",false);
                    return self.check("源分页积分");
                }
                if !e.raystate && decision.query_complete {
                    if query != 0 { e.memoize(&self.gl, e.state[state], query); }
                    break;
                }
                if !decision.pages.is_empty() {
                    e.stats.page_fault_rounds += 1;
                    e.stats.page_fault_requests += decision.pages.len() as u64;
                    let resident:Vec<_>=e.slots.iter().filter_map(|&page|page).collect();
                    let ranked=e.scheduler.rank(&decision.requests,&resident);
                    e.load_pages(&self.gl, &ranked)?;
                    if let Some(prefetch)=e.prefetch.as_mut() {
                        let across=e.source.logical_size[0].div_ceil(512);
                        for &page in ranked.iter().skip(PAGE_BATCH).take(16) {
                            if !e.bricks.iter().any(|(id,_)|*id==page) {
                                prefetch.try_request([page as u32%across,page as u32/across]);
                            }
                        }
                    }
                    e.bind_array(&self.gl, e.path, "pg_low_height", 4, e.low);
                    self.bind_tex(e.path, "pg_page_table", 9, e.page_table);
                }
                e.checkpoint(&self.gl,"integrating",false);
                self.check("继续源分页光线")?;
            }
        }
        Err("源分页光线查询超过已配置反弹上限".into())
    }


}

impl PagedExport {
    unsafe fn checkpoint(&mut self, gl:&glow::Context, stage:&str, force:bool) {
        if !force && self.last_checkpoint.elapsed().as_secs_f64()<2. {return;}
        self.last_checkpoint=std::time::Instant::now(); self.timers.collect(gl,false);
        let report=serde_json::json!({"build_identity":"D055-profile-20261110","stage":stage,"diagnostic_test":std::env::var_os("DEMSTUDIO_TEST_SOURCE").is_some(),
            "source":self.source_path,"parameters":self.requested_params,"elapsed_seconds":self.stats.started.elapsed().as_secs_f64(),
            "integrator":if self.raystate {"raystate"}else{"query_replay"},"slots":self.slots.len(),"steps":self.execution_steps,"max_feedback_wait_seconds":self.max_batch_wait_seconds,
            "max_batch_cpu_wall_seconds":self.max_batch_wall_seconds,
            "draw_activity":{"measured_draws":self.activity_draws,"active_pixels":self.activity_active,"pixel_capacity":self.activity_capacity,"zero_active_draws":self.activity_zero},
            "boundary_activity":{"done":self.boundary_done,"waiting":self.boundary_waiting,"active":self.boundary_active,"capacity":self.boundary_capacity},
            "sample_batch_enabled":self.batch.is_some(),"sample_batch_candidates":self.batch_samples,"sample_batch_steps":self.batch_steps,
            "sample_batch_syncs":self.batch_syncs,"sample_batch_completed":self.batch_completed,"sample_batch_device_reductions":self.batch_reductions,
            "feedback_reads":self.stats.feedback_reads,"feedback_bytes":self.stats.feedback_bytes,
            "page_uploads":self.stats.page_uploads,"page_fault_rounds":self.stats.page_fault_rounds,
            "unique_page_uploads":self.stats.unique_page_uploads,"repeated_page_uploads":self.stats.repeated_page_uploads,
            "source_reads":self.stats.page_reads,"cpu_cache_hits":self.stats.brick_cache_hits,
            "completed_pixel_samples":self.stats.pixel_samples,"gpu_timing_partial":self.timers.report()});
        use std::io::Write;
        if let Ok(mut file)=std::fs::OpenOptions::new().create(true).append(true).open(&self.progress_log){
            if let Ok(data)=serde_json::to_vec(&report){let _=file.write_all(&data);let _=file.write_all(b"\n");let _=file.sync_data();}
        }
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
            (self.state[0], self.state_layers),
            (self.state[1], self.state_layers),
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
    unsafe fn memoize_primary(&mut self, gl: &glow::Context, state: glow::Texture) {
        if self.compute||self.image_integrator {gl.memory_barrier(glow::FRAMEBUFFER_BARRIER_BIT);}
        let timer = self.timers.begin(gl, GpuStage::Memo);
        gl.bind_framebuffer(glow::FRAMEBUFFER,Some(self.fbo));
        for (layer,tex) in [(17,self.memo[0]),(18,self.memo[1])] {
            gl.framebuffer_texture_layer(glow::FRAMEBUFFER,glow::COLOR_ATTACHMENT0,Some(state),0,layer);
            gl.read_buffer(glow::COLOR_ATTACHMENT0);
            gl.bind_texture(glow::TEXTURE_2D_ARRAY,Some(tex));
            gl.copy_tex_sub_image_3d(glow::TEXTURE_2D_ARRAY,0,0,0,0,0,0,self.size[0] as i32,self.size[1] as i32);
        }
        self.timers.end(gl,GpuStage::Memo,timer);
    }
    unsafe fn memoize(&mut self, gl: &glow::Context, state: glow::Texture, query: u32) {
        let timer = self.timers.begin(gl, GpuStage::Memo);
        for (layer, tex) in [(3, self.memo[0]), (4, self.memo[1])] {
            gl.framebuffer_texture_layer(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, Some(state), 0, layer);
            gl.read_buffer(glow::COLOR_ATTACHMENT0);
            gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(tex));
            gl.copy_tex_sub_image_3d(glow::TEXTURE_2D_ARRAY, 0, 0, 0, query as i32,
                0, 0, self.size[0] as i32, self.size[1] as i32);
        }
        self.timers.end(gl, GpuStage::Memo, timer);
    }
    unsafe fn attach(
        &self,
        gl: &glow::Context,
        tex: glow::Texture,
        bank: i32,
    ) -> Result<(), String> {
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbo));
        let targets = if self.wide_mrt { 7 } else { 4 };
        let mut buffers = Vec::with_capacity(targets);
        for k in 0..targets {
            let layer = bank * targets as i32 + k as i32;
            let valid = layer < self.state_layers as i32;
            gl.framebuffer_texture_layer(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0 + k as u32,
                if valid { Some(tex) } else { None }, 0, layer.min(self.state_layers as i32 - 1));
            buffers.push(if valid {glow::COLOR_ATTACHMENT0 + k as u32} else {glow::NONE});
        }
        gl.draw_buffers(&buffers);
        if gl.check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE {
            return Err("源分页状态缓冲创建失败".into());
        }
        Ok(())
    }
    unsafe fn status(&self, gl: &glow::Context, tex: glow::Texture) -> Vec<f32> {
        if self.compute||self.image_integrator {gl.memory_barrier(glow::FRAMEBUFFER_BARRIER_BIT);}
        gl.bind_framebuffer(glow::FRAMEBUFFER,Some(self.fbo));
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
        if let Some(prefetch)=self.prefetch.as_mut() {
            let across=self.source.logical_size[0].div_ceil(512);
            for ([x,y],result) in prefetch.drain() {
                let mut brick=result?;
                self.bound_preparation_cpu_seconds+=prepare_brick_bounds(&mut brick,self.bicubic_bounds,self.source.logical_size,self.source.geo_halo);
                let brick=prepare_cached_brick(brick,self.tight_low.is_some(),self.source.logical_size,self.source.geo_halo,&mut self.bound_preparation_cpu_seconds)?;
                self.stats.prefetch_reads+=1;
                retain_brick(&mut self.bricks,(y*across+x) as i32,brick,CPU_BRICK_BUDGET);
            }
        }
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
                let mut brick = self.source.read_brick(page as u32 % across, page as u32 / across)?;
                self.stats.page_reads += 1;
                self.stats.page_read_seconds += read_started.elapsed().as_secs_f64();
                self.bound_preparation_cpu_seconds+=prepare_brick_bounds(&mut brick,self.bicubic_bounds,self.source.logical_size,self.source.geo_halo);
                prepare_cached_brick(brick,self.tight_low.is_some(),self.source.logical_size,self.source.geo_halo,&mut self.bound_preparation_cpu_seconds)?
            };
            let upload_started = std::time::Instant::now();
            let upload_unit=gl.get_parameter_i32(glow::ACTIVE_TEXTURE) as u32;
            gl.active_texture(glow::TEXTURE0+4);
            gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(self.low));
            for level in &brick.raw.levels {
                let (edge,values)=(level.edge,level.values.as_slice());
                let timer = self.timers.begin(gl, GpuStage::Upload);
                gl.tex_sub_image_3d(glow::TEXTURE_2D_ARRAY, level.level as i32,
                    0, 0, slot as i32, edge as i32, edge as i32, 1,
                    glow::RED, glow::FLOAT,
                    glow::PixelUnpackData::Slice(Some(bytes(values))));
                self.timers.end(gl, GpuStage::Upload, timer);
            }
            if let Some(t)=self.tight_low {
                gl.bind_texture(glow::TEXTURE_2D_ARRAY,Some(t));
                for level in &brick.tight {
                    let timer=self.timers.begin(gl,GpuStage::Upload);
                    gl.tex_sub_image_3d(glow::TEXTURE_2D_ARRAY,(level.level-1)as i32,0,0,slot as i32,level.edge as i32,level.edge as i32,1,glow::RED,glow::FLOAT,glow::PixelUnpackData::Slice(Some(bytes(&level.values))));
                    self.timers.end(gl,GpuStage::Upload,timer);
                }
                // Upload changes the currently active unit's array binding.
                // Restore low-height at that unit; callers rebind their path samplers.
                gl.bind_texture(glow::TEXTURE_2D_ARRAY,Some(self.low));
            }
            if let (Some(t),Some(program))=(self.leaf_bounds,self.leaf_program){
                let timer=self.timers.begin(gl,GpuStage::LeafBounds);
                let built=crate::renderer::bicubic_leaf::build(gl,program,self.low,t,slot,brick.raw.origin,self.source.logical_size,self.source.geo_halo);
                self.timers.end(gl,GpuStage::LeafBounds,timer);
                gl.active_texture(upload_unit);built?;
            }
            if let (Some(output),Some(program))=(self.row_cache,self.row_program){
                let transform=self.row_transform.ok_or("Bicubic行缓存须先准备高度变换")?;
                let across=self.source.logical_size[0].div_ceil(512);let core=[(page as u32%across)*512,(page as u32/across)*512];
                let started=std::time::Instant::now();let timer=self.timers.begin(gl,GpuStage::BicubicRows);
                let built=crate::renderer::bicubic_rows::build(gl,program,self.low,output,slot,[core[0]as i32,core[1]as i32],self.source.logical_size,self.source.padded_size,transform);
                self.timers.end(gl,GpuStage::BicubicRows,timer);self.row_preparation_cpu_seconds+=started.elapsed().as_secs_f64();
                gl.active_texture(upload_unit);built?;
            }
            if let Some(merged)=self.geo_merged.as_ref(){
                let started=std::time::Instant::now();let timer=self.timers.begin(gl,GpuStage::GeoMerged);
                // The merged lookup is indexed from pg_brick_core, not the
                // raw upload's halo origin. Match pg_walk's page binding.
                let across=self.source.logical_size[0].div_ceil(512);
                let core=[(page as u32%across)*512,(page as u32/across)*512];
                let built=merged.prepare_low(gl,self.low,self.coarse,slot as u32,[core[0]as i32,core[1]as i32],self.source.logical_size,self.source.padded_size);
                self.timers.end(gl,GpuStage::GeoMerged,timer);self.geo_merged_preparation_cpu_seconds+=started.elapsed().as_secs_f64();
                gl.active_texture(upload_unit);built?;
            }
            if let Some(seed)=self.ancestor_seed.as_ref(){
                let across=self.source.logical_size[0].div_ceil(512);let core=[(page as u32%across)*512,(page as u32/across)*512];
                let started=std::time::Instant::now();let timer=self.timers.begin(gl,GpuStage::AncestorSeed);
                let built=seed.prepare_low(gl,self.low,self.coarse,slot as u32,[core[0]as i32,core[1]as i32],self.source.logical_size,self.source.padded_size);
                self.timers.end(gl,GpuStage::AncestorSeed,timer);self.ancestor_seed_preparation_cpu_seconds+=started.elapsed().as_secs_f64();
                gl.active_texture(upload_unit);built?;
            }
            gl.active_texture(upload_unit);
            if gl.get_error()!=glow::NO_ERROR{return Err("上传分页高程或紧界失败".into());}
            self.stats.page_uploads += 1;
            if self.uploaded_pages[page as usize] {self.stats.repeated_page_uploads+=1;}
            else {self.uploaded_pages[page as usize]=true;self.stats.unique_page_uploads+=1;}
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
        let timer = self.timers.begin(gl, GpuStage::Upload);
        let table_unit=gl.get_parameter_i32(glow::ACTIVE_TEXTURE) as u32;
        gl.active_texture(glow::TEXTURE0+9);
        upload_page_table(gl, self.page_table, across, down, &self.page_map, false);
        gl.active_texture(table_unit);
        self.timers.end(gl, GpuStage::Upload, timer);
        Ok(())
    }

}

// Cached bricks contain upload-ready bounds; raw source mip storage is untouched.
fn prepare_brick_bounds(brick: &mut Brick, enabled: bool, size: [u32;2], offset: u32) -> f64 {
    if !enabled { return 0.; }
    let started=std::time::Instant::now();
    for level in &mut brick.levels {
        if level.level>0 {
            level.values=crate::renderer::bicubic_bounds::level_values(level,brick.origin,size,offset);
        }
    }
    started.elapsed().as_secs_f64()
}

struct CachedBrick {raw:Brick,tight:Vec<MipLevel>}
fn prepare_cached_brick(raw:Brick,enabled:bool,size:[u32;2],offset:u32,seconds:&mut f64)->Result<CachedBrick,String>{
    let started=std::time::Instant::now();
    let tight=if enabled{crate::renderer::tight_bounds::build_low(&raw,size,offset)?}else{Vec::new()};
    if enabled{*seconds+=started.elapsed().as_secs_f64();}
    Ok(CachedBrick{raw,tight})
}
fn brick_bytes(brick: &CachedBrick) -> usize {
    brick.raw.levels.iter().chain(brick.tight.iter()).map(|level| level.values.len() * std::mem::size_of::<f32>()).sum()
}

fn brick_cache_bytes(bricks: &VecDeque<(i32, CachedBrick)>) -> usize {
    bricks.iter().map(|(_, brick)| brick_bytes(brick)).sum()
}

fn retain_brick(bricks: &mut VecDeque<(i32, CachedBrick)>, page: i32, brick: CachedBrick, budget: usize) {
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

unsafe fn allocate_tight_page_cache(gl:&glow::Context,tex:glow::Texture,slots:usize)->Result<(),String>{
    gl.bind_texture(glow::TEXTURE_2D_ARRAY,Some(tex));
    gl.tex_parameter_i32(glow::TEXTURE_2D_ARRAY,glow::TEXTURE_MIN_FILTER,glow::NEAREST_MIPMAP_NEAREST as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D_ARRAY,glow::TEXTURE_MAG_FILTER,glow::NEAREST as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D_ARRAY,glow::TEXTURE_MAX_LEVEL,3);
    for level in 0..4 {let edge=320>>level;gl.tex_image_3d(glow::TEXTURE_2D_ARRAY,level,glow::R32F as i32,edge,edge,slots as i32,0,glow::RED,glow::FLOAT,glow::PixelUnpackData::Slice(None));
        if gl.get_error()!=glow::NO_ERROR{return Err("紧界分页显存分配失败".into());}}
    Ok(())
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

unsafe fn upload_levels(gl: &glow::Context, tex: glow::Texture, levels: &[MipLevel], base: u32,bounds:Option<([u32;2],u32)>,timers:&mut GpuTimers)->f64 {
    let mut bound_seconds=0.;
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
        let bound_started=std::time::Instant::now();
        let bound_values=bounds.map(|(size,offset)|crate::renderer::bicubic_bounds::level_values(level,[0,0],size,offset));
        if bound_values.is_some(){bound_seconds+=bound_started.elapsed().as_secs_f64();}
        let timer=timers.begin(gl,GpuStage::Upload);
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            (level.level - base) as i32,
            glow::R32F as i32,
            level.edge as i32,
            level.edge as i32,
            0,
            glow::RED,
            glow::FLOAT,
            glow::PixelUnpackData::Slice(Some(bytes(bound_values.as_deref().unwrap_or(&level.values)))),
        );
        timers.end(gl,GpuStage::Upload,timer);
    }
    bound_seconds
}

struct DispatchDecision {
    all_paths_done: bool,
    query_complete: bool,
    done_pixels: u32,
    pages: Vec<i32>,
    requests: Vec<(i32,u32)>,
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
        done_pixels: done as u32,
        requests: pages.iter().map(|&(page,count)|(page,count as u32)).collect(),
        pages: pages.into_iter().map(|(page, _)| page).collect(),
    })
}


















