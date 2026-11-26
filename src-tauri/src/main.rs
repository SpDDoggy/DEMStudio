#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod renderer;
mod video;
mod sidecar;
mod palette_library;
mod texture_history;
mod mcp_bridge;
use renderer::{native::RendererHandle, RenderParams, RenderStatus, ViewportRect};
use serde::Serialize;
use std::{
    path::PathBuf,
    sync::Mutex,
};
use tauri::{Manager, Emitter};
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
    if !["source", "half", "quarter", "preview"].contains(&p.export_terrain_scale.as_str()) {
        return Err("无效的导出地形精度".into());
    }
    if !matches!(p.output_bit_depth, 8 | 16) { return Err("输出位深仅支持8或16".into()); }
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
        p.height_curve,
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
        || !["linear", "accelerated", "nonlinear"].contains(&p.height_scale_mode.as_str())
        || !(0.01..=1000.).contains(&p.height_curve)
        || (p.height_scale_mode != "nonlinear" && p.height_curve != 1.)
        || !(0. ..=100.).contains(&p.height_offset)
        || p.sun_size <= 0.
        || !(0. ..=90.).contains(&p.sun_altitude)
        || p.height_step < 0.
        || p.sun_intensity < 0.
        || p.sky_intensity < 0.
        || !["constant", "atmosphere"].contains(&p.sky_model.as_str())
        || !(1..=if p.experimental_bounces {100} else {15}).contains(&p.max_bounces)
        || p.exposure < 0.
        || !p.shadow_padding.is_finite()
        || !(1. ..=200.).contains(&p.shadow_padding)
    {
        return Err("渲染参数超出有效范围".into());
    }
    renderer::gl_renderer::validate_contour_parameters(p)?;
    renderer::validate_appearance_parameters(p)?;
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
    renderer::validate_fog_parameters(p)?;
    renderer::validate_cloud_parameters(p)?;
    renderer::validate_water_parameters(p)?;
    if p.bloom != 0. {
        return Err("当前基线尚未启用泛光".into());
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
        name: "DemStudio",
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
        // Reuse the existing first-frame preview budget, then restore the exact
        // requested quality through the worker's normal idle refinement.
        h.set_interactive_params(params)?;
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
fn mcp_validate_params(params:RenderParams,output:bool)->Result<(),String>{
    if output {validate_output(&params)}else{validate(&params)}
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
async fn renderer_texture_preview(state:tauri::State<'_,Runtime>)->Result<Vec<u8>,String> {
    let handle=handle(&state)?.clone();
    tauri::async_runtime::spawn_blocking(move|| {
        let (rgba,width,height)=handle.texture_preview()?;
        encode_preview_png(rgba,width,height)
    }).await.map_err(|error|error.to_string())?
}

#[tauri::command]
async fn renderer_sample_appearance(x:f32,y:f32,state:tauri::State<'_,Runtime>)->Result<renderer::AppearanceSample,String> {
    let handle=handle(&state)?.clone();
    tauri::async_runtime::spawn_blocking(move||handle.sample_appearance(x,y)).await.map_err(|error|error.to_string())?
}
#[tauri::command]
async fn renderer_import(state: tauri::State<'_, Runtime>, sidecars: tauri::State<'_, sidecar::SidecarIo>, app: tauri::AppHandle) -> Result<Option<RenderStatus>, String> {
    let h = handle(&state)?;
    let Some(file) = rfd::AsyncFileDialog::new()
        .add_filter("Heightmap", &["jpg", "jpeg", "png", "tif", "tiff", "asc"])
        .pick_file()
        .await
    else {
        return Ok(None);
    };
    let path = file.path().to_owned();
    app.emit("renderer-import-started", ()).map_err(|e| e.to_string())?;
    let was_paused=h.status().paused;
    h.pause(true)?;
    let imported=async {
    let map = tauri::async_runtime::spawn_blocking(move || renderer::heightmap::load(&path))
        .await
        .map_err(|e| e.to_string())??;
    let initial_height_scale=renderer::initial_height_scale(&map);
    {
        let _sidecar_guard=sidecars.0.lock().map_err(|e|e.to_string())?;
        h.set_map(map)?;
    }
    // Import completes only after the render worker has uploaded the map.
    let ready=h.clone();
    let mut result=tauri::async_runtime::spawn_blocking(move || ready.wait_map_ready())
        .await.map_err(|e|e.to_string())??;
    result.initial_height_scale=initial_height_scale;
    result.paused=was_paused;
    Ok(Some(result))
    }.await;
    let resumed=h.pause(was_paused);
    match (imported,resumed) {
        (Err(error),_)=>Err(error),
        (Ok(_),Err(error))=>Err(error),
        (Ok(result),Ok(()))=>Ok(result),
    }
}
#[tauri::command]
fn source_settings_load(path:String,state:tauri::State<Runtime>,sidecars:tauri::State<sidecar::SidecarIo>)->Result<Option<serde_json::Value>,String>{
    let _guard=sidecars.0.lock().map_err(|e|e.to_string())?;
    let source=handle(&state)?.status().source;
    let target=sidecar::checked_path(source.as_ref().map(|s|s.path.as_path()),&path)?;
    sidecar::load(&target)
}
#[tauri::command]
fn source_settings_save(path:String,document:serde_json::Value,state:tauri::State<Runtime>,sidecars:tauri::State<sidecar::SidecarIo>)->Result<(),String>{
    let _guard=sidecars.0.lock().map_err(|e|e.to_string())?;
    let source=handle(&state)?.status().source;
    let target=sidecar::checked_path(source.as_ref().map(|s|s.path.as_path()),&path)?;
    sidecar::save(&target,document)
}
#[tauri::command]
async fn renderer_import_texture(
    state: tauri::State<'_, Runtime>,
    assets: tauri::State<'_, texture_history::TextureHistory>,
) -> Result<Option<serde_json::Value>, String> {
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
    let assets = assets.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let texture = renderer::appearance::load_texture(&path)?;
        let mut assets = assets.lock().map_err(|e| e.to_string())?;
        let id = assets.save(&texture)?;
        assets.set_original_path(&id, path)?;
        if let Err(error) = h.set_texture(texture) { assets.remove(&id); return Err(error); }
        assets.active = Some(id.clone());
        let mut status = serde_json::to_value(h.status()).map_err(|e| e.to_string())?;
        status["textureAsset"] = serde_json::json!(id);
        Ok(Some(status))
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn renderer_restore_texture(asset: Option<String>, state: tauri::State<'_, Runtime>, assets: tauri::State<'_, texture_history::TextureHistory>) -> Result<serde_json::Value, String> {
    let h = handle(&state)?;
    let assets = assets.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut assets = assets.lock().map_err(|e| e.to_string())?;
        if h.status().exporting { return Err("正在导出".into()); }
        if let Some(id) = asset.as_deref() { h.set_texture(assets.load(id)?)?; } else { h.clear_texture()?; }
        assets.active = asset.clone();
        let mut status = serde_json::to_value(h.status()).map_err(|e| e.to_string())?;
        status["textureAsset"] = serde_json::json!(asset);
        Ok(status)
    }).await.map_err(|e| e.to_string())?
}
#[tauri::command]
async fn renderer_prune_texture_assets(keep: Vec<String>, assets: tauri::State<'_, texture_history::TextureHistory>) -> Result<(), String> {
    let assets = assets.0.clone();
    tauri::async_runtime::spawn_blocking(move || { assets.lock().map_err(|e| e.to_string())?.prune(&keep); Ok(()) }).await.map_err(|e| e.to_string())?
}
#[tauri::command]
async fn renderer_appearance_distribution(state: tauri::State<'_, Runtime>) -> Result<renderer::AppearanceDistribution, String> {
    let h = handle(&state)?.clone();
    tauri::async_runtime::spawn_blocking(move || h.appearance_distribution()).await.map_err(|error| error.to_string())?
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
async fn renderer_layout_capture(state:tauri::State<'_,Runtime>,params:RenderParams)->Result<Vec<u8>,String>{
    let params=layout_capture_params(params)?;
    let h=handle(&state)?;
    tauri::async_runtime::spawn_blocking(move||{let(pixels,width,height)=h.capture_output_frame_with_params(Some(params))?;encode_capture_png(pixels,width,height)}).await.map_err(|e|e.to_string())?
}
fn layout_capture_params(mut params:RenderParams)->Result<RenderParams,String>{
    params.output_bit_depth=8;params.export_transparent=true;params.export_orthographic=false;params.export_georeference=false;
    params.export_shadow=false;params.export_normal=false;params.export_depth=false;params.export_shadow_layer=false;
    params.grid=false;params.ground_enabled=true;params.ground_shadow=true;params.appearance_pick=false;params.layout_shadow_only=true;
    params.cloud_enabled=false;params.water_enabled=false;
    validate_output(&params)?;Ok(params)
}
#[tauri::command]
async fn renderer_export_layout(bytes:Vec<u8>,filename:String,result_state:tauri::State<'_,ExportResultPath>)->Result<Option<String>,String>{
    let (width,height)=image::ImageReader::with_format(std::io::Cursor::new(&bytes),image::ImageFormat::Png).into_dimensions().map_err(|e|format!("布局图像无效：{e}"))?;
    if width==0||height==0||width>8192||height>8192{return Err("布局尺寸无效".into());}
    let name=PathBuf::from(filename).file_name().and_then(|s|s.to_str()).unwrap_or("地形布局.png").to_owned();
    let Some(file)=rfd::AsyncFileDialog::new().add_filter("PNG",&["png"]).set_file_name(name).save_file().await else{return Ok(None)};
    let path=file.path().to_path_buf();let saved=path.clone();
    tauri::async_runtime::spawn_blocking(move||std::fs::write(&saved,bytes)).await.map_err(|e|e.to_string())?.map_err(|e|e.to_string())?;
    *result_state.0.lock().map_err(|e|e.to_string())?=Some(path.clone());Ok(Some(path.to_string_lossy().into_owned()))
}
#[tauri::command]
async fn renderer_capture_png(state: tauri::State<'_, Runtime>) -> Result<Vec<u8>, String> {
    let h = handle(&state)?;
    tauri::async_runtime::spawn_blocking(move || {
        if h.status().exporting { return Err("正在导出".into()); }
        let (bytes, width, height) = h.capture_frame()?;
        encode_capture_png(bytes, width, height)
    }).await.map_err(|e| e.to_string())?
}
fn encode_capture_png(bytes: Vec<u8>, width: u32, height: u32) -> Result<Vec<u8>, String> {
    use image::ImageEncoder;
    let expected = (width as usize).checked_mul(height as usize).and_then(|n| n.checked_mul(4));
    if width == 0 || height == 0 || expected != Some(bytes.len()) { return Err("画面尺寸无效".into()); }
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png).write_image(&bytes, width, height, image::ExtendedColorType::Rgba8).map_err(|e| e.to_string())?;
    Ok(png)
}
#[tauri::command]
fn renderer_open_source(kind: String, state: tauri::State<'_, Runtime>, assets: tauri::State<'_, texture_history::TextureHistory>) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::{core::PCWSTR, Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL}};
    let source = match kind.as_str() {
        "height" => handle(&state)?.status().source.map(|source| source.path).ok_or("尚未导入高程图")?,
        "texture" => assets.0.lock().map_err(|e| e.to_string())?.active_original_path().ok_or("尚未导入贴图")?,
        _ => return Err("源文件类型无效".into()),
    };
    let target = source.parent().ok_or("源文件目录无效")?;
    if !target.is_dir() { return Err("源文件目录已不存在".into()); }
    let file: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    let action: Vec<u16> = "open".encode_utf16().chain(Some(0)).collect();
    let result = unsafe { ShellExecuteW(None, PCWSTR(action.as_ptr()), PCWSTR(file.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL) };
    if result.0 as isize <= 32 { return Err("无法打开源文件目录".into()); }
    Ok(())
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
async fn renderer_remove_texture(state: tauri::State<'_, Runtime>, assets: tauri::State<'_, texture_history::TextureHistory>) -> Result<RenderStatus, String> {
    let h = handle(&state)?;
    let assets = assets.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut assets = assets.lock().map_err(|e| e.to_string())?;
        h.clear_texture()?;
        assets.active = None;
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
    format: Option<String>,
) -> Result<Option<String>, String> {
    let format = format.as_deref().unwrap_or("png");
    let (label, extension) = match format {
        "png" => ("PNG", "png"),
        "jpeg" => ("JPEG", "jpg"),
        "bmp" => ("BMP", "bmp"),
        "tga" => ("TGA", "tga"),
        _ => return Err("不支持的导出格式".into()),
    };
    let mut params = params;
    if let Some(p) = params.as_mut() {
        if format != "png" { p.output_bit_depth = 8; }
        if matches!(format, "jpeg" | "bmp") { p.export_transparent = false; }
    }
    if let Some(p) = params.as_ref() {
        validate_output(p)?;
    }
    let h = handle(&state)?;
    let Some(file) = rfd::AsyncFileDialog::new()
        .add_filter(label, &[extension])
        .set_file_name(format!("terrain.{extension}"))
        .save_file()
        .await
    else {
        return Ok(None);
    };
    let path = file.path().with_extension(extension);
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
async fn renderer_video_preview(state:tauri::State<'_,Runtime>,video:tauri::State<'_,video::VideoState>,mut params:RenderParams)->Result<Vec<u8>,String>{
 if video.busy.load(std::sync::atomic::Ordering::Acquire){return Err("正在导出视频".into())}
 params.output_bit_depth=8;params.export_transparent=false;params.export_orthographic=false;params.export_georeference=false;
 validate_output(&params)?;
 let h=handle(&state)?;
 tauri::async_runtime::spawn_blocking(move||{let(pixels,width,height)=h.capture_video_frame(params)?;encode_preview_png(pixels,width,height)}).await.map_err(|e|e.to_string())?
}
#[tauri::command]
fn renderer_video_progress(state:tauri::State<video::VideoState>)->video::VideoProgress {state.progress()}
#[tauri::command]
async fn renderer_export_video(state:tauri::State<'_,Runtime>,video:tauri::State<'_,video::VideoState>,result_state:tauri::State<'_,ExportResultPath>,mut frames:Vec<RenderParams>,fps:Option<u32>)->Result<Option<String>,String>{
 use std::sync::atomic::Ordering;
 let fps=fps.unwrap_or(30);
 if !(1..=60).contains(&fps)||frames.is_empty()||frames.len()>18000{return Err("视频帧数或帧率无效".into())}
 let (width,height)=(frames[0].width,frames[0].height);
 if width<2||height<2||width>3840||height>2160||width%2!=0||height%2!=0{return Err("视频尺寸须为偶数，最大 3840 × 2160".into())}
 for frame in &mut frames {if frame.width!=width||frame.height!=height{return Err("视频帧尺寸必须一致".into())}frame.output_bit_depth=8;frame.export_transparent=false;frame.export_orthographic=false;frame.export_georeference=false;validate_output(frame)?;}
 let h=handle(&state)?;
 if video.busy.compare_exchange(false,true,Ordering::AcqRel,Ordering::Acquire).is_err(){return Err("正在导出视频".into())}
 video.cancel.store(false,Ordering::Release);video.completed.store(0,Ordering::Release);video.total.store(frames.len() as u32,Ordering::Release);
 let Some(file)=rfd::AsyncFileDialog::new().add_filter("MP4",&["mp4"]).set_file_name("terrain.mp4").save_file().await else{video.busy.store(false,Ordering::Release);return Ok(None)};
 let path=file.path().with_extension("mp4");
 if h.status().source.is_some_and(|source|std::fs::canonicalize(&path).ok().is_some_and(|p|std::fs::canonicalize(&source.path).ok().as_ref()==Some(&p))){video.busy.store(false,Ordering::Release);return Err("输出文件不能覆盖当前输入图像".into())}
 let saved=path.clone();let progress=video.inner().clone();
 let result=tauri::async_runtime::spawn_blocking(move||{
  let temp=video::temporary_path(&path);
  let result=(||->Result<(),String>{
   let mut encoder=video::Encoder::new(&temp,width,height,fps)?;
   for frame in frames {if progress.cancel.load(Ordering::Acquire){return Err("已取消视频导出".into())}
    let (pixels,w,hgt)=h.capture_video_frame(frame)?;
    if progress.cancel.load(Ordering::Acquire){return Err("已取消视频导出".into())}
    if (w,hgt)!=(width,height){return Err("视频渲染帧尺寸不一致".into())}encoder.write(&pixels)?;progress.completed.fetch_add(1,Ordering::AcqRel);
   }
   encoder.finish()?;if progress.cancel.load(Ordering::Acquire){return Err("已取消视频导出".into())}
   std::fs::rename(&temp,&path).map_err(|e|format!("保存视频失败：{e}"))?;Ok(())
  })();if result.is_err(){let _=std::fs::remove_file(&temp);}result
 }).await.map_err(|e|e.to_string()).and_then(|r|r);
 video.busy.store(false,Ordering::Release);
 result?;*result_state.0.lock().map_err(|e|e.to_string())?=Some(saved.clone());Ok(Some(saved.to_string_lossy().into_owned()))
}
#[tauri::command]
async fn renderer_export_mesh(state:tauri::State<'_,Runtime>,result_state:tauri::State<'_,ExportResultPath>,params:RenderParams,format:String)->Result<Option<String>,String>{
    let extension=match format.as_str(){"ply"=>"ply","obj"=>"obj",_=>return Err("不支持的模型格式".into())};
    let h=handle(&state)?;
    let Some(file)=rfd::AsyncFileDialog::new().add_filter(extension.to_ascii_uppercase(),&[extension]).set_file_name(format!("terrain.{extension}")).save_file().await else{return Ok(None)};
    let path=file.path().with_extension(extension);
    if let Some(source)=h.status().source {
        if std::fs::canonicalize(&path).ok().is_some_and(|p|std::fs::canonicalize(&source.path).ok().as_ref()==Some(&p)){return Err("输出文件不能覆盖当前输入图像".into())}
    }
    let result=path.to_string_lossy().into_owned();let saved=path.clone();
    tauri::async_runtime::spawn_blocking(move||h.export_mesh(path,params)).await.map_err(|e|e.to_string())??;
    *result_state.0.lock().map_err(|e|e.to_string())?=Some(saved);Ok(Some(result))
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
    let source = h.status().source;
    let Some(file) = rfd::AsyncFileDialog::new()
        .add_filter("TIFF", &["tif", "tiff"])
        .set_file_name("terrain.tif")
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
        .is_some_and(|p| source.as_ref().and_then(|source| std::fs::canonicalize(&source.path).ok()).as_ref() == Some(&p))
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

// Headless MCP path commands reuse the same renderer/encoders as UI exports.
fn mcp_input_path(path:&str)->Result<PathBuf,String>{
    let path=PathBuf::from(path);
    if !path.is_absolute(){return Err("必须提供绝对路径".into());}
    let path=std::fs::canonicalize(path).map_err(|e|e.to_string())?;
    if !path.is_file(){return Err("输入路径不是文件".into());}Ok(path)
}
fn mcp_output_path(path:&str,extensions:&[&str])->Result<PathBuf,String>{
    let path=PathBuf::from(path);
    if !path.is_absolute(){return Err("必须提供绝对路径".into());}
    if !path.extension().and_then(|x|x.to_str()).is_some_and(|x|extensions.iter().any(|e|x.eq_ignore_ascii_case(e))){return Err("输出扩展名与格式不一致".into());}
    match std::fs::symlink_metadata(&path){Ok(_)=>return Err("输出文件已存在，不能覆盖".into()),Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{},Err(e)=>return Err(e.to_string())}
    let parent=std::fs::canonicalize(path.parent().ok_or("输出目录无效")?).map_err(|e|e.to_string())?;
    if !parent.is_dir(){return Err("输出目录无效".into());}
    Ok(parent.join(path.file_name().ok_or("输出文件名无效")?))
}
fn mcp_staged_export<F>(path:&std::path::Path,export:F)->Result<(),String> where F:FnOnce(PathBuf)->Result<(),String>{
    static SERIAL:std::sync::atomic::AtomicU64=std::sync::atomic::AtomicU64::new(0);
    let directory=path.parent().ok_or("输出目录无效")?.join(format!(".demstudio-mcp-{}-{}",std::process::id(),SERIAL.fetch_add(1,std::sync::atomic::Ordering::Relaxed)));
    std::fs::create_dir(&directory).map_err(|e|e.to_string())?;
    let result=(||{
        export(directory.join(path.file_name().ok_or("输出文件名无效")?))?;
        let mut staged=Vec::new();
        for item in std::fs::read_dir(&directory).map_err(|e|e.to_string())?{
            let item=item.map_err(|e|e.to_string())?;
            if !item.file_type().map_err(|e|e.to_string())?.is_file(){return Err("导出包含无效文件".into());}
            staged.push((item.path(),path.parent().unwrap().join(item.file_name())));
        }
        if !staged.iter().any(|(_,target)|target==path){return Err("导出未生成目标文件".into());}
        let mut committed=Vec::new();
        for (source,target) in staged {
            if let Err(error)=std::fs::hard_link(source,&target){
                for own in committed{let _=std::fs::remove_file(own);}
                return Err(format!("保存失败（已有文件不会覆盖）：{error}"));
            }
            committed.push(target);
        }Ok(())
    })();
    let _=std::fs::remove_dir_all(directory);result
}
#[tauri::command]
async fn mcp_import_path(path:String,kind:String,state:tauri::State<'_,Runtime>,sidecars:tauri::State<'_,sidecar::SidecarIo>,assets:tauri::State<'_,texture_history::TextureHistory>)->Result<serde_json::Value,String>{
    let path=mcp_input_path(&path)?;let h=handle(&state)?;
    if h.status().exporting{return Err("正在导出".into());}
    match kind.as_str(){
        "dem"|"heightmap"=>{
            let was_paused=h.status().paused;h.pause(true)?;
            let imported=async {
                let map=tauri::async_runtime::spawn_blocking(move||renderer::heightmap::load(&path)).await.map_err(|e|e.to_string())??;
                let initial=renderer::initial_height_scale(&map);
                {let _guard=sidecars.0.lock().map_err(|e|e.to_string())?;h.set_map(map)?;}
                let ready=h.clone();let mut status=tauri::async_runtime::spawn_blocking(move||ready.wait_map_ready()).await.map_err(|e|e.to_string())??;
                status.initial_height_scale=initial;status.paused=was_paused;serde_json::to_value(status).map_err(|e|e.to_string())
            }.await;
            let resumed=h.pause(was_paused);
            match (imported,resumed){(Err(e),_)=>Err(e),(Ok(_),Err(e))=>Err(e),(Ok(value),Ok(()))=>Ok(value)}
        },
        "texture"=>{
            let assets=assets.0.clone();
            tauri::async_runtime::spawn_blocking(move||{
                let texture=renderer::appearance::load_texture(&path)?;let mut assets=assets.lock().map_err(|e|e.to_string())?;
                let id=assets.save(&texture)?;assets.set_original_path(&id,path)?;
                if let Err(error)=h.set_texture(texture){assets.remove(&id);return Err(error);}
                assets.active=Some(id.clone());let mut status=serde_json::to_value(h.status()).map_err(|e|e.to_string())?;status["textureAsset"]=serde_json::json!(id);Ok(status)
            }).await.map_err(|e|e.to_string())?
        },_=>Err("不支持的导入类型".into())
    }
}
#[tauri::command]
async fn mcp_export_path(path:String,kind:String,format:String,mut params:Option<RenderParams>,mut frames:Option<Vec<RenderParams>>,fps:Option<u32>,bytes:Option<Vec<u8>>,state:tauri::State<'_,Runtime>,video:tauri::State<'_,video::VideoState>,result_state:tauri::State<'_,ExportResultPath>)->Result<String,String>{
    use std::sync::atomic::Ordering;
    let extensions:&[&str]=match (kind.as_str(),format.as_str()){
        ("image","png")|( "layout","png")=>&["png"],("image","jpeg"|"jpg")=>&["jpg","jpeg"],("image","bmp")=>&["bmp"],("image","tga")=>&["tga"],
        ("image","tiff"|"tif"|"geotiff")|("geotiff","tiff"|"tif"|"geotiff")=>&["tif","tiff"],("mesh","ply")=>&["ply"],("mesh","obj")=>&["obj"],("video","mp4")=>&["mp4"],_=>return Err("不支持的导出类型或格式".into())};
    let path=mcp_output_path(&path,extensions)?;let h=handle(&state)?;
    if h.status().exporting||video.busy.load(Ordering::Acquire){return Err("正在导出".into());}
    if let Some(p)=params.as_mut(){
        if kind=="image"&&!matches!(format.as_str(),"png"|"tiff"|"tif"|"geotiff"){p.output_bit_depth=8;}
        if matches!(format.as_str(),"jpeg"|"jpg"|"bmp"){p.export_transparent=false;}
        validate_output(p)?;
    }
    let fps=fps.unwrap_or(30);
    if kind=="mesh"&&params.is_none(){return Err("模型导出缺少参数".into());}
    if kind=="layout"{
        let data=bytes.as_ref().ok_or("布局导出缺少 PNG 数据")?;
        let (w,h)=image::ImageReader::with_format(std::io::Cursor::new(data),image::ImageFormat::Png).into_dimensions().map_err(|e|format!("布局图像无效：{e}"))?;
        if w==0||h==0||w>8192||h>8192{return Err("布局尺寸无效".into());}
    }
    if kind=="video"{
        let frames=frames.as_mut().ok_or("视频导出缺少帧参数")?;
        if !(1..=60).contains(&fps)||frames.is_empty()||frames.len()>18000{return Err("视频帧数或帧率无效".into());}
        let (w,h)=(frames[0].width,frames[0].height);
        if w<2||h<2||w>3840||h>2160||w%2!=0||h%2!=0{return Err("视频尺寸须为偶数，最大 3840 × 2160".into());}
        for frame in frames.iter_mut(){if (frame.width,frame.height)!=(w,h){return Err("视频帧尺寸必须一致".into());}frame.output_bit_depth=8;frame.export_transparent=false;frame.export_orthographic=false;frame.export_georeference=false;validate_output(frame)?;}
        if video.busy.compare_exchange(false,true,Ordering::AcqRel,Ordering::Acquire).is_err(){return Err("正在导出视频".into());}
        video.cancel.store(false,Ordering::Release);video.completed.store(0,Ordering::Release);video.total.store(frames.len() as u32,Ordering::Release);
    }
    let saved=path.clone();let progress=video.inner().clone();let is_video=kind=="video";
    let result=tauri::async_runtime::spawn_blocking(move||mcp_staged_export(&path,|target|{
        match kind.as_str(){
            "image"|"geotiff"=>if matches!(format.as_str(),"tiff"|"tif"|"geotiff"){h.export_geotiff(target,params)}else{h.export_png(target,params)},
            "mesh"=>h.export_mesh(target,params.ok_or("模型导出缺少参数")?),
            "layout"=>std::fs::write(target,bytes.ok_or("布局导出缺少 PNG 数据")?).map_err(|e|e.to_string()),
            "video"=>{
                let frames=frames.ok_or("视频导出缺少帧参数")?;let (width,height)=(frames[0].width,frames[0].height);
                let mut encoder=video::Encoder::new(&target,width,height,fps)?;
                for frame in frames{
                    if progress.cancel.load(Ordering::Acquire){return Err("已取消视频导出".into());}
                    let (pixels,w,hgt)=h.capture_video_frame(frame)?;
                    if progress.cancel.load(Ordering::Acquire){return Err("已取消视频导出".into());}
                    if (w,hgt)!=(width,height){return Err("视频渲染帧尺寸不一致".into());}
                    encoder.write(&pixels)?;progress.completed.fetch_add(1,Ordering::AcqRel);
                }
                encoder.finish()?;if progress.cancel.load(Ordering::Acquire){return Err("已取消视频导出".into());}Ok(())
            },_=>Err("不支持的导出类型".into())
        }
    })).await.map_err(|e|e.to_string()).and_then(|r|r);
    if is_video{video.busy.store(false,Ordering::Release);}
    result?;*result_state.0.lock().map_err(|e|e.to_string())?=Some(saved.clone());Ok(saved.to_string_lossy().into_owned())
}

#[tauri::command]
fn renderer_cancel_export(state: tauri::State<Runtime>, video:tauri::State<video::VideoState>) -> Result<(), String> {
    video.cancel.store(true, std::sync::atomic::Ordering::Release);
    handle(&state)?.cancel_export();
    Ok(())
}






#[tauri::command]
fn ui_ready(control_count: usize) -> bool { let _ = control_count; true }
fn main() {
    tauri::Builder::default()
        .manage(Runtime::default())
        .manage(video::VideoState::default())
        .manage(ExportResultPath::default())
        .manage(sidecar::SidecarIo::default())
        .manage(texture_history::TextureHistory::default())
        .invoke_handler(tauri::generate_handler![
            app_info,
            ui_ready,
            renderer_init,
            renderer_resize,
            renderer_update,
            mcp_validate_params,
            mcp_bridge::mcp_control_reply,
            mcp_bridge::mcp_control_ready,
            renderer_pause,
            renderer_status,
            renderer_texture_preview,
            renderer_sample_appearance,
            renderer_import,
            mcp_import_path,
            mcp_export_path,
            source_settings_load,
            source_settings_save,
            palette_library::palette_library_load,
            palette_library::palette_library_save,
            palette_library::palette_library_import,
            palette_library::palette_library_export,
            renderer_import_texture,
            renderer_restore_texture,
            renderer_prune_texture_assets,
            renderer_appearance_distribution,
            renderer_remove_texture,
            renderer_preview,
            renderer_export_preview_result,
            renderer_open_export,
            renderer_open_source,
            renderer_capture_png,
            renderer_export_layout,
            renderer_layout_capture,
            renderer_export,
            renderer_export_mesh,
            renderer_export_video,
            renderer_video_progress,
            renderer_video_preview,
            renderer_export_geotiff,
            renderer_cancel_export
        ])
        .on_window_event(|window, event| {
            if matches!(
                event,
                tauri::WindowEvent::Destroyed
            ) {
                mcp_bridge::stop();
                if let Ok(h) = handle(&window.state::<Runtime>()) {
                    h.shutdown();
                }
            }
        })
        .setup(move |app| {
            if std::env::var("DEMSTUDIO_MCP").as_deref() != Ok("0") {
                if let Err(error) = mcp_bridge::start(app.handle().clone()) {
                    eprintln!("MCP连接启动失败: {error}");
                }
            }
            let window = app.get_webview_window("main").ok_or("主窗口不存在")?;
            let app_handle = app.handle().clone();
            window.with_webview(move |webview| {
                let result = unsafe {
                    webview.controller().CoreWebView2().and_then(|core| core.Settings()).and_then(|settings| settings.SetAreDefaultContextMenusEnabled(false))
                };
                if let Err(error) = result {
                    eprintln!("禁用 WebView 默认菜单失败: {error}");
                    app_handle.exit(1);
                }
            })?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("DemStudio startup failed");
}
