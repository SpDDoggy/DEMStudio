use crate::renderer::appearance::{load_texture, ColorTexture};
use image::ImageEncoder;
use std::{collections::{HashMap, HashSet}, fs::{self, OpenOptions}, io::Write, path::PathBuf, sync::{Arc, Mutex}, time::{SystemTime, UNIX_EPOCH}};

#[derive(Default, Clone)]
pub struct TextureHistory(pub Arc<Mutex<TextureAssets>>);

#[derive(Default)]
pub struct TextureAssets {
    directory: Option<PathBuf>,
    next: u64,
    entries: HashMap<String, Asset>,
    pub active: Option<String>,
}
struct Asset { path: PathBuf, name: String, original_path: Option<PathBuf> }

impl TextureAssets {
    pub fn save(&mut self, texture: &ColorTexture) -> Result<String, String> {
        if texture.width == 0 || texture.height == 0 || (texture.width as usize).checked_mul(texture.height as usize).and_then(|n| n.checked_mul(4)) != Some(texture.rgba.len()) { return Err("贴图尺寸无效".into()); }
        if self.directory.is_none() {
            let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|e| e.to_string())?.as_nanos();
            let path = std::env::temp_dir().join(format!("demstudio-texture-{}-{stamp}", std::process::id()));
            fs::create_dir(&path).map_err(|e| e.to_string())?;
            self.directory = Some(path);
        }
        self.next += 1;
        let id = format!("texture-{}", self.next);
        let path = self.directory.as_ref().unwrap().join(format!("{id}.png"));
        let file = OpenOptions::new().write(true).create_new(true).open(&path).map_err(|e| e.to_string())?;
        let mut writer = std::io::BufWriter::new(file);
        let result = image::codecs::png::PngEncoder::new(&mut writer).write_image(
            &texture.rgba, texture.width, texture.height, image::ExtendedColorType::Rgba8,
        ).and_then(|_| writer.flush().map_err(image::ImageError::IoError));
        drop(writer);
        if let Err(error) = result { let _ = fs::remove_file(&path); return Err(error.to_string()); }
        self.entries.insert(id.clone(), Asset { path, name: texture.name.clone(), original_path: None });
        Ok(id)
    }
    pub fn set_original_path(&mut self, id: &str, path: PathBuf) -> Result<(), String> {
        self.entries.get_mut(id).ok_or("贴图历史已失效")?.original_path = Some(path);
        Ok(())
    }
    pub fn active_original_path(&self) -> Option<PathBuf> {
        self.active.as_ref().and_then(|id| self.entries.get(id)).and_then(|asset| asset.original_path.clone())
    }
    pub fn load(&self, id: &str) -> Result<ColorTexture, String> {
        let asset = self.entries.get(id).ok_or("贴图历史已失效")?;
        let mut texture = load_texture(&asset.path)?;
        texture.name = asset.name.clone();
        Ok(texture)
    }
    pub fn remove(&mut self, id: &str) {
        if let Some(asset) = self.entries.remove(id) { let _ = fs::remove_file(asset.path); }
    }
    pub fn prune(&mut self, keep: &[String]) {
        let keep: HashSet<&str> = keep.iter().map(String::as_str).chain(self.active.as_deref()).collect();
        let remove: Vec<String> = self.entries.keys().filter(|id| !keep.contains(id.as_str())).cloned().collect();
        for id in remove { self.remove(&id); }
    }
}
impl Drop for TextureAssets {
    fn drop(&mut self) {
        for (_, asset) in self.entries.drain() { let _ = fs::remove_file(asset.path); }
        if let Some(path) = self.directory.take() { let _ = fs::remove_dir(path); }
    }
}
