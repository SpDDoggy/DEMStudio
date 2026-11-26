//! Independent export-layer names and decoding metadata.
use std::{path::{Path,PathBuf},fs};
use super::RenderParams;
pub(super) fn selected(p:&RenderParams)->Vec<&'static str>{
    [(p.export_normal,"normal"),(p.export_depth,"depth"),(p.export_shadow_layer,"shadow")]
        .into_iter().filter_map(|(on,name)|on.then_some(name)).collect()
}
pub(super) fn path(main:&Path,layer:&str)->Result<PathBuf,String>{
    let stem=main.file_stem().ok_or("导出文件名无效")?.to_string_lossy();
    Ok(main.with_file_name(format!("{stem}_{layer}.png")))
}
pub(super) fn metadata_path(main:&Path)->Result<PathBuf,String>{
    let stem=main.file_stem().ok_or("导出文件名无效")?.to_string_lossy();
    Ok(main.with_file_name(format!("{stem}_layers.json")))
}
pub(super) fn write_metadata(main:&Path,layers:&[&str],size:[u32;2],range:[f32;2])->Result<(),String>{
    let path=metadata_path(main)?;
    let content=serde_json::json!({"version":1,"width":size[0],"height":size[1],"bitDepth":16,"transfer":"linear (gamma 1.0)",
        "layers":layers,"mask":"terrain only; ground and background transparent",
        "normal":{"space":"world","axes":"+X image right, +Y image up, +Z terrain up","encoding":"rgb = normal * 0.5 + 0.5"},
        "depth":{"near":range[0],"far":range[1],"unit":"renderer scene unit (source grid cell)","encoding":"gray = clamp((far - primaryRayDistance) / (far - near), 0, 1)"},
        "shadow":{"encoding":"white = sun visible; black = sun occluded","light":"sun","samples":"same as main image"}});
    let pending=path.with_extension(format!("json.{}.part",std::process::id()));
    let result=(||{fs::write(&pending,serde_json::to_vec_pretty(&content).map_err(|e|e.to_string())?).map_err(|e|e.to_string())?;fs::rename(&pending,&path).map_err(|e|e.to_string())})();
    let _=fs::remove_file(pending);result
}
