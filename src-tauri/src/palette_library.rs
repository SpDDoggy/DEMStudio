use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashSet, fs::{self, OpenOptions}, io::{Read, Write}, path::{Path, PathBuf}, sync::{Mutex, atomic::{AtomicU64, Ordering}}};
use tauri::Manager;

const MAX_BYTES: u64 = 1024 * 1024;
static SERIAL: AtomicU64 = AtomicU64::new(0);
static LIBRARY_IO: Mutex<()> = Mutex::new(());
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document { version: u32, palettes: Vec<Palette> }
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Palette { id: String, name: String, stops: Vec<Stop> }
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stop { position: f64, color: String }

pub fn validate(value: &Value) -> Result<(), String> {
    let document: Document = serde_json::from_value(value.clone()).map_err(|_| "色带库格式无效")?;
    if document.version != 1 { return Err("色带库版本无效".into()); }
    if document.palettes.is_empty() { return Err("色带库不能为空".into()); }
    let mut ids = HashSet::new();
    for palette in document.palettes {
        if palette.id.trim().is_empty() || palette.name.trim().is_empty() || !ids.insert(palette.id) { return Err("色带名称或标识无效".into()); }
        if !(2..=16).contains(&palette.stops.len()) { return Err("色带须包含 2～16 个色标".into()); }
        if palette.stops.first().unwrap().position != 0. || palette.stops.last().unwrap().position != 1. { return Err("色带首末位置须为 0 和 1".into()); }
        let mut previous = -1.;
        for stop in palette.stops {
            if !stop.position.is_finite() || !(0. ..=1.).contains(&stop.position) || stop.position <= previous { return Err("色标位置须严格递增".into()); }
            if stop.color.len() != 7 || !stop.color.starts_with('#') || !stop.color.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit) { return Err("色标颜色无效".into()); }
            previous = stop.position;
        }
    }
    Ok(())
}
fn read(path: &Path) -> Result<Value, String> {
    let file = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_BYTES { return Err("色带库文件过大".into()); }
    let value = serde_json::from_slice(&bytes).map_err(|_| "色带库 JSON 无效")?;
    validate(&value)?;
    Ok(value)
}
fn save(path: &Path, value: &Value) -> Result<(), String> {
    validate(value)?;
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_BYTES { return Err("色带库文件过大".into()); }
    if let Some(parent) = path.parent() { fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".tmp-{}-{}", std::process::id(), SERIAL.fetch_add(1, Ordering::Relaxed)));
    let temp = PathBuf::from(name);
    let result = (|| {
        let mut file = OpenOptions::new().write(true).create_new(true).open(&temp).map_err(|e| e.to_string())?;
        file.write_all(&bytes).and_then(|_| file.sync_all()).map_err(|e| e.to_string())?;
        drop(file);
        replace(&temp, path).map_err(|e| e.to_string())
    })();
    if result.is_err() { let _ = fs::remove_file(&temp); }
    result
}
#[cfg(windows)]
fn replace(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name="kernel32")]
    extern "system" { fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32; }
    let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 1 | 8) } == 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) }
}
#[cfg(not(windows))]
fn replace(from: &Path, to: &Path) -> std::io::Result<()> { fs::rename(from, to) }
fn load_or_create(path: &Path, defaults: Value) -> Result<Value, String> {
    match fs::metadata(path) {
        Ok(_) => read(path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => { save(path, &defaults)?; Ok(defaults) },
        Err(e) => Err(e.to_string()),
    }
}
fn library_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(app.path().app_config_dir().map_err(|e| e.to_string())?.join("palettes.json"))
}
#[tauri::command]
pub fn palette_library_load(app: tauri::AppHandle, defaults: Value) -> Result<Value, String> {
    let _guard = LIBRARY_IO.lock().map_err(|e| e.to_string())?;
    load_or_create(&library_path(&app)?, defaults)
}
#[tauri::command]
pub fn palette_library_save(app: tauri::AppHandle, document: Value) -> Result<(), String> {
    let _guard = LIBRARY_IO.lock().map_err(|e| e.to_string())?;
    save(&library_path(&app)?, &document)
}
#[tauri::command]
pub async fn palette_library_import(app: tauri::AppHandle) -> Result<Option<Value>, String> {
    let Some(file) = rfd::AsyncFileDialog::new().add_filter("JSON", &["json"]).pick_file().await else { return Ok(None); };
    let document = read(file.path())?;
    { let _guard = LIBRARY_IO.lock().map_err(|e| e.to_string())?; save(&library_path(&app)?, &document)?; }
    Ok(Some(document))
}
#[tauri::command]
pub async fn palette_library_export(document: Value) -> Result<bool, String> {
    validate(&document)?;
    let Some(file) = rfd::AsyncFileDialog::new().add_filter("JSON", &["json"]).set_file_name("palettes.json").save_file().await else { return Ok(false); };
    save(file.path(), &document)?;
    Ok(true)
}
