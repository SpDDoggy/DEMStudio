//! Windows child viewport. All GL state stays on one dedicated render thread.
use super::{
    appearance::ColorTexture, gl_renderer::{GlRenderer, PagedExport}, HeightMap, OverlayRect, RenderParams,
    RenderStatus, ViewportRect,
    thumbnail::{ExportFrame, Thumbnail},
};
use std::{
    ffi::{c_void, CString},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use tauri::Emitter;
use windows::{
    core::{w, PCSTR},
    Win32::{
        Foundation::*,
        Graphics::{Dwm::*, Gdi::*, OpenGL::*},
        System::LibraryLoader::*,
        UI::{
            Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, GetKeyState},
            Shell::{
                DefSubclassProc, GetWindowSubclass, RemoveWindowSubclass, SHAppBarMessage,
                SetWindowSubclass, ABE_BOTTOM, ABE_LEFT, ABE_RIGHT, ABE_TOP, ABM_GETAUTOHIDEBAREX,
                APPBARDATA,
            },
            WindowsAndMessaging::*,
        },
    },
};

enum Command {
    Repaint,
    RetainFrame(mpsc::SyncSender<Result<(Vec<u8>,u32,u32),String>>),
    EndResize(usize,usize),
    Rect(ViewportRect),
    Params(RenderParams),
    InteractiveParams(RenderParams, Instant),
    Map(HeightMap),
    MapReady(mpsc::SyncSender<RenderStatus>),
    Texture(ColorTexture, mpsc::SyncSender<Result<(), String>>),
    ClearTexture(mpsc::SyncSender<Result<(), String>>),
    TexturePreview(mpsc::SyncSender<Result<(Vec<u8>,u32,u32),String>>),
    SampleAppearance(f32,f32,mpsc::SyncSender<Result<super::AppearanceSample,String>>),
    AppearanceDistribution(mpsc::SyncSender<Result<super::AppearanceDistribution,String>>),
    Pause(bool),
    Capture(mpsc::SyncSender<Result<(Vec<u8>, u32, u32), String>>),
    CaptureWithMetadata(mpsc::SyncSender<Result<(Vec<u8>, u32, u32, serde_json::Value), String>>),
    ExportPreviewResult(mpsc::SyncSender<Result<ExportFrame, String>>),
    CaptureOutput(
        Option<RenderParams>,
        bool,
        mpsc::SyncSender<Result<(Vec<u8>, u32, u32), String>>,
    ),
    ExportPng(
        PathBuf,
        Option<RenderParams>,
        mpsc::SyncSender<Result<(), String>>,
    ),
    PreviewOutput(
        bool,
        Option<RenderParams>,
        mpsc::SyncSender<Result<(Vec<u8>, u32, u32), String>>,
    ),
    ExportGeo(
        PathBuf,
        Option<RenderParams>,
        mpsc::SyncSender<Result<(), String>>,
    ),
    ExportMesh(PathBuf, RenderParams, mpsc::SyncSender<Result<(),String>>),
    Shutdown,
}

fn capture_identity_is_stale(completed_content: Option<u64>, current_content: u64, actual: &RenderParams, desired: &RenderParams) -> bool {
    completed_content != Some(current_content) || (*actual != *desired && *actual != preview_params(desired))
}

fn export_params(desired: &RenderParams, params: Option<RenderParams>) -> RenderParams {
    params.unwrap_or_else(|| desired.clone())
}

fn export_preview_params(
    desired: &RenderParams,
    requested: Option<RenderParams>,
    map: Option<(u32, u32)>,
) -> RenderParams {
    let mut params = export_params(desired, requested);
    if let Some((width, height)) = map {
        params.width = width;
        params.height = height;
        params.projection = "orthographic".into();
    }
    let scale = (960. / params.width.max(params.height) as f64).min(1.);
    params.width = (params.width as f64 * scale).round().max(1.) as u32;
    params.height = (params.height as f64 * scale).round().max(1.) as u32;
    params.grid = false;
    params.samples = 4;
    params
}

/// Geometry resolution is independent of output pixels and georeferencing.
#[derive(Debug, PartialEq)]
enum ExportTerrainPlan { Preview, Paged(u32) }

// Diagnostic wall-clock intervals overlap GPU execution; never add them to GPU timers.
struct ExportDiagnostic {
    started: Instant,
    id: String,
    params: serde_json::Value,
    preparation: f64,
    callbacks: f64,
    readback: f64,
    thumbnail: f64,
    writer: f64,
    outcome: String,
}
impl ExportDiagnostic {
    fn new(params: &RenderParams) -> Self {
        let stamp=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        Self { started:Instant::now(), id:format!("{}-{stamp}",std::process::id()),
            params:serde_json::to_value(params).unwrap_or_default(), preparation:0.,callbacks:0.,
            readback:0.,thumbnail:0.,writer:0.,outcome:"preparation_failed".into() }
    }
    fn report(&self) -> serde_json::Value {
        serde_json::json!({"build_identity":"D055-profile-20261110","diagnostic_test":std::env::var_os("DEMSTUDIO_TEST_SOURCE").is_some(), "session_id":self.id,
            "parameters":self.params,"outcome":self.outcome,"total_wall_seconds":self.started.elapsed().as_secs_f64(),
            "preparation_wall_seconds":self.preparation,"render_callbacks_wall_seconds":self.callbacks,
            "output_readback_wall_seconds":self.readback,"thumbnail_wall_seconds":self.thumbnail,
            "writer_including_callbacks_wall_seconds":self.writer,
            "encoding_and_write_residual_wall_seconds":(self.writer-self.callbacks).max(0.),
            "timing_scope":"Overlapping CPU wall intervals; readback includes GPU wait and display conversion. Writer residual excludes callbacks but combines encoding and file IO; not pure disk time."})
    }
}
impl Drop for ExportDiagnostic {
    fn drop(&mut self) {
        if let Ok(bytes)=serde_json::to_vec_pretty(&self.report()) {
            let _=std::fs::write(std::env::temp_dir().join(format!("DEMStudio-native-performance-{}.json",self.id)),bytes);
        }
    }
}

/// Every source-backed resolution uses the same bounded page address space.
/// Cache capacity may cover all pages; that never selects another integrator.
fn export_terrain_plan(mode: &str) -> Result<ExportTerrainPlan,String> {
    match mode {
        "preview"=>Ok(ExportTerrainPlan::Preview),
        "source"=>Ok(ExportTerrainPlan::Paged(1)),
        "half"=>Ok(ExportTerrainPlan::Paged(2)),
        "quarter"=>Ok(ExportTerrainPlan::Paged(4)),
        _=>Err("无效的导出地形精度".into()),
    }
}

unsafe fn prepare_export_terrain(
    renderer:&mut GlRenderer, preview:&HeightMap, desired:&RenderParams,
    geo:bool, cancel:&AtomicBool,status:&Arc<Mutex<RenderStatus>>,
) -> Result<Option<PagedExport>,String> {
    if cancel.load(Ordering::Acquire){return Err("已取消导出".into());}
    if preview.source.is_none() {return Ok(None);}
    let plan=export_terrain_plan(&desired.export_terrain_scale)?;
    match plan {
        ExportTerrainPlan::Preview=>Ok(None),
        ExportTerrainPlan::Paged(factor)=>{
            let mut last=0;
            renderer.begin_source_export(preview,desired,factor,geo,cancel,|scanned,total|{
                let percent=(scanned.saturating_mul(5)/total.max(1)).min(5) as u32;
                if percent!=last {status.lock().unwrap_or_else(|e|e.into_inner()).export_progress=percent;last=percent;}
            }).map(Some)
        },
    }
}

fn export_microtiles(width:u32,height:u32,paged:bool,bounces:u32)->impl Iterator<Item=[u32;4]> {
    let (step_x,step_y)=if paged {if bounces<=15 {(512,128)}else{(256,64)}}else{(width.max(1),height.max(1))};
    (0..height).step_by(step_y as usize).flat_map(move |y|
        (0..width).step_by(step_x as usize).map(move |x|[x,y,step_x.min(width-x),step_y.min(height-y)]))
}

/// Encoders keep their bounded strips; paged rendering uses smaller GPU windows.
unsafe fn render_export_tile(
    renderer:&mut GlRenderer, paged:&mut Option<PagedExport>, desired:&RenderParams,
    full_size:[u32;2], tile:[u32;4], geo:bool,
    completed:&mut u64,total:u64,status:&Arc<Mutex<RenderStatus>>,cancel:&AtomicBool,
    diagnostic:&mut ExportDiagnostic, layer:&str,
) -> Result<Vec<u8>,String> {
    let [x,y,width,height]=tile;
    if *completed == 0 {
        status.lock().unwrap_or_else(|e|e.into_inner()).export_work_progress=Some(0.);
    }
    let pixel_bytes=if desired.output_bit_depth==16 {8usize}else{4};
    let row_bytes=(width as usize).checked_mul(pixel_bytes).ok_or("导出分块尺寸溢出")?;
    let mut result=vec![0;row_bytes.checked_mul(height as usize).ok_or("导出分块尺寸溢出")?];
    for [dx,dy,w,h] in export_microtiles(width,height,paged.is_some(),desired.max_bounces) {
        if cancel.load(Ordering::Acquire){return Err("已取消导出".into());}
        let mut params=desired.clone();params.width=w;params.height=h;params.grid=false;
        if params.export_terrain_scale!="preview" {params.lod=0;}
        renderer.set_params(&params)?;
        if geo {
            renderer.set_geo_tile(Some([x+dx,y+dy,w,h]))?;
            renderer.reset_accumulation();renderer.restart_sampling_sequence();
        } else {
            renderer.set_output_tile(Some((full_size,[x+dx,y+dy,w,h])))?;
        }
        while renderer.samples()<desired.samples.max(1) {
            if cancel.load(Ordering::Acquire){return Err("已取消导出".into());}
            let finished=match paged.as_mut() {
                Some(source)=>renderer.render_source_batch(source,cancel,desired.samples.max(1)-renderer.samples())?,
                None=>u32::from(renderer.render_export_sample_step()?),
            };
            if finished>0 {
                *completed+=u64::from(w)*u64::from(h)*u64::from(finished);
                let mut state=status.lock().unwrap_or_else(|e|e.into_inner());
                state.export_work_progress=Some((*completed as f64 * 100. / total.max(1) as f64).min(100.));
                state.export_progress=
                    (if paged.is_some() {5+completed.saturating_mul(95)/total.max(1)}
                    else {completed.saturating_mul(100)/total.max(1)}).min(99) as u32;
            }
        }
        if layer == "beauty" { if let Some(source)=paged.as_mut(){renderer.finish_source_surface(source,cancel)?;} }
        let readback_started=Instant::now();
        let pixels=if layer != "beauty" {renderer.read_export_layer_pixels(desired.output_bit_depth)} else {match paged.as_mut() {
            Some(source)=>renderer.profile_source_output(source),
            None=>renderer.export_pixels_for_output(),
        }};
        diagnostic.readback+=readback_started.elapsed().as_secs_f64();
        let pixels=pixels?;
        let part_row=w as usize*pixel_bytes;
        if pixels.len()!=part_row*h as usize{return Err("导出分块像素数量无效".into());}
        for row in 0..h as usize {
            let dst=(dy as usize+row)*row_bytes+dx as usize*pixel_bytes;
            result[dst..dst+part_row].copy_from_slice(&pixels[row*part_row..(row+1)*part_row]);
        }
    }
    Ok(result)
}

fn latest_interactive_params(
    mut params: RenderParams,
    mut changed: Instant,
    receiver: &mpsc::Receiver<Command>,
) -> (RenderParams, Instant, Option<Command>) {
    loop {
        match receiver.try_recv() {
            Ok(Command::InteractiveParams(newer, time)) => {
                params = newer;
                changed = time;
            }
            Ok(barrier) => return (params, changed, Some(barrier)),
            Err(_) => return (params, changed, None),
        }
    }
}

fn preview_params(desired: &RenderParams) -> RenderParams {
    let (w, h) = (desired.width as f64, desired.height as f64);
    // Bicubic keeps its actual geometry during interaction, at a smaller pixel
    // budget. Full output/lighting/sampling are restored by the idle deadline.
    let edge = if desired.surface == "bicubic" {
        480.
    } else {
        960.
    };
    let pixels = edge * edge * 9. / 16.;
    let scale = 1f64
        .min(edge / w)
        .min(edge / h)
        .min((pixels / (w * h)).sqrt());
    let mut effective = desired.clone();
    effective.width = (w * scale).floor().max(1.) as u32;
    effective.height = (h * scale).floor().max(1.) as u32;
    effective.samples = 1;
    effective
}

// Never discard the first interactive frame merely because its idle timer expired.
fn preview_can_refine(preview: bool, samples: u32, pending: bool) -> bool {
    !pending && (!preview || samples > 0)
}



#[derive(Debug, PartialEq)]
enum FrameWork {
    None,
    Present,
    Sample,
}

fn frame_work(
    visible: bool,
    paused: bool,
    unfinished: bool,
    dirty: bool,
    repaint: bool,
    completed: bool,
) -> FrameWork {
    if !visible {
        FrameWork::None
    } else if !completed {
        // A reset output is not a frame. Never swap an empty/partial surface,
        // even when a native drag/resize repaint arrives before its first sample.
        FrameWork::Sample
    } else if repaint {
        FrameWork::Present
    } else if !paused && unfinished {
        FrameWork::Sample
    } else if dirty {
        FrameWork::Present
    } else {
        FrameWork::None
    }
}

fn parameter_status(
    status: &Arc<Mutex<RenderStatus>>,
    desired: &RenderParams,
    preview: bool,
    renderer: &GlRenderer,
) {
    let mut s = status.lock().unwrap_or_else(|e| e.into_inner());
    s.appearance_range = renderer.appearance_range();
    s.width = desired.width;
    s.height = desired.height;
    s.target_samples = desired.samples.max(1);
    s.preview = preview;
    let dimensions = renderer.dimensions();
    s.preview_width = if preview { dimensions.0 } else { 0 };
    s.preview_height = if preview { dimensions.1 } else { 0 };
    s.samples = if preview { 0 } else { renderer.samples() };
}

/// Replace only the contiguous stale parameter burst. The first non-parameter
/// command is returned to the worker as lookahead, never crossed or discarded.
fn latest_params(
    mut params: RenderParams,
    receiver: &mpsc::Receiver<Command>,
) -> (RenderParams, Option<Command>) {
    loop {
        match receiver.try_recv() {
            Ok(Command::Params(newer)) => params = newer,
            Ok(barrier) => return (params, Some(barrier)),
            Err(_) => return (params, None),
        }
    }
}

fn commit_texture_removal(
    result: &Result<(), String>,
    desired: &mut RenderParams,
    status: &mut RenderStatus,
) {
    if result.is_ok() {
        desired.texture_enabled = false;
        if desired.appearance_layers_version == 0 || desired.appearance_mode == "texture" {
            desired.appearance_mode = "material".into();
        }
        status.texture_name = None;
        status.texture_width = 0;
        status.texture_height = 0;
        status.samples = 0;
    }
}
struct Shared {
    window: isize,
    sender: mpsc::Sender<Command>,
    status: Arc<Mutex<RenderStatus>>,
    worker: Mutex<Option<thread::JoinHandle<()>>>,
    export_cancel: Arc<AtomicBool>,
    export_busy: AtomicBool,
}
#[derive(Clone)]
pub struct RendererHandle(Arc<Shared>);

// IPC overlays use physical top-left client coordinates; GL presentation uses
// physical top-left viewport-local coordinates. Saturation prevents malformed
// or off-window signed coordinates from overflowing during subtraction.
fn local_overlays(rect: &ViewportRect) -> Vec<OverlayRect> {
    rect.overlays
        .iter()
        .map(|card| OverlayRect {
            x: card.x.saturating_sub(rect.x),
            y: card.y.saturating_sub(rect.y),
            width: card.width,
            height: card.height,
        })
        .collect()
}

impl RendererHandle {
    fn send(&self, cmd: Command) -> Result<(), String> {
        self.0.sender.send(cmd).map_err(|_| "渲染线程已停止".into())
    }
    /// Must be called on the Tauri UI thread. Coordinates are physical pixels.
    pub fn resize(&self, mut rect: ViewportRect) -> Result<(), String> {
        unsafe {
            let window = HWND(self.0.window as *mut c_void);
            let parent = GetParent(window).map_err(|e| e.to_string())?;
            ensure_frameless(parent, window)?;
            // The viewport is always the full native client. A delayed WebView
            // resize IPC must never shrink/hide a surface already sized by HWND.
            let mut client = RECT::default();
            GetClientRect(parent, &mut client).map_err(|e| e.to_string())?;
            rect.x = 0;
            rect.y = 0;
            rect.width = client.right.max(0) as u32;
            rect.height = client.bottom.max(0) as u32;
            rect.visible = rect.width > 0 && rect.height > 0;
            sync_window_shape(parent)?;
            // Keep the transparent WebView above the unclipped WGL surface.
            // Wry must also paint beneath Tauri's transparent resize hit region.
            // Sibling clipping would leave an unshaded strip around the client.
            configure_webview_sibling(window)?;
            let flags = SWP_NOACTIVATE
                | if rect.visible && rect.width > 0 && rect.height > 0 {
                    SWP_SHOWWINDOW
                } else {
                    SWP_HIDEWINDOW
                };
            let mut current = RECT::default();
            GetClientRect(window, &mut current).map_err(|e| e.to_string())?;
            let capacity = viewport_capacity(parent, [current.right, current.bottom], [client.right, client.bottom]);
            let showing = IsWindowVisible(window).as_bool();
            let at_bottom = GetWindow(window, GW_HWNDNEXT).is_err();
            if viewport_position_changed(
                [current.right, current.bottom],
                capacity,
                showing,
                rect.visible,
                at_bottom,
            ) {
                SetWindowPos(
                    window,
                    Some(HWND_BOTTOM),
                    rect.x,
                    rect.y,
                    capacity[0],
                    capacity[1],
                    flags | SWP_NOCOPYBITS,
                )
                .map_err(|e| format!("定位原生视口失败: {e}"))?;
                request_repaint(window);

            }
        }
        self.send(Command::Rect(rect))
    }
    /// UI-thread diagnostics of the actual native presentation hierarchy.
    pub fn presentation(&self) -> Result<serde_json::Value, String> {
        unsafe {
            let window = HWND(self.0.window as *mut c_void);
            let parent = GetParent(window).map_err(|e| e.to_string())?;
            let mut children = Vec::new();
            let mut child = GetWindow(parent, GW_CHILD).ok();
            while let Some(hwnd) = child {
                let mut rect = RECT::default();
                GetWindowRect(hwnd, &mut rect).map_err(|e| e.to_string())?;
                let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
                children.push(serde_json::json!({
                    "hwnd": hwnd.0 as isize, "class": window_class(hwnd)?,
                    "style": style, "clipSiblings": style & WS_CLIPSIBLINGS.0 != 0,
                    "visible": IsWindowVisible(hwnd).as_bool(),
                    "screenRect": [rect.left, rect.top, rect.right, rect.bottom],
                    "viewport": hwnd == window,
                }));
                child = GetWindow(hwnd, GW_HWNDNEXT).ok();
            }
            let parent_style = GetWindowLongPtrW(parent, GWL_STYLE) as u32;
            let mut subclass_data = 0;
            let frame_paint_suppressed = GetWindowSubclass(
                parent,
                Some(frameless_proc),
                FRAMELESS_SUBCLASS,
                Some(&mut subclass_data),
            )
            .as_bool();
            let region = CreateRectRgn(0, 0, 0, 0);
            let mut bounds = RECT::default();
            GetWindowRect(parent, &mut bounds).map_err(|e| e.to_string())?;
            let (width, height) = (bounds.right - bounds.left, bounds.bottom - bounds.top);
            let region_type = GetWindowRgn(parent, region);
            let region_rounded = region_type == COMPLEXREGION
                && !PtInRegion(region, 0, 0).as_bool()
                && !PtInRegion(region, width - 1, 0).as_bool()
                && !PtInRegion(region, 0, height - 1).as_bool()
                && !PtInRegion(region, width - 1, height - 1).as_bool()
                && PtInRegion(region, width / 2, height / 2).as_bool();
            let _ = DeleteObject(region.into());
            let corner_preference = dwm_corner_preference(parent);
            let explicit_region = region_type.0 != ERROR;
            let system_rounded = !explicit_region && corner_preference == Some(DWMWCP_ROUND);
            let rounded = region_rounded || system_rounded;
            let mut client = RECT::default();
            GetClientRect(window, &mut client).map_err(|e| e.to_string())?;
            let mut visible_client = RECT::default();
            GetClientRect(parent, &mut visible_client).map_err(|e| e.to_string())?;
            let mut monitor = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            GetMonitorInfoW(
                MonitorFromWindow(parent, MONITOR_DEFAULTTONEAREST),
                &mut monitor,
            )
            .ok()
            .map_err(|e| e.to_string())?;
            Ok(
                serde_json::json!({"parent":parent.0 as isize,"viewport":window.0 as isize,
                    "parentStyle":parent_style,"frameless":frame_paint_suppressed && parent_style & WS_CAPTION.0 == 0,
                    "nativeCaptionAbsent":parent_style & WS_CAPTION.0 == 0,
                    "nonClientPaintSuppressed":frame_paint_suppressed,
                    "roundedCorners":rounded,
                    "cornerRendering":if system_rounded {"dwm"} else if region_rounded {"region"} else {"square"},
                    "explicitWindowRegion":explicit_region,
                    "systemCornerPreference":corner_preference.map(|v|v.0),
                    "cornerVisualVerification":false,
                    "maximized":IsZoomed(parent).as_bool(),
                    "outerRect":[bounds.left,bounds.top,bounds.right,bounds.bottom],
                    "viewportClientSize":[client.right,client.bottom],
                    "stableDrawable":true,
                    "visibleClientSize":[visible_client.right,visible_client.bottom],
                    "monitorWorkArea":[monitor.rcWork.left,monitor.rcWork.top,monitor.rcWork.right,monitor.rcWork.bottom],
                    "childrenTopToBottom":children}),
            )
        }
    }
    pub fn set_params(&self, params: RenderParams) -> Result<(), String> {
        self.send(Command::Params(params))
    }
    pub fn set_interactive_params(&self, params: RenderParams) -> Result<(), String> {
        self.send(Command::InteractiveParams(params, Instant::now()))
    }
    pub fn set_map(&self, map: HeightMap) -> Result<(), String> {
        self.send(Command::Map(map))
    }
    pub fn wait_map_ready(&self) -> Result<RenderStatus, String> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.send(Command::MapReady(tx))?;
        rx.recv().map_err(|_| self.status().error.unwrap_or_else(|| "渲染线程已停止".into()))
    }
    pub fn set_texture(&self, texture: ColorTexture) -> Result<(), String> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.send(Command::Texture(texture, tx))?;
        rx.recv().map_err(|e| e.to_string())?
    }
    pub fn clear_texture(&self) -> Result<(), String> {
        if self.0.export_busy.load(Ordering::Acquire) || self.status().exporting {
            return Err("正在导出".into());
        }
        let (tx, rx) = mpsc::sync_channel(1);
        self.send(Command::ClearTexture(tx))?;
        rx.recv()
            .map_err(|_| "渲染线程已停止，贴图未移除".to_string())?
    }
    pub fn texture_preview(&self)->Result<(Vec<u8>,u32,u32),String> {
        let (tx,rx)=mpsc::sync_channel(1);
        self.send(Command::TexturePreview(tx))?;
        rx.recv_timeout(Duration::from_secs(30)).map_err(|_|"读取贴图超时".to_string())?
    }
    pub fn sample_appearance(&self,x:f32,y:f32)->Result<super::AppearanceSample,String> {
        let (tx,rx)=mpsc::sync_channel(1);
        self.send(Command::SampleAppearance(x,y,tx))?;
        rx.recv_timeout(Duration::from_secs(30)).map_err(|_|"读取地形高程超时".to_string())?
    }
    pub fn appearance_distribution(&self)->Result<super::AppearanceDistribution,String> {
        let (tx,rx)=mpsc::sync_channel(1);
        self.send(Command::AppearanceDistribution(tx))?;
        rx.recv_timeout(Duration::from_secs(30)).map_err(|_|"读取预览高程分布超时".to_string())?
    }
    pub fn pause(&self, paused: bool) -> Result<(), String> {
        self.send(Command::Pause(paused))
    }
    pub fn status(&self) -> RenderStatus {
        self.0
            .status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    pub fn capture(&self) -> Result<Vec<u8>, String> {
        self.capture_frame().map(|(pixels, _, _)| pixels)
    }
    /// Pixel data and dimensions are captured atomically on the render thread.
    pub fn capture_frame(&self) -> Result<(Vec<u8>, u32, u32), String> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.send(Command::Capture(tx))?;
        rx.recv_timeout(std::time::Duration::from_secs(30))
            .map_err(|_| "读取渲染结果超时".to_string())?
    }
    pub fn capture_frame_with_metadata(&self) -> Result<(Vec<u8>, u32, u32, serde_json::Value), String> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.send(Command::CaptureWithMetadata(tx))?;
        rx.recv_timeout(Duration::from_secs(30)).map_err(|_| "读取渲染结果超时".to_string())?
    }
    /// Last successful file export, sampled from its actual final RGBA tiles.
    pub fn capture_export_result(&self) -> Result<ExportFrame, String> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.send(Command::ExportPreviewResult(tx))?;
        rx.recv().map_err(|_| "渲染线程已停止，导出结果未读取".to_string())?
    }
    /// Export the scene at this queue position at its requested output size.
    pub fn capture_output_frame(&self) -> Result<(Vec<u8>, u32, u32), String> {
        self.capture_output_frame_with_params(None)
    }
    pub fn capture_output_frame_with_params(
        &self,
        params: Option<RenderParams>,
    ) -> Result<(Vec<u8>, u32, u32), String> {
        self.capture_output_frame_internal(params, false)
    }
    pub fn capture_video_frame(&self, params:RenderParams)->Result<(Vec<u8>,u32,u32),String>{
        self.capture_output_frame_internal(Some(params), true)
    }
    fn capture_output_frame_internal(&self, params:Option<RenderParams>,preserve_grid:bool)->Result<(Vec<u8>,u32,u32),String>{
        if self
            .0
            .export_busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err("正在导出".into());
        }
        self.0.export_cancel.store(false, Ordering::Release);
        let (tx, rx) = mpsc::sync_channel(1);
        let result = self
            .send(Command::CaptureOutput(params, preserve_grid, tx))
            .and_then(|()| {
                rx.recv()
                    .map_err(|_| "渲染线程已停止，PNG 导出未完成".to_string())?
            });
        self.0.export_busy.store(false, Ordering::Release);
        result
    }
    /// Bounded orthographic preview of the complete imported preview raster.
    pub fn export_png(&self, path: PathBuf, params: Option<RenderParams>) -> Result<(), String> {
        if self
            .0
            .export_busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err("正在导出".into());
        }
        self.0.export_cancel.store(false, Ordering::Release);
        let (tx, rx) = mpsc::sync_channel(1);
        let result = self
            .send(Command::ExportPng(path, params, tx))
            .and_then(|()| {
                rx.recv()
                    .map_err(|_| "渲染线程已停止，PNG 导出未完成".to_string())?
            });
        self.0.export_busy.store(false, Ordering::Release);
        result
    }

    /// Bounded orthographic preview of the complete imported preview raster.
    pub fn capture_export_preview(
        &self,
        orthographic: bool,
        params: Option<RenderParams>,
    ) -> Result<(Vec<u8>, u32, u32), String> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.send(Command::PreviewOutput(orthographic, params, tx))?;
        rx.recv()
            .map_err(|_| "渲染线程已停止，预览未完成".to_string())?
    }
    /// A queue barrier; run the blocking wait outside Tauri's UI thread.
    pub fn export_geotiff(
        &self,
        path: PathBuf,
        params: Option<RenderParams>,
    ) -> Result<(), String> {
        if self
            .0
            .export_busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err("正在导出 TIFF".into());
        }
        self.0.export_cancel.store(false, Ordering::Release);
        let (tx, rx) = mpsc::sync_channel(1);
        let result = self
            .send(Command::ExportGeo(path, params, tx))
            .and_then(|()| {
                rx.recv()
                    .map_err(|_| "渲染线程已停止，TIFF 导出未完成".to_string())?
            });
        self.0.export_busy.store(false, Ordering::Release);
        result
    }
    pub fn export_mesh(&self,path:PathBuf,params:RenderParams)->Result<(),String>{
        if self.0.export_busy.compare_exchange(false,true,Ordering::AcqRel,Ordering::Acquire).is_err(){return Err("正在导出".into())}
        self.0.export_cancel.store(false,Ordering::Release);
        let (tx,rx)=mpsc::sync_channel(1);
        let result=self.send(Command::ExportMesh(path,params,tx)).and_then(|()|rx.recv().map_err(|_|"渲染线程已停止，模型导出未完成".to_string())?);
        self.0.export_busy.store(false,Ordering::Release);result
    }
    pub fn cancel_export(&self) {
        self.0.export_cancel.store(true, Ordering::Release);
    }
    /// Call from the UI thread before destroying the parent window.
    pub fn shutdown(&self) {
        self.cancel_export();
        let _ = self.send(Command::Shutdown);
        if let Some(worker) = self
            .0
            .worker
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            let _ = worker.join();
            unsafe {
                let _ = DestroyWindow(HWND(self.0.window as *mut c_void));
            }
        }
    }
}

const FRAMELESS_SUBCLASS: usize = 0x44454d;
const PRESENT_REFRESH: Duration = Duration::from_millis(500);

fn viewport_position_changed(
    current: [i32; 2],
    target: [i32; 2],
    showing: bool,
    visible: bool,
    at_bottom: bool,
) -> bool {
    current != target || showing != visible || !at_bottom
}

fn auto_hide_inset(mut work: RECT, edge: u32) -> RECT {
    match edge {
        ABE_BOTTOM => work.bottom -= 1,
        ABE_LEFT => work.left += 1,
        ABE_TOP => work.top += 1,
        ABE_RIGHT => work.right -= 1,
        _ => {}
    }
    work
}

unsafe fn accessible_work_area(monitor: &MONITORINFO) -> RECT {
    let mut work = monitor.rcWork;
    for edge in [ABE_LEFT, ABE_TOP, ABE_RIGHT, ABE_BOTTOM] {
        let mut bar = APPBARDATA {
            cbSize: std::mem::size_of::<APPBARDATA>() as u32,
            uEdge: edge,
            rc: monitor.rcMonitor,
            ..Default::default()
        };
        if SHAppBarMessage(ABM_GETAUTOHIDEBAREX, &mut bar) != 0 {
            work = auto_hide_inset(work, edge);
        }
    }
    work
}

fn maximized_work_area(monitor: RECT, work: RECT) -> (POINT, POINT) {
    (
        POINT {
            x: work.left - monitor.left,
            y: work.top - monitor.top,
        },
        POINT {
            x: work.right - work.left,
            y: work.bottom - work.top,
        },
    )
}

fn window_uses_rounded_corners(maximized: bool, bounds: RECT, monitor: Option<RECT>) -> bool {
    !maximized
        && !monitor.is_some_and(|screen| {
            bounds.left <= screen.left
                && bounds.top <= screen.top
                && bounds.right >= screen.right
                && bounds.bottom >= screen.bottom
        })
}

unsafe fn dwm_corner_preference(hwnd: HWND) -> Option<DWM_WINDOW_CORNER_PREFERENCE> {
    let mut preference = DWMWCP_DEFAULT;
    DwmGetWindowAttribute(
        hwnd, DWMWA_WINDOW_CORNER_PREFERENCE,
        &mut preference as *mut _ as *mut c_void,
        std::mem::size_of_val(&preference) as u32,
    ).ok().map(|_| preference)
}

// Prefer the compositor's antialiased whole-window mask. A GDI window region
// both has binary pixel coverage and prevents DWM from rounding the HWND.
// Share one rule
// between native lifecycle messages and delayed WebView resize IPC, so neither
// can restore rounded corners while maximized/fullscreen.
unsafe fn sync_window_shape(hwnd: HWND) -> Result<(), String> {
    if IsIconic(hwnd).as_bool() {
        return Ok(());
    }
    let mut bounds = RECT::default();
    GetWindowRect(hwnd, &mut bounds).map_err(|e| e.to_string())?;
    let width = bounds.right - bounds.left;
    let height = bounds.bottom - bounds.top;
    if width <= 0 || height <= 0 {
        return Ok(());
    }
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let screen = GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info)
        .as_bool()
        .then_some(info.rcMonitor);
    let rounded = window_uses_rounded_corners(IsZoomed(hwnd).as_bool(), bounds, screen);
    let preference = if rounded { DWMWCP_ROUND } else { DWMWCP_DONOTROUND };
    let compositor_supported = dwm_corner_preference(hwnd) == Some(preference)
        || DwmSetWindowAttribute(
            hwnd, DWMWA_WINDOW_CORNER_PREFERENCE,
            &preference as *const _ as *const c_void,
            std::mem::size_of_val(&preference) as u32,
        ).is_ok();
    if compositor_supported {
        let previous = CreateRectRgn(0, 0, 0, 0);
        let has_region = !previous.is_invalid() && GetWindowRgn(hwnd, previous).0 != ERROR;
        if !previous.is_invalid() { let _ = DeleteObject(previous.into()); }
        if has_region && SetWindowRgn(hwnd, None, true) == 0 {
            return Err("清除窗口硬裁切区域失败".into());
        }
        return Ok(());
    }
    // Windows 10 does not expose DWMWA_WINDOW_CORNER_PREFERENCE. Retain the
    // existing shape there rather than losing clipping of native descendants.
    let region = if rounded {
        CreateRoundRectRgn(0, 0, width + 1, height + 1, 16, 16)
    } else {
        CreateRectRgn(0, 0, width, height)
    };
    if region.is_invalid() {
        return Err("创建窗口裁切区域失败".into());
    }
    let previous = CreateRectRgn(0, 0, 0, 0);
    let same = !previous.is_invalid()
        && GetWindowRgn(hwnd, previous).0 != ERROR
        && EqualRgn(region, previous).as_bool();
    if !previous.is_invalid() {
        let _ = DeleteObject(previous.into());
    }
    if same {
        let _ = DeleteObject(region.into());
    } else if SetWindowRgn(hwnd, Some(region), true) == 0 {
        let _ = DeleteObject(region.into());
        return Err("应用窗口裁切区域失败".into());
    }
    // Windows owns the region after a successful SetWindowRgn.
    Ok(())
}

// Runs on the owning thread, even inside the native move/size modal loop.
// The native surface must not wait for a Chromium/JS layout round-trip.
fn stable_surface_capacity(current: [i32; 2], client: [i32; 2], monitor: [i32; 2]) -> [i32; 2] {
    [current[0].max(client[0]).max(monitor[0]).max(1),
     current[1].max(client[1]).max(monitor[1]).max(1)]
}
unsafe fn viewport_capacity(parent: HWND, current: [i32; 2], client: [i32; 2]) -> [i32; 2] {
    let mut monitor = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    let size = if GetMonitorInfoW(MonitorFromWindow(parent, MONITOR_DEFAULTTONEAREST), &mut monitor).as_bool() {
        [monitor.rcMonitor.right-monitor.rcMonitor.left, monitor.rcMonitor.bottom-monitor.rcMonitor.top]
    } else { client };
    stable_surface_capacity(current, client, size)
}
unsafe fn refresh_native_viewport(parent: HWND, viewport: HWND) {
    if viewport.is_invalid() || IsIconic(parent).as_bool() { return; }
    let mut client = RECT::default();
    let mut current = RECT::default();
    if GetClientRect(parent, &mut client).is_ok() && GetClientRect(viewport, &mut current).is_ok()
        && client.right > 0 && client.bottom > 0 {
        let capacity = viewport_capacity(parent, [current.right,current.bottom], [client.right,client.bottom]);
        // Ordinary sizing leaves the WGL drawable intact. The parent clips the
        // larger child; only a larger monitor/client requires capacity growth.
        if capacity != [current.right,current.bottom] {
            let _ = SetWindowPos(viewport, Some(HWND_BOTTOM), 0, 0, capacity[0], capacity[1],
                SWP_NOACTIVATE | SWP_NOCOPYBITS);
        }
    }
    request_repaint(viewport);
}

unsafe fn request_repaint(viewport: HWND) {
    let input = GetWindowLongPtrW(viewport, GWLP_USERDATA) as *mut WindowInput;
    if !input.is_null() && !(*input).repaint_queued.swap(true, Ordering::AcqRel) {
        let _ = (*input).sender.send(Command::Repaint);
    }
}

// Release the queue gate before drawing: a resize which arrives while the GPU
// is presenting belongs to the next frame, not the one already in flight.
fn present_with_repaint_gate<T>(queued: &AtomicBool, present: impl FnOnce() -> T) -> T {
    queued.store(false, Ordering::Release);
    present()
}

fn command_needs_immediate_present(command: &Command) -> bool {
    matches!(command, Command::Repaint | Command::Rect(_) | Command::EndResize(..))
}

fn priority_resize_command(handoff:&Mutex<Option<(usize,usize)>>,pending:&mut Option<Command>) -> Option<Command> {
    handoff.lock().unwrap_or_else(|e|e.into_inner()).take()
        .map(|(epoch,parent)|Command::EndResize(epoch,parent)).or_else(||pending.take())
}

fn frameless_style(style: u32) -> u32 {
    style & !WS_CAPTION.0
}

unsafe fn ensure_frameless(parent: HWND, viewport: HWND) -> Result<(), String> {
    if !SetWindowSubclass(
        parent,
        Some(frameless_proc),
        FRAMELESS_SUBCLASS,
        viewport.0 as usize,
    )
    .as_bool()
    {
        return Err("安装无边框绘制处理失败".into());
    }
    let style = GetWindowLongPtrW(parent, GWL_STYLE) as u32;
    let next = frameless_style(style);
    if next != style {
        SetLastError(WIN32_ERROR(0));
        let old = SetWindowLongPtrW(parent, GWL_STYLE, next as isize);
        if old == 0 && GetLastError() != WIN32_ERROR(0) {
            return Err(format!(
                "清除系统标题栏失败: {}",
                windows::core::Error::from_thread()
            ));
        }
        SetWindowPos(
            parent,
            None,
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER,
        )
        .map_err(|e| format!("刷新无边框样式失败: {e}"))?;
    }
    Ok(())
}

// Preserve Tao's activation/focus bookkeeping and native sizing. Only prevent
// Windows from painting a second title bar/border over the custom client UI.
const RESIZE_FRAME_READY:u32=WM_APP+0x3a1;

unsafe fn begin_resize_snapshot(parent:HWND,viewport:HWND) {
    let input=GetWindowLongPtrW(viewport,GWLP_USERDATA) as *mut WindowInput;
    if input.is_null()||(*input).resizing {return;}
    (*input).resizing=true;
    let (send,receive)=mpsc::sync_channel(1);
    if (*input).sender.send(Command::RetainFrame(send)).is_err() {return;}
    if let Ok(Ok((pixels,width,height)))=receive.recv_timeout(Duration::from_millis(250)) {
        if (*input).resize_snapshot.is_none() {
            (*input).resize_snapshot=super::resize_snapshot::ResizeSnapshot::new(parent,viewport).ok();
        }
        if let Some(snapshot)=&mut (*input).resize_snapshot {
            if let Err(error)=snapshot.show(width,height,pixels) {eprintln!("缩放保帧: {error}");}
        }
    }
}

unsafe extern "system" fn frameless_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    id: usize,
    data: usize,
) -> LRESULT {
    match msg {
        WM_ENTERSIZEMOVE if data!=0 => {
            let input=GetWindowLongPtrW(HWND(data as *mut c_void),GWLP_USERDATA) as *mut WindowInput;
            if !input.is_null() {(*input).resize_epoch=(*input).resize_epoch.wrapping_add(1);}
            DefSubclassProc(hwnd,msg,wp,lp)
        }
        WM_SIZING if data!=0 => {
            begin_resize_snapshot(hwnd,HWND(data as *mut c_void));
            DefSubclassProc(hwnd,msg,wp,lp)
        }
        WM_EXITSIZEMOVE if data!=0 => {
            let result=DefSubclassProc(hwnd,msg,wp,lp);
            let viewport=HWND(data as *mut c_void);
            let input=GetWindowLongPtrW(viewport,GWLP_USERDATA) as *mut WindowInput;
            refresh_native_viewport(hwnd,viewport);
            if !input.is_null() {
                (*input).resizing=false;
                *(*input).resize_handoff.lock().unwrap_or_else(|e|e.into_inner())=
                    Some(((*input).resize_epoch,hwnd.0 as usize));
                // Wake the worker, but do not put the handoff behind the resize
                // IPC backlog. Its dedicated slot is checked before that queue.
                let _=(*input).sender.send(Command::Repaint);
            }
            result
        }
        RESIZE_FRAME_READY if data!=0 && lp.0==data as isize => {
            let input=GetWindowLongPtrW(HWND(data as *mut c_void),GWLP_USERDATA) as *mut WindowInput;
            if !input.is_null()&&!(*input).resizing&&(*input).resize_epoch==wp.0 {
                if let Some(snapshot)=&mut (*input).resize_snapshot {snapshot.hide();}
            }
            LRESULT(0)
        }
        WM_STYLECHANGING if wp.0 as i32 == GWL_STYLE.0 && lp.0 != 0 => {
            let styles = &mut *(lp.0 as *mut STYLESTRUCT);
            styles.styleNew = frameless_style(styles.styleNew);
            DefSubclassProc(hwnd, msg, wp, lp)
        }
        WM_GETMINMAXINFO if lp.0 != 0 => {
            // Preserve Tao's track-size constraints, then bound the whole HWND
            // to rcWork. Client-only clipping can leave HWND over the taskbar.
            let result = DefSubclassProc(hwnd, msg, wp, lp);
            let mut monitor = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if GetMonitorInfoW(
                MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST),
                &mut monitor,
            )
            .as_bool()
            {
                let (position, size) =
                    maximized_work_area(monitor.rcMonitor, accessible_work_area(&monitor));
                let info = &mut *(lp.0 as *mut MINMAXINFO);
                info.ptMaxPosition = position;
                info.ptMaxSize = size;
            }
            result
        }
        WM_NCPAINT => LRESULT(0),
        WM_ERASEBKGND => LRESULT(1),
        WM_NCACTIVATE => DefSubclassProc(hwnd, msg, wp, LPARAM(-1)),
        WM_ACTIVATE | WM_SIZE | WM_SHOWWINDOW | WM_WINDOWPOSCHANGED
        | WM_DISPLAYCHANGE | WM_DPICHANGED => {
            let result = DefSubclassProc(hwnd, msg, wp, lp);
            if data != 0 {
                refresh_native_viewport(hwnd, HWND(data as *mut c_void));
                let _ = sync_window_shape(hwnd);
                let input=GetWindowLongPtrW(HWND(data as *mut c_void),GWLP_USERDATA) as *mut WindowInput;
                if !input.is_null() {
                    if let Some(snapshot)=&mut (*input).resize_snapshot {let _=snapshot.resize();}
                }
            }
            result
        }
        WM_NCDESTROY => {
            let _ = RemoveWindowSubclass(hwnd, Some(frameless_proc), id);
            DefSubclassProc(hwnd, msg, wp, lp)
        }
        _ => DefSubclassProc(hwnd, msg, wp, lp),
    }
}

unsafe fn window_class(hwnd: HWND) -> Result<String, String> {
    let mut name = [0u16; 256];
    let count = GetClassNameW(hwnd, &mut name);
    if count == 0 {
        return Err(format!(
            "读取窗口类失败: {}",
            windows::core::Error::from_thread()
        ));
    }
    Ok(String::from_utf16_lossy(&name[..count as usize]))
}

/// Only the direct, same-parent Wry container is changed; no Chromium internals.
unsafe fn configure_webview_sibling(viewport: HWND) -> Result<(), String> {
    let parent = GetParent(viewport).map_err(|e| format!("读取视口父窗口失败: {e}"))?;
    let mut child = GetWindow(parent, GW_CHILD).ok();
    let mut found = false;
    while let Some(hwnd) = child {
        if hwnd != viewport && window_class(hwnd)? == "WRY_WEBVIEW" {
            found = true;
            let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
            if style as u32 & WS_CLIPSIBLINGS.0 != 0 {
                SetLastError(WIN32_ERROR(0));
                let old = SetWindowLongPtrW(hwnd, GWL_STYLE, style & !(WS_CLIPSIBLINGS.0 as isize));
                if old == 0 && GetLastError() != WIN32_ERROR(0) {
                    return Err(format!(
                        "设置WebView兄弟裁剪失败: {}",
                        windows::core::Error::from_thread()
                    ));
                }
                SetWindowPos(
                    hwnd,
                    None,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED,
                )
                .map_err(|e| format!("刷新WebView裁剪失败: {e}"))?;
                if !RedrawWindow(Some(hwnd), None, None, RDW_INVALIDATE | RDW_ALLCHILDREN).as_bool()
                {
                    return Err(format!(
                        "重绘WebView失败: {}",
                        windows::core::Error::from_thread()
                    ));
                }
            }
        }
        child = GetWindow(hwnd, GW_HWNDNEXT).ok();
    }
    if !found {
        return Err("未找到与原生视口同父的WRY_WEBVIEW窗口".into());
    }
    Ok(())
}

struct WindowInput {
    app: tauri::AppHandle,
    sender: mpsc::Sender<Command>,
    right_gesture: Option<RightGesture>,
    released: Arc<AtomicBool>,
    repaint_queued: Arc<AtomicBool>,
    resize_snapshot: Option<super::resize_snapshot::ResizeSnapshot>,
    resizing: bool,
    resize_epoch: usize,
    resize_handoff: Arc<Mutex<Option<(usize,usize)>>>,
}

// Four logical pixels separate a menu click from camera movement.
#[link(name = "user32")]
extern "system" {
    #[link_name = "GetDpiForWindow"]
    fn viewport_window_dpi(hwnd: HWND) -> u32;
}

#[derive(Clone, Copy)]
struct RightGesture {
    origin: (i32, i32),
    last: (i32, i32),
    threshold: i32,
    dragged: bool,
    shift_seen: bool,
}
impl RightGesture {
    fn new(pos: (i32, i32), dpi: u32, mouse_keys: usize) -> Self {
        Self { origin: pos, last: pos, threshold: ((4 * dpi.max(96) + 95) / 96) as i32,
            dragged: false, shift_seen: mouse_keys & 0x0004 != 0 }
    }
    fn exceeds_threshold(&self, pos: (i32, i32)) -> bool {
        (pos.0 - self.origin.0).abs().max((pos.1 - self.origin.1).abs()) >= self.threshold
    }
    fn movement(&mut self, pos: (i32, i32), mouse_keys: usize) -> Option<Orbit> {
        self.shift_seen |= mouse_keys & 0x0004 != 0;
        self.dragged |= self.exceeds_threshold(pos);
        if !self.dragged { return None; }
        let delta = Orbit { dx: pos.0 - self.last.0, dy: pos.1 - self.last.1 };
        self.last = pos;
        Some(delta)
    }
    fn is_menu_click(&self, pos: (i32, i32), mouse_keys: usize) -> bool {
        !self.dragged && !self.shift_seen && mouse_keys & 0x0004 == 0 && !self.exceeds_threshold(pos)
    }
}

#[derive(Clone, serde::Serialize)]
struct Orbit {
    dx: i32,
    dy: i32,
}
#[derive(Clone, serde::Serialize)]
struct Zoom {
    delta: f32,
}
fn drag_event_name(mouse_keys: usize) -> Option<&'static str> {
    // WM_MOUSEMOVE wParam: MK_RBUTTON=0x0002, MK_SHIFT=0x0004.
    if mouse_keys & 0x0002 == 0 {
        None
    } else if mouse_keys & 0x0004 != 0 {
        Some("viewport-pan")
    } else {
        Some("viewport-orbit")
    }
}
unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let create = &*(lp.0 as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
    }
    let input = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WindowInput;
    if msg == WM_NCDESTROY {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        if !input.is_null() {
            (*input).released.store(true, Ordering::Release);
            drop(Box::from_raw(input));
        }
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    match msg {
        WM_LBUTTONDOWN if !input.is_null() => {
            let mut rect=RECT::default();
            if GetParent(hwnd).and_then(|parent| GetClientRect(parent,&mut rect)).is_ok() && rect.right>0 && rect.bottom>0 {
                let x=(lp.0 as i16 as f32)/rect.right as f32;
                let y=((lp.0>>16) as i16 as f32)/rect.bottom as f32;
                let _=(*input).app.emit("viewport-pick",serde_json::json!({"x":x,"y":y}));
            }
            return LRESULT(0);
        }
        WM_KEYDOWN if !input.is_null() && wp.0 == 0x1b && lp.0 & (1 << 30) == 0 => {
            // Preserve default Escape handling while allowing a focused native viewport
            // to hand temporary MCP control back to the visible WebView HUD.
            let _=(*input).app.emit("viewport-restore-hud", ());
        }
        WM_KEYDOWN if !input.is_null() && wp.0 == 0xc0 && lp.0 & (1 << 30) == 0 => {
            if GetKeyState(0x11) < 0 && GetKeyState(0x10) < 0
                && [0x12,0x5b,0x5c].into_iter().all(|key| GetKeyState(key) >= 0) {
                let _=(*input).app.emit("viewport-test-bounces", ());
                return LRESULT(0);
            }
        }
        WM_PAINT => {
            if !input.is_null() {
                request_repaint(hwnd);
            }
            let _ = ValidateRect(Some(hwnd), None);
            return LRESULT(0);
        }
        WM_ERASEBKGND => return LRESULT(1),
        WM_RBUTTONDOWN if !input.is_null() => {
            let pos = (lp.0 as i16 as i32, (lp.0 >> 16) as i16 as i32);
            (*input).right_gesture = Some(RightGesture::new(pos, viewport_window_dpi(hwnd), wp.0));
            SetCapture(hwnd);
            return LRESULT(0);
        }
        WM_MOUSEMOVE if !input.is_null() => {
            let pos = (lp.0 as i16 as i32, (lp.0 >> 16) as i16 as i32);
            if let Some(event) = drag_event_name(wp.0) {
                if let Some(gesture) = (*input).right_gesture.as_mut() {
                    if let Some(delta) = gesture.movement(pos, wp.0) {
                        let _ = (*input).app.emit(event, delta);
                    }
                }
            } else if (*input).right_gesture.take().is_some() {
                let _ = ReleaseCapture();
            }
            return LRESULT(0);
        }
        WM_RBUTTONUP if !input.is_null() => {
            let pos = (lp.0 as i16 as i32, (lp.0 >> 16) as i16 as i32);
            // Take before releasing capture, which synchronously sends WM_CAPTURECHANGED.
            let gesture = (*input).right_gesture.take();
            let _ = ReleaseCapture();
            if gesture.is_some_and(|gesture| gesture.is_menu_click(pos, wp.0)) {
                let mut point = POINT { x: pos.0, y: pos.1 };
                if let Ok(parent) = GetParent(hwnd) {
                    if ClientToScreen(hwnd, &mut point).as_bool()
                        && ScreenToClient(parent, &mut point).as_bool() {
                        let _ = (*input).app.emit("viewport-context-menu",
                            serde_json::json!({ "x": point.x, "y": point.y }));
                    }
                }
            }
            return LRESULT(0);
        }
        WM_CAPTURECHANGED | WM_CANCELMODE if !input.is_null() => {
            (*input).right_gesture = None;
            if msg == WM_CANCELMODE { let _ = ReleaseCapture(); }
            return LRESULT(0);
        }
        // The native child never opens an OS/browser default context menu.
        WM_CONTEXTMENU => return LRESULT(0),
        WM_MOUSEWHEEL if !input.is_null() => {
            let _ = (*input).app.emit(
                "viewport-zoom",
                Zoom {
                    delta: (wp.0 >> 16) as i16 as f32 / 120.0,
                },
            );
            return LRESULT(0);
        }
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

/// Creates a hidden sibling of the WebView on its UI thread; no standalone window.
// Tao's transparent host enables DWM desktop transparency. Only the WebView
// needs alpha here: its transparent pixels must reveal our opaque WGL sibling,
// never the desktop when DWM replaces the host's redirection surface on resize.
unsafe fn make_host_opaque(parent: HWND) -> Result<(), String> {
    DwmEnableBlurBehindWindow(parent, &DWM_BLURBEHIND {
        dwFlags: DWM_BB_ENABLE,
        fEnable: false.into(),
        ..Default::default()
    }).map_err(|e| format!("关闭宿主桌面透明失败: {e}"))
}

pub fn start(parent: isize, app: tauri::AppHandle) -> Result<RendererHandle, String> {
    let (sender, receiver) = mpsc::channel();
    let repaint_queued = Arc::new(AtomicBool::new(false));
    let resize_handoff=Arc::new(Mutex::new(None));
    let hwnd = unsafe {
        make_host_opaque(HWND(parent as *mut c_void))?;
        // Protect the parent before creation/first resize can expose its frame.
        ensure_frameless(HWND(parent as *mut c_void), HWND::default())?;
        let module = GetModuleHandleW(None).map_err(|e| e.to_string())?;
        let class = WNDCLASSW {
            style: CS_OWNDC,
            lpfnWndProc: Some(window_proc),
            hInstance: HINSTANCE(module.0),
            lpszClassName: w!("DEMStudioNativeViewport"),
            ..Default::default()
        };
        // An already registered class is reusable for this process.
        if RegisterClassW(&class) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
            return Err(format!(
                "注册原生视口失败: {}",
                windows::core::Error::from_thread()
            ));
        }
        let released = Arc::new(AtomicBool::new(false));
        let input = Box::into_raw(Box::new(WindowInput {
            app,
            sender: sender.clone(),
            right_gesture: None,
            released: released.clone(),
            repaint_queued: repaint_queued.clone(),
            resize_snapshot: None,
            resizing: false,
            resize_epoch: 0,
            resize_handoff: resize_handoff.clone(),
        }));
        // WM_NCDESTROY owns userdata after WM_NCCREATE. Class/parent were validated above.
        match CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class.lpszClassName,
            w!(""),
            WS_CHILD | WS_CLIPCHILDREN,
            0,
            0,
            1,
            1,
            Some(HWND(parent as *mut c_void)),
            None,
            Some(HINSTANCE(module.0)),
            Some(input as *const c_void),
        ) {
            Ok(hwnd) => hwnd,
            Err(error) => {
                // Failures before WM_NCCREATE have no WM_NCDESTROY to release userdata.
                if !released.load(Ordering::Acquire) {
                    drop(Box::from_raw(input));
                }
                return Err(format!("创建原生视口失败: {error}"));
            }
        }
    };
    unsafe {
        if let Err(error) = ensure_frameless(HWND(parent as *mut c_void), hwnd) {
            let _ = DestroyWindow(hwnd);
            return Err(error);
        }
    }
    let window = hwnd.0 as isize;
    let status = Arc::new(Mutex::new(RenderStatus::default()));
    let worker_status = status.clone();
    let export_cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = export_cancel.clone();
    let worker = thread::Builder::new()
        .name("DEMStudio-OpenGL".into())
        .spawn(move || {
            let result = unsafe {
                render_thread(
                    window,
                    receiver,
                    &worker_status,
                    &worker_cancel,
                    &repaint_queued,
                    &resize_handoff,
                )
            };
            let mut state = worker_status.lock().unwrap_or_else(|e| e.into_inner());
            state.ready = false;
            if let Err(error) = result {
                state.error = Some(error);
            }
        })
        .map_err(|e| {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            format!("启动渲染线程失败: {e}")
        })?;
    Ok(RendererHandle(Arc::new(Shared {
        window,
        sender,
        status,
        worker: Mutex::new(Some(worker)),
        export_cancel,
        export_busy: AtomicBool::new(false),
    })))
}

struct WglContext {
    window: HWND,
    dc: HDC,
    context: HGLRC,
}


impl Drop for WglContext {
    fn drop(&mut self) {
        unsafe {
            let _ = wglMakeCurrent(HDC::default(), HGLRC::default());
            let _ = wglDeleteContext(self.context);
            ReleaseDC(Some(self.window), self.dc);
        }
    }
}
impl WglContext {
    unsafe fn new(window: HWND) -> Result<Self, String> {
        let dc = GetDC(Some(window));
        if dc.0.is_null() {
            return Err("获取视口DC失败".into());
        }
        let pfd = PIXELFORMATDESCRIPTOR {
            nSize: std::mem::size_of::<PIXELFORMATDESCRIPTOR>() as u16,
            nVersion: 1,
            dwFlags: PFD_DRAW_TO_WINDOW | PFD_SUPPORT_OPENGL | PFD_DOUBLEBUFFER,
            iPixelType: PFD_TYPE_RGBA,
            cColorBits: 32,
            cAlphaBits: 8,
            cDepthBits: 24,
            iLayerType: PFD_MAIN_PLANE.0 as u8,
            ..Default::default()
        };
        let format = ChoosePixelFormat(dc, &pfd);
        if format == 0 {
            ReleaseDC(Some(window), dc);
            return Err("无可用OpenGL像素格式".into());
        }
        if let Err(e) = SetPixelFormat(dc, format, &pfd) {
            ReleaseDC(Some(window), dc);
            return Err(e.to_string());
        }
        let context = match wglCreateContext(dc) {
            Ok(c) => c,
            Err(e) => {
                ReleaseDC(Some(window), dc);
                return Err(e.to_string());
            }
        };
        let guard = Self {
            window,
            dc,
            context,
        };
        wglMakeCurrent(dc, context).map_err(|e| e.to_string())?;
        Ok(guard)
    }
}

// The stored result preview remains bounded RGBA8, independently of file depth.
fn output_thumbnail_bytes(pixels: &[u8], depth: u8) -> std::borrow::Cow<'_, [u8]> {
    if depth==16 {
        std::borrow::Cow::Owned(pixels.chunks_exact(2).map(|p|((u32::from(u16::from_ne_bytes([p[0],p[1]]))+128)/257) as u8).collect())
    } else {std::borrow::Cow::Borrowed(pixels)}
}

unsafe fn export_source_grid(
    renderer: &mut GlRenderer,
    preview: &HeightMap,
    desired: &RenderParams,
    path: &Path,
    status: &Arc<Mutex<RenderStatus>>,
    cancel: &AtomicBool,
) -> Result<ExportFrame, String> {
    export_image(renderer, preview, desired, path, status, cancel, true)
}

// One bounded, whole-frame statistics pass is frozen for all output tiles.
// This never allocates a source-sized histogram/render buffer.
unsafe fn freeze_export_stretch(
    renderer: &mut GlRenderer, desired: &RenderParams, size: [u32;2], geo: bool, cancel: &AtomicBool,
) -> Result<(),String> {
    freeze_export_stretch_with_source(renderer,desired,size,geo,cancel,&mut None)
}

unsafe fn freeze_export_stretch_with_source(
    renderer:&mut GlRenderer,desired:&RenderParams,size:[u32;2],geo:bool,cancel:&AtomicBool,
    paged:&mut Option<PagedExport>,
)->Result<(),String>{
    renderer.clear_frozen_stretch();
    if desired.stretch_type == "none" { return Ok(()); }
    let scale=256. / size[0].max(size[1]).max(1) as f64;
    let mut pilot=desired.clone();
    pilot.width=(size[0] as f64*scale).round().max(2.) as u32;
    pilot.height=(size[1] as f64*scale).round().max(2.) as u32;
    pilot.samples=4;pilot.grid=false;
    if pilot.export_terrain_scale!="preview" {pilot.lod=0;}
    renderer.set_params(&pilot)?;
    if geo { renderer.set_geo_tile(Some([0,0,size[0],size[1]]))?; }
    else { renderer.set_output_tile(None)?; }
    renderer.reset_accumulation();
    renderer.restart_sampling_sequence();
    while renderer.samples() < 4 {
        if cancel.load(Ordering::Acquire){return Err(if geo {"已取消 TIFF 导出"} else {"已取消 PNG 导出"}.into());}
        match paged.as_mut() {
            Some(source)=>{renderer.render_source_sample(source,cancel)?;},
            None=>{renderer.render_export_sample_step()?;},
        }
    }
    if let Some(source)=paged.as_mut(){renderer.finish_source_surface(source,cancel)?;}
    renderer.freeze_stretch_from_current();
    Ok(())
}

fn export_image_extent(params: &RenderParams) -> Result<super::export_extent::ExportExtent,String> {
    if params.width == 0 || params.height == 0 { return Err("输出尺寸无效".into()); }
    let mut effective=params.clone();
    effective.export_shadow &= effective.export_orthographic;
    super::export_extent::ExportExtent::for_source(params.width,params.height,&effective)
}

unsafe fn export_camera_png(
    renderer: &mut GlRenderer,
    preview: &HeightMap,
    desired: &RenderParams,
    path: &Path,
    status: &Arc<Mutex<RenderStatus>>,
    cancel: &AtomicBool,
) -> Result<ExportFrame, String> {
    export_image(renderer, preview, desired, path, status, cancel, false)
}

unsafe fn export_image(
    renderer: &mut GlRenderer, preview: &HeightMap, desired: &RenderParams,
    path: &Path, status: &Arc<Mutex<RenderStatus>>, cancel: &AtomicBool, tiff: bool,
) -> Result<ExportFrame,String> {
    let cancelled="已取消图像导出";
    if cancel.load(Ordering::Acquire) { return Err(cancelled.into()); }
    let mut params = desired.clone();
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if !tiff && matches!(extension.as_str(), "jpg" | "jpeg" | "bmp" | "tga") {
        params.output_bit_depth = 8;
        if matches!(extension.as_str(), "jpg" | "jpeg" | "bmp") { params.export_transparent = false; }
    }
    params.grid = false;
    if params.export_orthographic { params.projection = "orthographic".into(); }
    let extent=export_image_extent(&params)?;
    if extent.width > super::png_output::MAX_EDGE || extent.height > super::png_output::MAX_EDGE {
        return Err("输出尺寸超出范围".into());
    }
    let extra_format = !tiff && !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("png"));
    if extra_format { super::image_output::validate(path, extent.width, extent.height, params.output_bit_depth)?; }
    let geo=if params.export_orthographic && params.export_georeference {
        let source=preview.source.as_ref().ok_or("源图像没有坐标系")?;
        let geo=source.geo.as_ref().filter(|geo|geo.has_crs()).ok_or("源图像没有坐标系")?;
        Some(geo.resized_grid(source.width,source.height,params.width,params.height,extent.left,extent.top)?)
    } else {None};
    super::georef::validate_output_sidecars(path,geo.is_some())?;
    let layers=super::export_layers::selected(&params);
    for layer in &layers {
        let output=super::export_layers::path(path,layer)?;
        super::georef::validate_output_sidecars(&output,geo.is_some())?;
        if preview.source.as_ref().is_some_and(|source|std::fs::canonicalize(&output).ok().is_some_and(|p|std::fs::canonicalize(&source.path).ok().as_ref()==Some(&p))) {
            return Err("图层输出不能覆盖当前输入图像".into());
        }
    }
    renderer.set_export_layer("beauty")?;

    let mut diagnostic=ExportDiagnostic::new(&params);
    let mut paged=prepare_export_terrain(renderer,preview,&params,params.export_orthographic,cancel,status)
        .map_err(|e|if e=="已取消导出" {cancelled.to_owned()} else {e})?;
    diagnostic.preparation=diagnostic.started.elapsed().as_secs_f64();
    let result=(||->Result<ExportFrame,String>{
    if params.export_orthographic {
        renderer.set_output_tile(None)?;
        renderer.set_geo_grid(Some([params.width, params.height, extent.left, extent.top]))?;
    } else {
        renderer.set_geo_tile(None)?;
    }
    let depth_range=renderer.export_depth_range_for(&params);
    renderer.set_export_depth_range(depth_range)?;
    let samples = params.samples.max(1);
    let total = u64::from(extent.width) * u64::from(extent.height) * u64::from(samples) * (1 + layers.len() as u64);
    let mut completed = 0u64;
    let mut thumbnail = Thumbnail::new(extent.width, extent.height)?;
    freeze_export_stretch_with_source(renderer, &params, [extent.width,extent.height], params.export_orthographic, cancel, &mut paged)?;
    let writer_started=Instant::now();
    let mut render_tile = |x, y, width, height| {
            if cancel.load(Ordering::Acquire) { return Err(cancelled.into()); }
            let callback_started=Instant::now();
            let pixels=render_export_tile(renderer,&mut paged,&params,[extent.width,extent.height],
                [x,y,width,height],params.export_orthographic,&mut completed,total,status,cancel,&mut diagnostic,"beauty");
            let pixels=match pixels {Ok(p)=>p,Err(e)=>{diagnostic.callbacks+=callback_started.elapsed().as_secs_f64();return Err(e)}};
            let thumbnail_started=Instant::now();
            let preview = output_thumbnail_bytes(&pixels, params.output_bit_depth);
            let added=thumbnail.add_tile(x, y, width, height, &preview);
            diagnostic.thumbnail+=thumbnail_started.elapsed().as_secs_f64();
            diagnostic.callbacks+=callback_started.elapsed().as_secs_f64();
            added?;
            Ok(pixels)
        };
    let written=if tiff {
        super::geotiff::write_image_depth(path,extent.width,extent.height,geo.as_ref(),params.output_bit_depth,&mut render_tile)
    } else if extra_format {
        super::image_output::write_depth(path,extent.width,extent.height,params.output_bit_depth,&mut render_tile)
    } else {
        super::png_output::write_depth(path,extent.width,extent.height,params.output_bit_depth,&mut render_tile)
    };
    diagnostic.writer=writer_started.elapsed().as_secs_f64();
    written?;
    if let Some(geo)=geo.as_ref() { super::georef::write_png_aux(path,geo)?; }
    else {super::georef::clear_output_aux(path)?;}
    let frame=thumbnail.finish()?;
    renderer.clear_frozen_stretch();
    let mut layer_params=params.clone();
    layer_params.output_bit_depth=16;
    layer_params.stretch_type="none".into();
    for layer in &layers {
        if cancel.load(Ordering::Acquire) { return Err(format!("{cancelled}；主图已写出")); }
        let layer_path=super::export_layers::path(path,layer)?;
        renderer.set_export_layer(layer)?;
        renderer.reset_accumulation();
        renderer.restart_sampling_sequence();
        let layer_started=Instant::now();
        let written=super::png_output::write_data_depth(&layer_path,extent.width,extent.height,16,|x,y,w,h| {
            let callback_started=Instant::now();
            let pixels=render_export_tile(renderer,&mut paged,&layer_params,[extent.width,extent.height],
                [x,y,w,h],params.export_orthographic,&mut completed,total,status,cancel,&mut diagnostic,layer);
            diagnostic.callbacks+=callback_started.elapsed().as_secs_f64();
            pixels
        });
        diagnostic.writer+=layer_started.elapsed().as_secs_f64();
        written.map_err(|e|format!("{layer} 图层导出失败：{e}；主图已写出"))?;
        if let Some(geo)=geo.as_ref() {super::georef::write_png_aux(&layer_path,geo)?;}
        else {super::georef::clear_output_aux(&layer_path)?;}
    }
    if !layers.is_empty() {super::export_layers::write_metadata(path,&layers,[extent.width,extent.height],depth_range)?;}
    Ok(frame)
    })();
    let restored_layer=renderer.set_export_layer("beauty");
    let result=result.and_then(|frame|restored_layer.map(|()|frame));
    let result=result.map_err(|e|if e=="已取消导出" {cancelled.to_owned()} else {e});
    diagnostic.outcome=match &result {Ok(_)=>"completed".into(),Err(e)=>e.clone()};
    if let Some(mut source)=paged.take(){
        renderer.set_source_export_context(&mut source,diagnostic.report());
        renderer.end_source_export(source);
    }
    result
}

unsafe fn render_thread(
    window: isize,
    receiver: mpsc::Receiver<Command>,
    status: &Arc<Mutex<RenderStatus>>,
    export_cancel: &AtomicBool,
    repaint_queued: &AtomicBool,
    resize_handoff: &Mutex<Option<(usize,usize)>>,
) -> Result<(), String> {
    let context = WglContext::new(HWND(window as *mut c_void))?;
    let opengl = GetModuleHandleW(w!("opengl32.dll")).map_err(|e| e.to_string())?;
    let gl = glow::Context::from_loader_function(|name| {
        let name = CString::new(name).unwrap();
        let ptr = wglGetProcAddress(PCSTR(name.as_ptr() as *const u8))
            .map(|p| p as *const c_void)
            .unwrap_or(std::ptr::null());
        if ptr.is_null() || (ptr as usize) <= 3 || ptr as isize == -1 {
            GetProcAddress(opengl, PCSTR(name.as_ptr() as *const u8))
                .map(|p| p as *const c_void)
                .unwrap_or(std::ptr::null())
        } else {
            ptr
        }
    });
    let mut renderer = GlRenderer::new(gl)?;
    let (driver, version) = renderer.driver();
    {
        let mut s = status.lock().unwrap_or_else(|e| e.into_inner());
        s.ready = true;
        s.gl_renderer = driver;
        s.gl_version = version;
    }
    let mut rect = ViewportRect {
        x: 0,
        y: 0,
        width: 1,
        height: 1,
        visible: false,
        overlays: Vec::new(),
    };
    let mut paused = false;
    let mut has_map = false;
    let mut current_preview: Option<HeightMap> = None;
    let mut current_texture:Option<ColorTexture>=None;
    let mut export_result: Option<ExportFrame> = None;
    let mut target = RenderParams::default().samples;
    let mut desired = RenderParams::default();
    let mut preview = false;
    // Finish at most one small preview before applying the latest camera. A
    // stream of input must not repeatedly cancel every partial preview frame.
    let mut deferred_preview = false;
    let mut deadline: Option<Instant> = None;
    let mut dirty = false;
    let mut repaint = false;
    let mut pending = None;
    let mut last_present = Instant::now();
    let mut resize_ready: Option<(usize,usize)> = None;
    loop {
        let parent = GetAncestor(context.window, GA_ROOT);
        let visible = has_map
            && rect.visible
            && rect.width > 0
            && rect.height > 0
            && IsWindowVisible(parent).as_bool()
            && IsWindowVisible(context.window).as_bool()
            && !IsIconic(parent).as_bool();
        let active = visible && !paused;
        let priority=priority_resize_command(resize_handoff,&mut pending);
        let command = if priority.is_some() {
            priority
        } else if visible
            && (dirty
                || (active
                    && (renderer.samples() < target
                        || deadline.is_some_and(|d| Instant::now() >= d))))
        {
            match receiver.try_recv() {
                Ok(c) => Some(c),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(_) => break,
            }
        } else {
            let timeout = if active {
                deadline
                    .map(|d| {
                        d.saturating_duration_since(Instant::now())
                            .min(Duration::from_millis(250))
                    })
                    .unwrap_or(Duration::from_millis(250))
            } else {
                Duration::from_millis(250)
            };
            match receiver.recv_timeout(timeout) {
                Ok(c) => Some(c),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(_) => break,
            }
        };
        if let Some(command) = command {
            let present_now = command_needs_immediate_present(&command);
            match command {
                Command::Shutdown => break,
                Command::RetainFrame(reply) => {
                    let result=(|| -> Result<_,String> {
                        if !has_map {return Err("没有可保留的地形画面".into());}
                        let mut actual=RECT::default();let mut backing=RECT::default();
                        GetClientRect(GetParent(context.window).map_err(|e|e.to_string())?,&mut actual).map_err(|e|e.to_string())?;
                        GetClientRect(context.window,&mut backing).map_err(|e|e.to_string())?;
                        if actual.right<=0||actual.bottom<=0 {return Err("视口不可见".into());}
                        let (w,h)=(actual.right as u32,actual.bottom as u32);
                        let backing_height=backing.bottom.max(actual.bottom) as u32;
                        renderer.present(w,h,backing_height,&local_overlays(&rect))?;
                        Ok((renderer.retain_presented_frame(w,h,backing_height)?,w,h))
                    })();
                    let _=reply.send(result);
                }
                Command::EndResize(epoch,parent) => {
                    resize_ready=Some((epoch,parent));
                    dirty=true;repaint=true;
                }
                Command::Repaint => {
                    dirty = true;
                    repaint = true;
                }
                Command::Texture(texture, reply) => {
                    let result = renderer.upload_color(&texture);
                    if result.is_ok() {
                        current_texture=Some(texture.clone());
                        let mut s = status.lock().unwrap_or_else(|e| e.into_inner());
                        s.texture_name = Some(texture.name);
                        s.texture_width = texture.width;
                        s.texture_height = texture.height;
                        s.samples = 0;
                        dirty = true;
                    }
                    let _ = reply.send(result);
                }
                Command::ClearTexture(reply) => {
                    let result = renderer.clear_color();
                    {
                        let mut s = status.lock().unwrap_or_else(|e| e.into_inner());
                        commit_texture_removal(&result, &mut desired, &mut s);
                    }
                    if result.is_ok() {
                        current_texture=None;
                        if paused {
                            renderer.render_sample()?;
                        }
                        parameter_status(status, &desired, preview, &renderer);
                        dirty = true;
                    }
                    let _ = reply.send(result);
                }
                Command::TexturePreview(reply) => {
                    let result=current_texture.as_ref().map(|texture|(texture.rgba.clone(),texture.width,texture.height)).ok_or_else(||"尚未导入贴图".to_string());
                    let _=reply.send(result);
                }
                Command::AppearanceDistribution(reply) => {
                    let result=current_preview.as_ref().ok_or_else(||"尚无高度图".to_string())
                        .and_then(super::appearance_distribution::distribution);
                    let _=reply.send(result);
                }
                Command::SampleAppearance(x,y,reply) => {
                    // This barrier is queued after the pick-enabled parameter snapshot.
                    // Finish the small pending interactive frame before reading its metadata.
                    let result=(|| {
                        if !has_map {return Err("尚无高度图".to_string());}
                        renderer.set_params(&preview_params(&desired))?;
                        deferred_preview=false;
                        preview=true;
                        target=1;
                        deadline=Some(Instant::now()+Duration::from_millis(250));
                        dirty=true;
                        while renderer.samples()==0 || renderer.sample_pending() {renderer.render_sample()?;}
                        renderer.sample_appearance(x,y)
                    })();
                    let _=reply.send(result);
                }
                Command::Rect(new_rect) => {
                    rect = new_rect;
                    dirty = true;
                    repaint = true;
                }
                Command::Pause(value) => {
                    paused = value;
                    status.lock().unwrap_or_else(|e| e.into_inner()).paused = value;
                }
                Command::Params(params) => {
                    let (params, barrier) = latest_params(params, &receiver);
                    pending = barrier;
                    if super::gl_renderer::same_scene_without_output_filter(&desired,&params) && params!=desired {
                        desired=params;
                        renderer.set_display_params(&desired)?;
                        parameter_status(status,&desired,preview,&renderer);
                        dirty=true;
                        continue;
                    }
                    target = params.samples.max(1);
                    desired = params.clone();
                    deadline = None;
                    deferred_preview = false;
                    renderer.set_params(&params)?;
                    if preview {
                        renderer.reset_accumulation();
                    }
                    preview = false;
                    parameter_status(status, &desired, preview, &renderer);
                    dirty = true;
                }
                Command::InteractiveParams(params, changed) => {
                    let (params, changed, barrier) =
                        latest_interactive_params(params, changed, &receiver);
                    pending = barrier;
                    if super::gl_renderer::same_scene_without_output_filter(&desired,&params) && params!=desired {
                        desired=params;
                        renderer.set_display_params(&desired)?;
                        parameter_status(status,&desired,preview,&renderer);
                        dirty=true;
                        continue;
                    }
                    // UI flush before pause/export may resend the current scene.
                    // Preserve its completed accumulation and its idle deadline.
                    if params == desired {
                        continue;
                    }
                    desired = params;
                    deadline = Some(changed + Duration::from_millis(250));
                    if preview && renderer.sample_pending() {
                        deferred_preview = true;
                        continue;
                    }
                    deferred_preview = false;
                    let effective = preview_params(&desired);
                    renderer.set_params(&effective)?;
                    if !preview {
                        renderer.reset_accumulation();
                    }
                    preview = true;
                    target = 1;
                    parameter_status(status, &desired, preview, &renderer);
                    dirty = true;
                }
                Command::Map(map) => {
                    status.lock().unwrap_or_else(|e|e.into_inner()).frame_presented=false;
                    if deferred_preview {
                        renderer.set_params(&preview_params(&desired))?;
                        deferred_preview = false;
                    }
                    let (name, width, height) = (map.name.clone(), map.width, map.height);
                    renderer.upload_map(&map)?;
                    if let Some(scale)=super::initial_height_scale(&map) {
                        desired.height_scale=scale;
                        desired.height_scale_mode="linear".into();
                        desired.height_curve=1.;
                        renderer.set_params(&if preview {preview_params(&desired)} else {desired.clone()})?;
                    }
                    has_map = true;
                    dirty = true;
                    let mut s = status.lock().unwrap_or_else(|e| e.into_inner());
                    s.map_name = name;
                    s.map_width = width;
                    s.map_height = height;
                    s.source = map.source.clone();
                    s.samples = if preview { 0 } else { renderer.samples() };
                    current_preview = Some(map);
                }
                Command::MapReady(reply) => {
                    let _ = reply.send(status.lock().unwrap_or_else(|e|e.into_inner()).clone());
                }
                Command::ExportPreviewResult(reply) => {
                    let _ = reply.send(export_result.clone().ok_or_else(|| "尚无成功导出结果".to_string()));
                }
                Command::Capture(reply) => {
                    let result = if has_map && renderer.samples() > 0 {
                        renderer.pixels().map(|pixels| {
                            let (width, height) = renderer.dimensions();
                            (pixels, width, height)
                        })
                    } else {
                        Err("尚无已完成的渲染样本".into())
                    };
                    let _ = reply.send(result);
                }
                Command::CaptureWithMetadata(reply) => {
                    let result = (|| {
                        if !has_map { return Err("尚无高度图".to_string()); }
                        let pixels = renderer.pixels()?;
                        let mut metadata = renderer.output_identity().ok_or_else(|| "尚无已完成的渲染画面".to_string())?;
                        let captured_params:RenderParams = serde_json::from_value(metadata["params"].clone()).map_err(|e|e.to_string())?;
                        // Reset keeps old output pixels; deferred interaction can also leave the
                        // renderer on an older camera. Neither is a current-frame capture.
                        metadata["stale"] = serde_json::json!(capture_identity_is_stale(
                            metadata["contentGeneration"].as_u64(), renderer.content_generation(), &captured_params, &desired));
                        metadata["submittedTargetSamples"] = serde_json::json!(desired.samples);
                        metadata["submittedParams"] = serde_json::to_value(&desired).map_err(|e|e.to_string())?;
                        metadata["currentContentGeneration"] = serde_json::json!(renderer.content_generation());
                        metadata["captureKind"] = serde_json::json!("viewport-output-fbo");
                        metadata["frameGeneration"] = metadata["contentGeneration"].clone();
                        metadata["frameSequence"] = metadata["outputGeneration"].clone();
                        metadata["appliedGeneration"] = serde_json::json!(renderer.content_generation());
                        metadata["parameters"] = metadata["params"].clone();
                        metadata["status"] = serde_json::json!({
                            "ready":true, "samples":metadata["samples"], "targetSamples":metadata["targetSamples"],
                            "width":metadata["params"]["width"], "height":metadata["params"]["height"],
                            "source":metadata["source"], "stale":metadata["stale"]
                        });
                        let (width,height) = (captured_params.width,captured_params.height);
                        Ok((pixels,width,height,metadata))
                    })();
                    let _ = reply.send(result);
                }
                Command::CaptureOutput(params, preserve_grid, reply) => {
                    {
                        let mut state = status.lock().unwrap_or_else(|e| e.into_inner());
                        state.exporting = true;
                        state.export_progress = 0;
                        state.export_work_progress = None;
                    }
                    let result = (|| -> Result<_, String> {
                        if !has_map {
                            return Err("尚无高度图".into());
                        }
                        let mut params = export_params(&desired, params);
                        params.output_bit_depth = 8;
                        if !preserve_grid { params.grid = false; }
                        renderer.set_params(&params)?;
                        // Editing aids never enter the output. Finish every requested sample.
                        while renderer.samples() < params.samples.max(1) {
                            if export_cancel.load(Ordering::Acquire) {
                                return Err("已取消 PNG 导出".into());
                            }
                            renderer.render_export_sample_step()?;
                            let mut state = status.lock().unwrap_or_else(|e| e.into_inner());
                            state.export_work_progress=Some(f64::from(renderer.samples()) * 100. / f64::from(params.samples.max(1)));
                            state.export_progress =
                                (renderer.samples() * 100 / params.samples.max(1)).min(99);
                        }
                        let (width, height) = renderer.dimensions();
                        Ok((renderer.export_pixels()?, width, height))
                    })();
                    let restored = (|| -> Result<(), String> {
                        renderer.clear_frozen_stretch();
                        renderer.set_params(&desired)?;
                        // A paused viewport still needs a visible restored frame.
                        if renderer.samples() == 0 {
                            renderer.render_sample()?;
                        }
                        Ok(())
                    })();
                    preview = false;
                    deferred_preview = false;
                    deadline = None;
                    target = desired.samples.max(1);
                    dirty = true;
                    parameter_status(status, &desired, preview, &renderer);
                    let result = match (result, restored) {
                        (Ok(frame), Ok(())) => Ok(frame),
                        (Err(error), Ok(())) => Err(error),
                        (_, Err(error)) => Err(format!("恢复视图失败：{error}")),
                    };
                    {
                        let mut state = status.lock().unwrap_or_else(|e| e.into_inner());
                        state.exporting = false;
                        state.export_work_progress = None;
                        if result.is_ok() {
                            state.export_progress = 100;
                        }
                    }
                    let _ = reply.send(result);
                }
                Command::ExportMesh(path,params,reply)=>{
                    {let mut state=status.lock().unwrap_or_else(|e|e.into_inner());state.exporting=true;state.export_progress=0;state.export_work_progress=None;}
                    let result=(||{
                        let preview=current_preview.as_ref().ok_or("尚无高度图")?;
                        let map=super::mesh_source::prepare(preview,&params,export_cancel,|v|{status.lock().unwrap_or_else(|e|e.into_inner()).export_progress=v;})?;
                        super::mesh_export::export(&path,&map,&params,current_texture.as_ref(),|v|{
                            if export_cancel.load(Ordering::Acquire){return Err("已取消导出".into())}
                            status.lock().unwrap_or_else(|e|e.into_inner()).export_progress=15+v*85/100;Ok(())
                        })
                    })();
                    {let mut state=status.lock().unwrap_or_else(|e|e.into_inner());state.exporting=false;state.export_work_progress=None;if result.is_ok(){state.export_progress=100;}}
                    let _=reply.send(result);
                }
                Command::ExportPng(path, params, reply) => {
                    export_result = None;
                    {
                        let mut state = status.lock().unwrap_or_else(|e| e.into_inner());
                        state.exporting = true;
                        state.export_progress = 0;
                        state.export_work_progress = None;
                    }
                    let params = export_params(&desired, params);
                    let result = match current_preview.as_ref() {
                        Some(map) => export_camera_png(&mut renderer, map, &params, &path, status, export_cancel),
                        None => Err("尚无高度图".into()),
                    };
                    let restored = (|| -> Result<(), String> {
                        renderer.clear_frozen_stretch();
                        renderer.set_output_tile(None)?;
                        renderer.set_geo_tile(None)?;
                        if let Some(map) = current_preview.as_ref() { renderer.upload_map(map)?; }
                        renderer.set_params(&desired)?;
                        renderer.reset_accumulation();
                        renderer.render_sample()?;
                        Ok(())
                    })();
                    preview = false;
                    deferred_preview = false;
                    deadline = None;
                    target = desired.samples.max(1);
                    dirty = true;
                    parameter_status(status, &desired, preview, &renderer);
                    let result = match (result, restored) {
                        (Ok(frame), Ok(())) => { export_result = Some(frame); Ok(()) },
                        (Err(error), Ok(())) => Err(error),
                        (Ok(_), Err(e)) => Err(format!("图像已写出，但恢复预览失败：{e}")),
                        (Err(e), Err(restore)) => Err(format!("{e}；恢复预览失败：{restore}")),
                    };
                    {
                        let mut state = status.lock().unwrap_or_else(|e| e.into_inner());
                        state.exporting = false;
                        state.export_work_progress = None;
                        if result.is_ok() {
                            state.export_progress = 100;
                        }
                    }
                    let _ = reply.send(result);
                }
                Command::PreviewOutput(orthographic, params, reply) => {
                    let result = (|| -> Result<_, String> {
                        let map = current_preview.as_ref().ok_or("尚无高度图")?;
                        let mut snapshot = export_params(&desired, params);
                        snapshot.export_orthographic |= orthographic;
                        if snapshot.export_orthographic {snapshot.projection="orthographic".into();}
                        let base_size=[snapshot.width,snapshot.height];
                        let extent=export_image_extent(&snapshot)?;
                        let output_size=[extent.width,extent.height];
                        let source_framing=snapshot.export_orthographic;
                        let params = export_preview_params(
                            &desired, Some(snapshot), Some((extent.width,extent.height)).filter(|_|source_framing),
                        );
                        renderer.set_output_tile(None)?;
                        renderer.set_params(&params)?;
                        if source_framing {
                            renderer.set_geo_grid(Some([base_size[0],base_size[1],extent.left,extent.top]))?;
                            renderer.set_geo_tile(Some([0,0,extent.width,extent.height]))?;
                        } else {renderer.set_geo_tile(None)?;}
                        freeze_export_stretch(
                            &mut renderer, &params, output_size, source_framing, &AtomicBool::new(false),
                        )?;
                        renderer.set_params(&params)?;
                        renderer.reset_accumulation();
                        while renderer.samples() < 4 {
                            renderer.render_export_sample_step()?;
                        }
                        Ok((renderer.export_pixels()?, params.width, params.height))
                    })();
                    let restored = (|| -> Result<(), String> {
                        renderer.clear_frozen_stretch();
                        renderer.set_geo_tile(None)?;
                        renderer.set_output_tile(None)?;
                        renderer.set_params(&desired)?;
                        renderer.reset_accumulation();
                        // Preserve the paused flag while restoring its visible image.
                        renderer.render_sample()?;
                        Ok(())
                    })();
                    preview = false;
                    deferred_preview = false;
                    deadline = None;
                    target = desired.samples.max(1);
                    dirty = true;
                    parameter_status(status, &desired, preview, &renderer);
                    let result = match (result, restored) {
                        (Ok(frame), Ok(())) => Ok(frame),
                        (Err(error), Ok(())) => Err(error),
                        (_, Err(error)) => Err(format!("恢复视图失败：{error}")),
                    };
                    let _ = reply.send(result);
                }
                Command::ExportGeo(path, params, reply) => {
                    export_result = None;
                    {
                        let mut state = status.lock().unwrap_or_else(|e| e.into_inner());
                        state.exporting = true;
                        state.export_progress = 0;
                        state.export_work_progress = None;
                    }
                    let export = export_params(&desired, params);
                    let result = match current_preview.as_ref() {
                        Some(map) => export_source_grid(
                            &mut renderer,
                            map,
                            &export,
                            &path,
                            status,
                            export_cancel,
                        ),
                        None => Err("尚无高度图".into()),
                    };
                    // The export may have changed texture, shader, dimensions and
                    // accumulation. Restore even on decoder/GPU/cancel failures.
                    let restored = (|| -> Result<(), String> {
                        renderer.clear_frozen_stretch();
                        renderer.set_geo_tile(None)?;
                        renderer.set_output_tile(None)?;
                        if let Some(map) = current_preview.as_ref() {
                            renderer.upload_map(map)?;
                        }
                        renderer.set_params(&desired)?;
                        renderer.reset_accumulation();
                        if renderer.samples() == 0 {
                            renderer.render_sample()?;
                        }
                        Ok(())
                    })();
                    preview = false;
                    deferred_preview = false;
                    deadline = None;
                    target = desired.samples.max(1);
                    dirty = true;
                    parameter_status(status, &desired, preview, &renderer);
                    let result = match (result, restored) {
                        (Ok(frame), Ok(())) => { export_result = Some(frame); Ok(()) },
                        (Err(error), Ok(())) => Err(error),
                        (Ok(_), Err(error)) => {
                            Err(format!("TIFF 已写出，但恢复预览失败：{error}"))
                        }
                        (Err(error), Err(restore)) => {
                            Err(format!("{error}；恢复预览失败：{restore}"))
                        }
                    };
                    {
                        let mut state = status.lock().unwrap_or_else(|e| e.into_inner());
                        state.exporting = false;
                        state.export_work_progress = None;
                        if result.is_ok() {
                            state.export_progress = 100;
                        }
                    }
                    let _ = reply.send(result);
                }
            }
            // Native sizing must not wait for a gap in the IPC message stream.
            // Reuse the completed output before handling the next queued event.
            if !present_now {
                continue;
            }
        }
        if !visible {
            continue;
        }
        if active
            && !repaint
            && deadline.is_some_and(|d| Instant::now() >= d)
            && preview_can_refine(preview, renderer.samples(), renderer.sample_pending())
        {
            renderer.set_params(&desired)?;
            renderer.reset_accumulation();
            preview = false;
            deferred_preview = false;
            deadline = None;
            target = desired.samples.max(1);
            parameter_status(status, &desired, preview, &renderer);
            dirty = true;
        }
        // DWM may discard a WGL backing during restore/resize without sending
        // child WM_PAINT. Refresh only the last complete GPU output; this is
        // independent of sampling/JS and performs no CPU readback.
        repaint |=
            (paused || renderer.samples() >= target) && last_present.elapsed() >= PRESENT_REFRESH;
        match frame_work(
            visible,
            paused,
            renderer.samples() < target,
            dirty,
            repaint,
            renderer.samples() > 0,
        ) {
            FrameWork::None => continue,
            FrameWork::Present => {}
            FrameWork::Sample => {
                if !renderer.render_sample_step()? {
                    // Keep the last complete presented sample while a strip is in
                    // flight. Check queued camera/pause/export commands next turn.
                    continue;
                }
            }
        }
        let presented = present_with_repaint_gate(repaint_queued, || -> Result<bool, String> {
            let mut actual = RECT::default();
            GetClientRect(GetParent(context.window).map_err(|e| e.to_string())?, &mut actual)
                .map_err(|e| format!("读取实际视口尺寸失败: {e}"))?;
            if actual.right <= 0 || actual.bottom <= 0 {
                return Ok(false);
            }
            let mut backing = RECT::default();
            GetClientRect(context.window, &mut backing).map_err(|e| e.to_string())?;
            renderer.present(
                actual.right as u32,
                actual.bottom as u32,
                backing.bottom.max(actual.bottom) as u32,
                &local_overlays(&rect),
            )?;
            SwapBuffers(context.dc).map_err(|e| format!("交换视口缓冲失败: {e}"))?;
            Ok(true)
        })?;
        if !presented {
            continue;
        }
        last_present = Instant::now();
        status.lock().unwrap_or_else(|e|e.into_inner()).frame_presented=true;
        if let Some((epoch,parent))=resize_ready.take() {
            // Retain the GDI cover until the newly sized WGL frame has actually
            // reached the compositor, not just the render queue.
            let _=DwmFlush();
            let _=PostMessageW(Some(HWND(parent as *mut c_void)),RESIZE_FRAME_READY,
                WPARAM(epoch),LPARAM(context.window.0 as isize));
        }
        dirty = false;
        repaint = false;
        parameter_status(status, &desired, preview, &renderer);
        if deferred_preview && renderer.samples() >= target {
            renderer.set_params(&preview_params(&desired))?;
            deferred_preview = false;
            dirty = true;
        }
    }
    // Renderer drops while the context remains current; guard releases WGL afterwards.
    drop(renderer);
    Ok(())
}
