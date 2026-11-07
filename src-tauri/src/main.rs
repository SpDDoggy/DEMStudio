#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod renderer;
use renderer::{native::RendererHandle, RenderParams, RenderStatus, ViewportRect};
use serde::Serialize;
use std::{
    path::PathBuf,
    sync::Mutex,
};
use tauri::Manager;
#[derive(Default)]
struct Runtime(Mutex<Option<RendererHandle>>);
#[derive(Default)]
struct ExportResultPath(Mutex<Option<PathBuf>>);

fn handle(state: &Runtime) -> Result<RendererHandle, String> {
    state
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .clone()
        .ok_or("渲染器尚未初始化".into())
}
fn validate(p: &RenderParams) -> Result<(), String> {
    for value in [p.texture_exposure, p.scene_exposure] {
        if !value.is_finite() || !(-4. ..=4.).contains(&value) {
            return Err("曝光应在-4至4 EV之间".into());
        }
    }
    if !p.scene_gamma.is_finite() || !(0.1..=4.).contains(&p.scene_gamma) {
        return Err("场景Gamma应在0.1至4之间".into());
    }
    for value in [p.texture_contrast, p.texture_shadows, p.texture_highlights, p.texture_saturation,
        p.scene_brightness, p.scene_contrast, p.scene_saturation, p.scene_temperature,
        p.scene_shadows, p.scene_highlights] {
        if !value.is_finite() || !(-100. ..=100.).contains(&value) {
            return Err("调色参数应在-100至100之间".into());
        }
    }
    if !["none", "minmax", "stddev", "equalize", "percentclip"].contains(&p.stretch_type.as_str())
        || !["normal", "color"].contains(&p.elevation_blend.as_str())
        || !p.stretch_deviations.is_finite() || !(0.1..=10.).contains(&p.stretch_deviations)
        || !p.stretch_low.is_finite() || !(0. ..=49.).contains(&p.stretch_low)
        || !p.stretch_high.is_finite() || !(0. ..=49.).contains(&p.stretch_high)
    { return Err("无效的拉伸或混合参数".into()); }

    let numbers = [
        p.sun_altitude,
        p.sun_azimuth,
        p.sun_size,
        p.sun_intensity,
        p.sky_intensity,
        p.fog_density,
        p.camera_yaw,
        p.camera_pitch,
        p.camera_distance,
        p.field_of_view,
        p.exposure,
        p.gamma,
        p.bloom,
        p.height_scale,
        p.height_offset,
        p.height_step,
    ];
    if numbers
        .iter()
        .chain(p.camera_pan.iter())
        .any(|v| !v.is_finite())
        || p.width < 16
        || p.height < 16
        || p.width > 4096
        || p.height > 4096
        || p.samples < 1
        || p.samples > 65536
        || p.gamma <= 0.
        || p.camera_distance <= 0.
        || !(0. ..=90.).contains(&p.camera_pitch)
        || p.field_of_view <= 0.
        || p.field_of_view >= 175.
        || p.height_scale <= 0.
        || !(0. ..=100.).contains(&p.height_offset)
        || p.sun_size <= 0.
        || !(0. ..=90.).contains(&p.sun_altitude)
        || p.height_step < 0.
        || p.sun_intensity < 0.
        || p.sky_intensity < 0.
        || p.exposure < 0.
        || !p.shadow_padding.is_finite()
        || !(1. ..=200.).contains(&p.shadow_padding)
    {
        return Err("渲染参数超出有效范围".into());
    }
    for adjustment in [
        p.output_contrast,
        p.output_saturation,
        p.output_temperature,
        p.output_shadows,
        p.output_highlights,
    ] {
        if !adjustment.is_finite() || !(-100. ..=100.).contains(&adjustment) {
            return Err("导出后处理参数应在-100至100之间".into());
        }
    }
    if p.fog_density != 0. || p.bloom != 0. {
        return Err("当前基线尚未启用雾和泛光".into());
    }
    if !["cube", "poly", "bilinear", "bicubic"].contains(&p.surface.as_str())
        || !["perspective", "orthographic", "isometric"].contains(&p.projection.as_str())
    {
        return Err("无效的表面或投影类型".into());
    }
    for opacity in [p.material_opacity, p.texture_opacity, p.elevation_opacity] {
        if !opacity.is_finite() || !(0. ..=100.).contains(&opacity) {
            return Err("不透明度应在0至100之间".into());
        }
    }
    for c in [&p.sun_color, &p.sky_color, &p.material_color]
        .into_iter()
        .chain(p.ramp_colors.iter())
    {
        if c.len() != 7
            || !c.starts_with('#')
            || !c.as_bytes()[1..].iter().all(|b| b.is_ascii_hexdigit())
        {
            return Err("颜色格式无效".into());
        }
    }
    if !["material", "texture", "elevation"].contains(&p.appearance_mode.as_str())
        || !["terrain", "grayscale", "spectral", "custom"].contains(&p.color_ramp.as_str())
        || !["continuous", "discrete"].contains(&p.ramp_interpolation.as_str())
    {
        return Err("无效的外观模式或高程色带".into());
    }
    Ok(())
}
fn validate_output(p: &RenderParams) -> Result<(), String> {
    if p.width == 0
        || p.height == 0
        || p.width > renderer::png_output::MAX_EDGE
        || p.height > renderer::png_output::MAX_EDGE
    {
        return Err(format!(
            "导出尺寸应在 1 至 {} 之间",
            renderer::png_output::MAX_EDGE
        ));
    }
    let mut scene = p.clone();
    scene.width = scene.width.clamp(16, 4096);
    scene.height = scene.height.clamp(16, 4096);
    validate(&scene)
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AppInfo {
    name: &'static str,
    version: &'static str,
    renderer_ready: bool,
}
#[tauri::command]
fn app_info(state: tauri::State<Runtime>) -> AppInfo {
    AppInfo {
        name: "DEMStudio",
        version: env!("CARGO_PKG_VERSION"),
        renderer_ready: handle(&state).is_ok_and(|h| h.status().ready),
    }
}
#[tauri::command]
fn renderer_init(
    rect: ViewportRect,
    params: RenderParams,
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<Runtime>,
) -> Result<RenderStatus, String> {
    validate(&params)?;
    let mut slot = state.0.lock().map_err(|e| e.to_string())?;
    if slot.is_none() {
        let h = renderer::native::start(window.hwnd().map_err(|e| e.to_string())?.0 as isize, app)?;
        h.set_map(renderer::heightmap::demo())?;
        h.set_params(params)?;
        h.resize(rect)?;
        *slot = Some(h);
    }
    Ok(slot.as_ref().unwrap().status())
}
#[tauri::command]
fn renderer_resize(rect: ViewportRect, state: tauri::State<Runtime>) -> Result<(), String> {
    handle(&state)?.resize(rect)
}
#[tauri::command]
fn renderer_update(params: RenderParams, state: tauri::State<Runtime>) -> Result<(), String> {
    validate(&params)?;
    handle(&state)?.set_interactive_params(params)
}
#[tauri::command]
fn renderer_pause(paused: bool, state: tauri::State<Runtime>) -> Result<(), String> {
    handle(&state)?.pause(paused)
}
#[tauri::command]
fn renderer_status(state: tauri::State<Runtime>) -> Result<RenderStatus, String> {
    Ok(handle(&state)?.status())
}
#[tauri::command]
async fn renderer_import(state: tauri::State<'_, Runtime>) -> Result<Option<RenderStatus>, String> {
    let h = handle(&state)?;
    let Some(file) = rfd::AsyncFileDialog::new()
        .add_filter("Heightmap", &["jpg", "jpeg", "png", "tif", "tiff", "asc"])
        .pick_file()
        .await
    else {
        return Ok(None);
    };
    let path = file.path().to_owned();
    let map = tauri::async_runtime::spawn_blocking(move || renderer::heightmap::load(&path))
        .await
        .map_err(|e| e.to_string())??;
    h.set_map(map)?;
    Ok(Some(h.status()))
}
#[tauri::command]
async fn renderer_import_texture(
    state: tauri::State<'_, Runtime>,
) -> Result<Option<RenderStatus>, String> {
    let h = handle(&state)?;
    if h.status().exporting {
        return Err("正在导出".into());
    }
    let Some(file) = rfd::AsyncFileDialog::new()
        .add_filter("Texture", &["jpg", "jpeg", "png", "tif", "tiff"])
        .pick_file()
        .await
    else {
        return Ok(None);
    };
    let path = file.path().to_owned();
    tauri::async_runtime::spawn_blocking(move || {
        let texture = renderer::appearance::load_texture(&path)?;
        h.set_texture(texture)?;
        Ok(Some(h.status()))
    })
    .await
    .map_err(|e| e.to_string())?
}
/// A user-requested snapshot, never a per-frame presentation/readback path.
#[tauri::command]
async fn renderer_preview(
    state: tauri::State<'_, Runtime>,
    orthographic: bool,
    params: Option<RenderParams>,
) -> Result<Vec<u8>, String> {
    if let Some(p) = params.as_ref() {
        validate_output(p)?;
    }
    let h = handle(&state)?;
    tauri::async_runtime::spawn_blocking(move || {
        if h.status().exporting {
            return Err("正在导出".into());
        }
        let (bytes, width, height) = h.capture_export_preview(orthographic, params)?;
        encode_preview_png(bytes, width, height)
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn renderer_export_preview_result(state: tauri::State<'_, Runtime>) -> Result<Vec<u8>, String> {
    let h = handle(&state)?;
    tauri::async_runtime::spawn_blocking(move || {
        let (bytes, width, height) = h.capture_export_result()?;
        encode_preview_png(bytes, width, height)
    }).await.map_err(|e| e.to_string())?
}
#[tauri::command]
fn renderer_open_export(state: tauri::State<'_, ExportResultPath>, directory: bool) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::{core::PCWSTR, Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL}};
    let saved = state.0.lock().map_err(|e| e.to_string())?.clone().ok_or("尚无成功导出的文件")?;
    let target = if directory { saved.parent().ok_or("导出目录无效")?.to_path_buf() } else { saved };
    if !target.exists() { return Err("导出文件或目录已不存在".into()); }
    let file: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    let action: Vec<u16> = "open".encode_utf16().chain(Some(0)).collect();
    let result = unsafe { ShellExecuteW(None, PCWSTR(action.as_ptr()), PCWSTR(file.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL) };
    if result.0 as isize <= 32 { return Err("无法打开导出结果".into()); }
    Ok(())
}
fn encode_preview_png(bytes: Vec<u8>, width: u32, height: u32) -> Result<Vec<u8>, String> {
    let rgba = image::RgbaImage::from_raw(width, height, bytes).ok_or("预览尺寸无效")?;
    let preview = image::DynamicImage::ImageRgba8(rgba).thumbnail(960, 720);
    let mut png = std::io::Cursor::new(Vec::new());
    preview
        .write_to(&mut png, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(png.into_inner())
}
#[tauri::command]
async fn renderer_remove_texture(state: tauri::State<'_, Runtime>) -> Result<RenderStatus, String> {
    let h = handle(&state)?;
    tauri::async_runtime::spawn_blocking(move || {
        h.clear_texture()?;
        Ok(h.status())
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn renderer_export(
    state: tauri::State<'_, Runtime>,
    result_state: tauri::State<'_, ExportResultPath>,
    params: Option<RenderParams>,
) -> Result<Option<String>, String> {
    if let Some(p) = params.as_ref() {
        validate_output(p)?;
    }
    let h = handle(&state)?;
    let Some(file) = rfd::AsyncFileDialog::new()
        .add_filter("PNG", &["png"])
        .set_file_name("terrain.png")
        .save_file()
        .await
    else {
        return Ok(None);
    };
    let path = file.path().with_extension("png");
    if let Some(source) = h.status().source {
        if std::fs::canonicalize(&path)
            .ok()
            .is_some_and(|p| std::fs::canonicalize(&source.path).ok().as_ref() == Some(&p))
        {
            return Err("输出文件不能覆盖当前输入图像".into());
        }
    }
    let result = path.to_string_lossy().into_owned();
    let saved_path = path.clone();
    tauri::async_runtime::spawn_blocking(move || h.export_png(path, params))
        .await
        .map_err(|e| e.to_string())??;
    *result_state.0.lock().map_err(|e| e.to_string())? = Some(saved_path);
    Ok(Some(result))
}
#[tauri::command]
async fn renderer_export_geotiff(
    state: tauri::State<'_, Runtime>,
    result_state: tauri::State<'_, ExportResultPath>,
    params: Option<RenderParams>,
) -> Result<Option<String>, String> {
    if let Some(p) = params.as_ref() {
        validate_output(p)?;
    }
    let h = handle(&state)?;
    let source = h.status().source.ok_or("请先导入带地理坐标的图像")?;
    let geo = source.geo.as_ref().ok_or("输入图像没有地理坐标")?;
    if geo.keys.is_empty() {
        return Err("输入坐标系缺少可写入 GeoTIFF 的定义".into());
    }
    let Some(file) = rfd::AsyncFileDialog::new()
        .add_filter("GeoTIFF", &["tif", "tiff"])
        .set_file_name("terrain-ortho.tif")
        .save_file()
        .await
    else {
        return Ok(None);
    };
    let mut path = file.path().to_owned();
    if !path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("tif") || e.eq_ignore_ascii_case("tiff"))
    {
        path.set_extension("tif");
    }
    // The imported source remains the persistent data source for later exports.
    if std::fs::canonicalize(&path)
        .ok()
        .is_some_and(|p| std::fs::canonicalize(&source.path).ok().as_ref() == Some(&p))
    {
        return Err("输出文件不能覆盖当前输入图像".into());
    }
    let result = path.to_string_lossy().into_owned();
    let saved_path = path.clone();
    tauri::async_runtime::spawn_blocking(move || h.export_geotiff(path, params))
        .await
        .map_err(|e| e.to_string())??;
    *result_state.0.lock().map_err(|e| e.to_string())? = Some(saved_path);
    Ok(Some(result))
}
#[tauri::command]
fn renderer_cancel_export(state: tauri::State<Runtime>) -> Result<(), String> {
    handle(&state)?.cancel_export();
    Ok(())
}
#[tauri::command]
fn ui_ready(control_count: usize) -> bool { let _ = control_count; true }
fn main() {
    tauri::Builder::default()
        .manage(Runtime::default())
        .manage(ExportResultPath::default())
        .invoke_handler(tauri::generate_handler![
            app_info,
            ui_ready,
            renderer_init,
            renderer_resize,
            renderer_update,
            renderer_pause,
            renderer_status,
            renderer_import,
            renderer_import_texture,
            renderer_remove_texture,
            renderer_preview,
            renderer_export_preview_result,
            renderer_open_export,
            renderer_export,
            renderer_export_geotiff,
            renderer_cancel_export
        ])
        .on_window_event(|window, event| {
            if matches!(
                event,
                tauri::WindowEvent::CloseRequested { .. } | tauri::WindowEvent::Destroyed
            ) {
                if let Ok(h) = handle(&window.state::<Runtime>()) {
                    h.shutdown();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("DEMStudio startup failed");
}
