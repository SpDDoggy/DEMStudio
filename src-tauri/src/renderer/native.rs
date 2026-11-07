//! Windows child viewport. All GL state stays on one dedicated render thread.
use super::{
    appearance::ColorTexture, gl_renderer::GlRenderer, HeightMap, OverlayRect, RenderParams,
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
        Graphics::{Gdi::*, OpenGL::*},
        System::LibraryLoader::*,
        UI::{
            Input::KeyboardAndMouse::{ReleaseCapture, SetCapture},
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
    Rect(ViewportRect),
    Params(RenderParams),
    InteractiveParams(RenderParams, Instant),
    Map(HeightMap),
    Texture(ColorTexture, mpsc::SyncSender<Result<(), String>>),
    ClearTexture(mpsc::SyncSender<Result<(), String>>),
    Pause(bool),
    Capture(mpsc::SyncSender<Result<(Vec<u8>, u32, u32), String>>),
    ExportPreviewResult(mpsc::SyncSender<Result<ExportFrame, String>>),
    CaptureOutput(
        Option<RenderParams>,
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
    Shutdown,
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

/// Bound the dense terrain texture independently of the export's pixel grid.
/// The source grid still determines exact output pixel centres and georeference.
fn export_geometry_edge(width: u32, height: u32, gpu_limit: u32) -> u32 {
    // Preserve the existing full-resolution path for original supported sources.
    // The bounded large-source path must not silently reduce their terrain detail.
    if width.max(height) <= super::raster::MAX_DECODE_EDGE
        && width.max(height).next_power_of_two() <= gpu_limit
    {
        return width.max(height).max(2);
    }
    let mut edge = width.max(height).min(8192).min(gpu_limit).max(2);
    let ratio = (edge as f64 / width.max(height).max(1) as f64).min(1.);
    let w = (width as f64 * ratio).round().max(2.) as u64;
    let h = (height as f64 * ratio).round().max(2.) as u64;
    if w * h * 4 > 128 * 1024 * 1024 {
        edge = edge.min(4096);
    }
    edge
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
        desired.appearance_mode = "material".into();
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
            let showing = IsWindowVisible(window).as_bool();
            let at_bottom = GetWindow(window, GW_HWNDNEXT).is_err();
            if viewport_position_changed(
                [current.right, current.bottom],
                [rect.width as i32, rect.height as i32],
                showing,
                rect.visible,
                at_bottom,
            ) {
                SetWindowPos(
                    window,
                    Some(HWND_BOTTOM),
                    rect.x,
                    rect.y,
                    rect.width.min(i32::MAX as u32) as i32,
                    rect.height.min(i32::MAX as u32) as i32,
                    flags,
                )
                .map_err(|e| format!("定位原生视口失败: {e}"))?;
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
            let rounded = region_type == COMPLEXREGION
                && !PtInRegion(region, 0, 0).as_bool()
                && !PtInRegion(region, width - 1, 0).as_bool()
                && !PtInRegion(region, 0, height - 1).as_bool()
                && !PtInRegion(region, width - 1, height - 1).as_bool()
                && PtInRegion(region, width / 2, height / 2).as_bool();
            let _ = DeleteObject(region.into());
            let mut client = RECT::default();
            GetClientRect(window, &mut client).map_err(|e| e.to_string())?;
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
                    "maximized":IsZoomed(parent).as_bool(),
                    "outerRect":[bounds.left,bounds.top,bounds.right,bounds.bottom],
                    "viewportClientSize":[client.right,client.bottom],
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
            .send(Command::CaptureOutput(params, tx))
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

// Clip the entire HWND, including WGL and WebView descendants. Share one rule
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
    let region = if rounded {
        CreateRoundRectRgn(0, 0, width + 1, height + 1, 24, 24)
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
unsafe fn refresh_native_viewport(parent: HWND, viewport: HWND) {
    if viewport.is_invalid() || IsIconic(parent).as_bool() {
        return;
    }
    let mut client = RECT::default();
    let mut current = RECT::default();
    if GetClientRect(parent, &mut client).is_ok()
        && GetClientRect(viewport, &mut current).is_ok()
        && (client.right != current.right || client.bottom != current.bottom)
        && client.right > 0
        && client.bottom > 0
    {
        let _ = SetWindowPos(
            viewport,
            Some(HWND_BOTTOM),
            0,
            0,
            client.right,
            client.bottom,
            SWP_NOACTIVATE,
        );
    }
    request_repaint(viewport);
}

unsafe fn request_repaint(viewport: HWND) {
    let input = GetWindowLongPtrW(viewport, GWLP_USERDATA) as *mut WindowInput;
    if !input.is_null() && !(*input).repaint_queued.swap(true, Ordering::AcqRel) {
        let _ = (*input).sender.send(Command::Repaint);
    }
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
unsafe extern "system" fn frameless_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    id: usize,
    data: usize,
) -> LRESULT {
    match msg {
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
        WM_ACTIVATE | WM_SIZE | WM_SHOWWINDOW | WM_WINDOWPOSCHANGED | WM_EXITSIZEMOVE
        | WM_DISPLAYCHANGE | WM_DPICHANGED => {
            let result = DefSubclassProc(hwnd, msg, wp, lp);
            if data != 0 {
                refresh_native_viewport(hwnd, HWND(data as *mut c_void));
                let _ = sync_window_shape(hwnd);
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
    last: Option<(i32, i32)>,
    released: Arc<AtomicBool>,
    repaint_queued: Arc<AtomicBool>,
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
        WM_PAINT => {
            if !input.is_null() {
                request_repaint(hwnd);
            }
            let _ = ValidateRect(Some(hwnd), None);
            return LRESULT(0);
        }
        WM_ERASEBKGND => return LRESULT(1),
        WM_RBUTTONDOWN if !input.is_null() => {
            (*input).last = Some((lp.0 as i16 as i32, (lp.0 >> 16) as i16 as i32));
            SetCapture(hwnd);
            return LRESULT(0);
        }
        WM_MOUSEMOVE if !input.is_null() => {
            let pos = (lp.0 as i16 as i32, (lp.0 >> 16) as i16 as i32);
            if let Some(last) = (*input).last {
                if let Some(event) = drag_event_name(wp.0) {
                    let _ = (*input).app.emit(
                        event,
                        Orbit {
                            dx: pos.0 - last.0,
                            dy: pos.1 - last.1,
                        },
                    );
                    (*input).last = Some(pos);
                } else {
                    (*input).last = None;
                    let _ = ReleaseCapture();
                }
            }
            return LRESULT(0);
        }
        WM_RBUTTONUP | WM_CAPTURECHANGED if !input.is_null() => {
            (*input).last = None;
            if msg == WM_RBUTTONUP {
                let _ = ReleaseCapture();
            }
            return LRESULT(0);
        }
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
pub fn start(parent: isize, app: tauri::AppHandle) -> Result<RendererHandle, String> {
    let (sender, receiver) = mpsc::channel();
    let repaint_queued = Arc::new(AtomicBool::new(false));
    let hwnd = unsafe {
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
            last: None,
            released: released.clone(),
            repaint_queued: repaint_queued.clone(),
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

unsafe fn export_source_grid(
    renderer: &mut GlRenderer,
    preview: &HeightMap,
    desired: &RenderParams,
    path: &Path,
    status: &Arc<Mutex<RenderStatus>>,
    cancel: &AtomicBool,
) -> Result<ExportFrame, String> {
    let check_cancel = || {
        if cancel.load(Ordering::Acquire) {
            Err("已取消 TIFF 导出".to_string())
        } else {
            Ok(())
        }
    };
    check_cancel()?;
    let source = preview.source.as_ref().ok_or("当前地形没有源图像")?;
    if source.geo.is_none() {
        return Err("源图像没有地理坐标，无法导出带坐标系的 TIFF".into());
    }
    // Export pixels retain the source grid, while geometry stays within a bounded
    // GPU texture. Very large sources use area-reduced heights, never a dense
    // 81200² heightmap or full-resolution RGBA frame.
    let geometry_edge =
        export_geometry_edge(source.width, source.height, renderer.heightmap_edge_limit());
    let full = super::raster::load(&source.path, geometry_edge)?;
    check_cancel()?;
    let loaded = full.source.as_ref().ok_or("重新读取的源图像没有地理参考")?;
    let same_geo = serde_json::to_value(&source.geo).map_err(|e| e.to_string())?
        == serde_json::to_value(&loaded.geo).map_err(|e| e.to_string())?;
    if loaded.width != source.width || loaded.height != source.height || !same_geo {
        return Err("源图像尺寸或地理参考已改变，请重新导入后导出".into());
    }
    renderer.upload_map(&full)?;
    drop(full);
    check_cancel()?;
    let extent =
        super::export_extent::ExportExtent::for_source(source.width, source.height, desired)?;
    renderer.set_geo_grid(Some([source.width, source.height, extent.left, extent.top]))?;
    let samples = desired.samples.max(1);
    let total = u64::from(extent.width) * u64::from(extent.height) * u64::from(samples);
    let mut completed = 0u64;
    let mut thumbnail = Thumbnail::new(extent.width, extent.height)?;
    freeze_export_stretch(renderer, desired, [extent.width,extent.height], true, cancel)?;
    super::geotiff::write_orthographic_extent(
        path,
        source,
        extent.width,
        extent.height,
        extent.left,
        extent.top,
        |x, y, width, height| {
            check_cancel()?;
            let mut params = desired.clone();
            params.width = width;
            params.height = height;
            params.projection = "orthographic".into();
            params.grid = false;
            renderer.set_params(&params)?;
            renderer.set_geo_tile(Some([x, y, width, height]))?;
            renderer.reset_accumulation();
            let tile_pixels = u64::from(width) * u64::from(height);
            while renderer.samples() < samples {
                check_cancel()?;
                if renderer.render_export_sample_step()? {
                    completed += tile_pixels;
                    let mut state = status.lock().unwrap_or_else(|e| e.into_inner());
                    state.export_progress =
                        (completed.saturating_mul(100) / total.max(1)).min(99) as u32;
                }
            }
            check_cancel()?;
            let pixels = renderer.export_pixels()?;
            thumbnail.add_tile(x, y, width, height, &pixels)?;
            Ok(pixels)
        },
    )?;
    thumbnail.finish()
}

// One bounded, whole-frame statistics pass is frozen for all output tiles.
// This never allocates a source-sized histogram/render buffer.
unsafe fn freeze_export_stretch(
    renderer: &mut GlRenderer, desired: &RenderParams, size: [u32;2], geo: bool, cancel: &AtomicBool,
) -> Result<(),String> {
    renderer.clear_frozen_stretch();
    if desired.stretch_type == "none" { return Ok(()); }
    let scale=256. / size[0].max(size[1]).max(1) as f64;
    let mut pilot=desired.clone();
    pilot.width=(size[0] as f64*scale).round().max(2.) as u32;
    pilot.height=(size[1] as f64*scale).round().max(2.) as u32;
    pilot.samples=4;pilot.grid=false;
    renderer.set_params(&pilot)?;
    if geo { renderer.set_geo_tile(Some([0,0,size[0],size[1]]))?; }
    else { renderer.set_output_tile(None)?; }
    renderer.reset_accumulation();
    renderer.restart_sampling_sequence();
    while renderer.samples() < 4 {
        if cancel.load(Ordering::Acquire){return Err(if geo {"已取消 TIFF 导出"} else {"已取消 PNG 导出"}.into());}
        renderer.render_export_sample_step()?;
    }
    renderer.freeze_stretch_from_current();
    Ok(())
}

/// Orthographic PNG covers the complete source footprint independently of CRS.
fn validate_orthographic_png_size(map: &HeightMap, params: &RenderParams) -> Result<(), String> {
    let (width, height) = map.source.as_ref().map(|source| (source.width, source.height))
        .unwrap_or((map.width, map.height));
    if params.width == 0 || params.height == 0 { return Err("PNG 输出尺寸无效".into()); }
    let expected_height = f64::from(params.width) * f64::from(height) / f64::from(width);
    let expected_width = f64::from(params.height) * f64::from(width) / f64::from(height);
    if (f64::from(params.height) - expected_height).abs() > 0.5
        && (f64::from(params.width) - expected_width).abs() > 0.5 {
        return Err("正射 PNG 尺寸须保持原图比例".into());
    }
    Ok(())
}

unsafe fn export_camera_png(
    renderer: &mut GlRenderer,
    preview: &HeightMap,
    desired: &RenderParams,
    path: &Path,
    status: &Arc<Mutex<RenderStatus>>,
    cancel: &AtomicBool,
) -> Result<ExportFrame, String> {
    if cancel.load(Ordering::Acquire) { return Err("已取消 PNG 导出".into()); }
    if desired.width == 0 || desired.height == 0
        || desired.width > super::png_output::MAX_EDGE || desired.height > super::png_output::MAX_EDGE {
        return Err("PNG 输出尺寸超出范围".into());
    }
    let mut params = desired.clone();
    params.grid = false;
    if params.export_orthographic {
        validate_orthographic_png_size(preview, &params)?;
        // Original footprint only; GeoTIFF shadow expansion stays separate.
        params.export_shadow = false;
        params.projection = "orthographic".into();
        if let Some(source) = &preview.source {
            let edge = export_geometry_edge(source.width, source.height, renderer.heightmap_edge_limit());
            let full = super::raster::load(&source.path, edge)?;
            let loaded = full.source.as_ref().ok_or("重新读取的源图像无效")?;
            if loaded.width != source.width || loaded.height != source.height {
                return Err("源图像尺寸已改变，请重新导入后导出".into());
            }
            if cancel.load(Ordering::Acquire) { return Err("已取消 PNG 导出".into()); }
            renderer.upload_map(&full)?;
        }
        renderer.set_output_tile(None)?;
        renderer.set_geo_grid(Some([params.width, params.height, 0, 0]))?;
    } else {
        renderer.set_geo_tile(None)?;
    }
    let samples = params.samples.max(1);
    let total = u64::from(params.width) * u64::from(params.height) * u64::from(samples);
    let mut completed = 0u64;
    let mut thumbnail = Thumbnail::new(params.width, params.height)?;
    freeze_export_stretch(renderer, &params, [params.width,params.height], params.export_orthographic, cancel)?;
    super::png_output::write(
        path,
        params.width,
        params.height,
        |x, y, width, height| {
            if cancel.load(Ordering::Acquire) { return Err("已取消 PNG 导出".into()); }
            let mut tile = params.clone();
            tile.width = width;
            tile.height = height;
            renderer.set_params(&tile)?;
            if params.export_orthographic {
                renderer.set_geo_tile(Some([x, y, width, height]))?;
                renderer.reset_accumulation();
                renderer.restart_sampling_sequence();
            } else {
                renderer.set_output_tile(Some(([params.width, params.height], [x, y, width, height])))?;
            }
            while renderer.samples() < samples {
                if cancel.load(Ordering::Acquire) { return Err("已取消 PNG 导出".into()); }
                if renderer.render_export_sample_step()? {
                    completed += u64::from(width) * u64::from(height);
                    status.lock().unwrap_or_else(|e| e.into_inner()).export_progress =
                        (completed.saturating_mul(100) / total.max(1)).min(99) as u32;
                }
            }
            let pixels = renderer.export_pixels()?;
            thumbnail.add_tile(x, y, width, height, &pixels)?;
            Ok(pixels)
        },
    )?;
    thumbnail.finish()
}

unsafe fn render_thread(
    window: isize,
    receiver: mpsc::Receiver<Command>,
    status: &Arc<Mutex<RenderStatus>>,
    export_cancel: &AtomicBool,
    repaint_queued: &AtomicBool,
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
        let command = if pending.is_some() {
            pending.take()
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
            match command {
                Command::Shutdown => break,
                Command::Repaint => {
                    dirty = true;
                    repaint = true;
                }
                Command::Texture(texture, reply) => {
                    let result = renderer.upload_color(&texture);
                    if result.is_ok() {
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
                        if paused {
                            renderer.render_sample()?;
                        }
                        parameter_status(status, &desired, preview, &renderer);
                        dirty = true;
                    }
                    let _ = reply.send(result);
                }
                Command::Rect(new_rect) => {
                    rect = new_rect;
                    dirty = true;
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
                    if deferred_preview {
                        renderer.set_params(&preview_params(&desired))?;
                        deferred_preview = false;
                    }
                    let (name, width, height) = (map.name.clone(), map.width, map.height);
                    renderer.upload_map(&map)?;
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
                Command::CaptureOutput(params, reply) => {
                    {
                        let mut state = status.lock().unwrap_or_else(|e| e.into_inner());
                        state.exporting = true;
                        state.export_progress = 0;
                    }
                    let result = (|| -> Result<_, String> {
                        if !has_map {
                            return Err("尚无高度图".into());
                        }
                        let mut params = export_params(&desired, params);
                        params.grid = false;
                        renderer.set_params(&params)?;
                        // Editing aids never enter the output. Finish every requested sample.
                        while renderer.samples() < params.samples.max(1) {
                            if export_cancel.load(Ordering::Acquire) {
                                return Err("已取消 PNG 导出".into());
                            }
                            renderer.render_export_sample_step()?;
                            let mut state = status.lock().unwrap_or_else(|e| e.into_inner());
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
                        if result.is_ok() {
                            state.export_progress = 100;
                        }
                    }
                    let _ = reply.send(result);
                }
                Command::ExportPng(path, params, reply) => {
                    export_result = None;
                    {
                        let mut state = status.lock().unwrap_or_else(|e| e.into_inner());
                        state.exporting = true;
                        state.export_progress = 0;
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
                        if params.export_orthographic {
                            if let Some(map) = current_preview.as_ref() { renderer.upload_map(map)?; }
                        }
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
                        (Ok(_), Err(e)) => Err(format!("PNG 已写出，但恢复预览失败：{e}")),
                        (Err(e), Err(restore)) => Err(format!("{e}；恢复预览失败：{restore}")),
                    };
                    {
                        let mut state = status.lock().unwrap_or_else(|e| e.into_inner());
                        state.exporting = false;
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
                        let png_orthographic = !orthographic && snapshot.export_orthographic;
                        if png_orthographic {
                            validate_orthographic_png_size(map, &snapshot)?;
                            snapshot.export_shadow = false;
                            snapshot.projection = "orthographic".into();
                        }
                        let source_size = map.source.as_ref().map(|source| [source.width, source.height])
                            .unwrap_or([map.width, map.height]);
                        let extent = if orthographic {
                            Some(if map.source.is_some() {
                                super::export_extent::ExportExtent::for_source(source_size[0], source_size[1], &snapshot)?
                            } else {
                                super::export_extent::shadow_extent(map, &snapshot)?
                            })
                        } else { None };
                        let output_size = extent.map(|extent| [extent.width, extent.height])
                            .unwrap_or([snapshot.width, snapshot.height]);
                        let params = export_preview_params(
                            &desired,
                            Some(snapshot),
                            extent.map(|extent| (extent.width, extent.height)),
                        );
                        renderer.set_output_tile(None)?;
                        renderer.set_params(&params)?;
                        if let Some(extent) = extent {
                            renderer.set_geo_grid(Some([source_size[0], source_size[1], extent.left, extent.top]))?;
                            renderer.set_geo_tile(Some([0, 0, extent.width, extent.height]))?;
                        } else if png_orthographic {
                            renderer.set_geo_grid(Some([output_size[0], output_size[1], 0, 0]))?;
                            renderer.set_geo_tile(Some([0, 0, output_size[0], output_size[1]]))?;
                        } else {
                            renderer.set_geo_tile(None)?;
                        }
                        freeze_export_stretch(
                            &mut renderer, &params, output_size, orthographic || png_orthographic, &AtomicBool::new(false),
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
                        if result.is_ok() {
                            state.export_progress = 100;
                        }
                    }
                    let _ = reply.send(result);
                }
            }
            continue;
        }
        if !visible {
            continue;
        }
        if active
            && deadline.is_some_and(|d| Instant::now() >= d)
            && !(deferred_preview && renderer.sample_pending())
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
        let mut actual = RECT::default();
        GetClientRect(context.window, &mut actual)
            .map_err(|e| format!("读取实际视口尺寸失败: {e}"))?;
        if actual.right <= 0 || actual.bottom <= 0 {
            continue;
        }
        renderer.present(
            actual.right as u32,
            actual.bottom as u32,
            &local_overlays(&rect),
        )?;
        SwapBuffers(context.dc).map_err(|e| format!("交换视口缓冲失败: {e}"))?;
        last_present = Instant::now();
        // One native repaint may remain queued until a swap succeeds; otherwise
        // a WM_WINDOWPOS/WM_PAINT burst can starve the worker's presentation.
        repaint_queued.store(false, Ordering::Release);
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


