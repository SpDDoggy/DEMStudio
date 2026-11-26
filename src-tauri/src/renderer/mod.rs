mod mesh_export;
mod mesh_appearance;
mod mesh_source;
mod bilinear_shadow_precision;
mod bilinear_refine;
mod smooth_bilinear;
pub mod appearance;
mod appearance_distribution;
pub use appearance_distribution::AppearanceDistribution;
mod appearance_settings;
mod background;
mod water;
pub(crate) use water::validate as validate_water_parameters;
mod fog;
pub(crate) use fog::validate as validate_fog_parameters;
mod cloud;


pub(crate) use cloud::validate as validate_cloud_parameters;
mod ramp;
pub(crate) use appearance_settings::validate as validate_appearance_parameters;
pub mod export_extent;
mod export_layers;
pub mod georef;
pub mod geotiff;
pub mod gl_renderer;
pub mod heightmap;
pub mod native;
#[cfg(target_os = "windows")]
mod resize_snapshot;
pub mod png_output;
pub mod image_output;
pub mod raster;
mod sky;
mod tiff_stream;
mod source_grid;
mod paged_source;
mod source_export_policy;
mod paged_shader;
mod raystate_shader;
mod compact_feedback;
mod active_queue;
mod phase_queue;
mod wavefront_queue;
mod async_samples;
mod async_active_queue;
mod compute_image_layout;


















mod sample_wavefront;
mod wavefront_runtime;
mod sample_wavefront_shader;
mod gpu_batch;
mod bicubic_profile;
mod walk_profile;
mod bicubic_bounds;
mod bicubic_static;
mod bicubic_leaf;
mod bicubic_rows;
mod tight_bounds;
mod geo_merged;
mod ancestor_seed;
mod directional_skip;







mod visibility_bounds;
mod visibility_probe;
mod visibility_diagnostic;

pub mod thumbnail;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct CloudSeedWeight {
    pub seed: u32,
    pub weight: f32,
}

#[derive(Clone, Debug)]
pub struct HeightMap {
    pub width: u32,
    pub height: u32,
    /// Row-major samples in source order: left-to-right, top-to-bottom.
    /// GPU upload converts rows into the terrain's +Y-up coordinates.
    pub values: Vec<f32>,
    /// Source-wide range in values units, never recomputed from a preview.
    pub display_range: [f32; 2],
    /// Source-value extrema, independent of preview sampling and legacy geometry units.
    pub source_elevation_range: Option<[f64; 2]>,
    pub bottom: f32,
    pub span: f32,
    pub name: String,
    pub source: Option<raster::RasterSource>,
}

pub fn initial_height_scale(map: &HeightMap) -> Option<f32> {
    let path=map.source.as_ref().map(|s| s.path.as_path())
        .unwrap_or_else(|| std::path::Path::new(&map.name));
    let extension=path.extension()?.to_str()?;
    (extension.eq_ignore_ascii_case("tif") || extension.eq_ignore_ascii_case("tiff")).then_some(0.2)
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct RenderParams {
    pub appearance_layers_version: u8,
    pub material_enabled: bool,
    pub elevation_enabled: bool,
    pub texture_enabled: bool,
    pub transition_background_from: Option<background::BackgroundState>,
    pub transition_background_mix: f32,
    pub background_mode: String,
    pub background_color: String,
    pub background_color2: String,
    pub background_angle: f32,
    pub background_follow_light: bool,
    pub sky_follow_light: bool,
    pub transition_sky_follow: Option<f32>,
    pub transition_sky_model: Option<f32>,
    pub ground_enabled: bool,
    pub ground_color: String,
    pub ground_shadow: bool,
    pub export_transparent: bool,
    pub layout_shadow_only: bool,
    pub export_normal: bool,
    pub export_depth: bool,
    pub export_shadow_layer: bool,
    pub texture_mapping: String,
    pub texture_fit: String,
    pub texture_scale: f32,
    pub texture_offset_x: f32,
    pub texture_offset_y: f32,
    pub texture_rotation: f32,
    pub texture_blend: String,
    pub material_roughness: f32,
    pub material_specular: f32,
    pub material_specular_color: String,
    pub ramp_stops: Vec<RampStop>,
    pub ramp_offset: f32,
    pub ramp_reverse: bool,
    pub ramp_wrap: bool,
    pub ramp_range_mode: String,
    pub ramp_min: f64,
    pub ramp_max: f64,
    pub appearance_pick: bool,
    pub sun_altitude: f32,
    pub sun_azimuth: f32,
    pub sun_size: f32,
    pub sun_intensity: f32,
    pub sun_color: String,
    pub sky_intensity: f32,
    pub sky_color: String,
    pub sky_model: String,
    pub max_bounces: u32,
    pub experimental_bounces: bool,
    pub water_enabled: bool,
    pub water_level: f32,
    pub water_opacity: f32,
    pub water_color: String,
    pub water_roughness: f32,
    pub fog_enabled: bool,
    pub fog_density: f32,
    pub fog_color: String,
    pub fog_start: f32,
    pub fog_end: f32,
    pub fog_height_enabled: bool,
    pub fog_height_base: f32,
    pub fog_height_falloff: f32,
    pub cloud_enabled: bool,
    pub cloud_shadow_enabled: bool,
    pub cloud_shadow_strength: f32,
    pub cloud_coverage: f32,
    pub cloud_density: f32,
    pub cloud_height: f32,
    pub cloud_thickness: f32,
    pub cloud_scale: f32,
    pub cloud_range: f32,
    pub cloud_offset_x: f32,
    pub cloud_offset_y: f32,
    pub cloud_seed: u32,
    pub transition_cloud_seeds: Vec<CloudSeedWeight>,
    pub cloud_color: String,
    pub camera_yaw: f32,
    pub camera_pitch: f32,
    pub camera_distance: f32,
    /// World-space target offset normalized by the map's longest side.
    pub camera_pan: [f32; 3],
    /// Film-plane offset in shortest-side half-frame units, before FOV scaling.
    pub camera_frame_x: f32,
    pub camera_frame_y: f32,
    pub field_of_view: f32,
    pub exposure: f32,
    pub gamma: f32,
    pub bloom: f32,
    pub scene_exposure: f32,
    pub scene_gamma: f32,
    pub scene_brightness: f32,
    pub scene_contrast: f32,
    pub scene_saturation: f32,
    pub scene_temperature: f32,
    pub scene_shadows: f32,
    pub scene_highlights: f32,
    pub texture_exposure: f32,
    pub texture_contrast: f32,
    pub texture_shadows: f32,
    pub texture_highlights: f32,
    pub texture_saturation: f32,
    pub output_contrast: f32,
    pub output_saturation: f32,
    pub output_temperature: f32,
    pub output_shadows: f32,
    pub output_highlights: f32,
    pub stretch_type: String,
    pub stretch_deviations: f32,
    pub stretch_low: f32,
    pub stretch_high: f32,
    pub contour_enabled: bool,
    pub contour_interval: f32,
    pub contour_width: f32,
    pub contour_smoothing: f32,
    pub contour_color: String,
    pub contour_opacity: f32,
    pub contour_major_every: u32,
    pub contour_major_width: f32,
    pub elevation_blend: String,
    pub height_scale: f32,
    pub height_scale_mode: String,
    pub height_curve: f32,
    pub height_offset: f32,
    pub height_step: f32,
    pub lod: i32,
    pub surface: String,
    pub material_color: String,
    pub appearance_mode: String,
    pub color_ramp: String,
    pub ramp_interpolation: String,
    pub material_opacity: f32,
    pub texture_opacity: f32,
    pub elevation_opacity: f32,
    pub ramp_colors: [String; 5],
    pub width: u32,
    pub height: u32,
    pub samples: u32,
    pub output_bit_depth: u8,
    /// Axial terrain sample resolution; independent of output image dimensions.
    pub export_terrain_scale: String,
    pub projection: String,
    pub grid: bool,
    pub export_shadow: bool,
    pub export_orthographic: bool,
    pub export_georeference: bool,
    pub shadow_padding: f32,
}

impl Default for RenderParams {
    fn default() -> Self {
        Self {
            appearance_layers_version: 0,
            material_enabled: true,
            elevation_enabled: false,
            texture_enabled: false,
            transition_background_from: None,
            transition_background_mix: 1.,
            background_mode: "sky".into(),
            background_color: "#30343b".into(),
            background_color2: "#647180".into(),
            background_angle: 90.,
            background_follow_light: false,
            sky_follow_light: false,
            transition_sky_follow: None,
            transition_sky_model: None,
            ground_enabled: true,
            ground_color: "#767676".into(),
            ground_shadow: true,
            export_transparent: false,
            layout_shadow_only: false,
            export_normal: false,
            export_depth: false,
            export_shadow_layer: false,
            texture_mapping: "aligned".into(),
            texture_fit: "fill".into(),
            texture_scale: 1.,
            texture_offset_x: 0.,
            texture_offset_y: 0.,
            texture_rotation: 0.,
            texture_blend: "overlay".into(),
            material_roughness: 0.5,
            material_specular: 0.,
            material_specular_color: "#ffffff".into(),
            ramp_stops: Vec::new(),
            ramp_offset: 0.,
            ramp_reverse: false,
            ramp_wrap: false,
            ramp_range_mode: "source".into(),
            ramp_min: 0.,
            ramp_max: 1.,
            appearance_pick: false,
            sun_altitude: 45.,
            sun_azimuth: 135.,
            sun_size: 1.,
            sun_intensity: 100.,
            sun_color: "#fff1db".into(),
            sky_intensity: 45.,
            sky_color: "#a9c9e8".into(),
            sky_model: "atmosphere".into(),
            max_bounces: 8,
            experimental_bounces: false,
            water_enabled: false,
            water_level: 20.,
            water_opacity: 80.,
            water_color: "#4c9fa6".into(),
            water_roughness: 0.2,
            fog_enabled: false,
            fog_density: 20.,
            fog_color: "#c8d6df".into(),
            fog_start: 0.,
            fog_end: 10000.,
            fog_height_enabled: false,
            fog_height_base: 0.,
            fog_height_falloff: 20.,
            cloud_enabled: false,
            cloud_shadow_enabled: false,
            cloud_shadow_strength: 50.,
            cloud_coverage: 35.,
            cloud_density: 60.,
            cloud_height: 15.,
            cloud_thickness: 12.,
            cloud_scale: 25.,
            cloud_range: 125.,
            cloud_offset_x: 0.,
            cloud_offset_y: 0.,
            cloud_seed: 7,
            transition_cloud_seeds: Vec::new(),
            cloud_color: "#f3f1e8".into(),
            camera_yaw: 45.,
            camera_pitch: 35.,
            camera_distance: 10.,
            camera_pan: [0.; 3],
            camera_frame_x: 0.,
            camera_frame_y: 0.,
            field_of_view: 45.,
            exposure: 1.,
            gamma: 2.2,
            bloom: 0.,
            scene_exposure: 0.,
            scene_gamma: 2.2,
            scene_brightness: 0.,
            scene_contrast: 0.,
            scene_saturation: 0.,
            scene_temperature: 0.,
            scene_shadows: 0.,
            scene_highlights: 0.,
            texture_exposure: 0.,
            texture_contrast: 0.,
            texture_shadows: 0.,
            texture_highlights: 0.,
            texture_saturation: 0.,
            output_contrast: 0.,
            output_saturation: 0.,
            output_temperature: 0.,
            output_shadows: 0.,
            output_highlights: 0.,
            stretch_type: "none".into(),
            stretch_deviations: 2.,
            stretch_low: 2.,
            stretch_high: 2.,
            contour_enabled: false,
            contour_interval: 5.,
            contour_width: 0.65,
            contour_smoothing: 1.,
            contour_color: "#6b6258".into(),
            contour_opacity: 45.,
            contour_major_every: 5,
            contour_major_width: 1.1,
            elevation_blend: "color".into(),
            height_scale: 1.,
            height_scale_mode: "linear".into(),
            height_curve: 1.,
            height_offset: 0.,
            height_step: 0.,
            lod: 0,
            surface: "bilinear".into(),
            material_color: "#d5d2c9".into(),
            appearance_mode: "material".into(),
            color_ramp: "terrain".into(),
            ramp_interpolation: "continuous".into(),
            material_opacity: 100.,
            texture_opacity: 100.,
            elevation_opacity: 100.,
            ramp_colors: ["#26523b", "#6e9152", "#b3a36e", "#9c856e", "#f0ede3"].map(String::from),
            width: 1920,
            height: 1080,
            samples: 256,
            output_bit_depth: 8,
            export_terrain_scale: "source".into(),
            projection: "perspective".into(),
            grid: true,
            export_shadow: false,
            export_orthographic: false,
            export_georeference: false,
            shadow_padding: 20.,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RampStop {
    pub position: f32,
    pub color: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppearanceSample {
    pub height: f64,
    pub normalized_height: f32,
    pub uv: [f32;2],
    pub range: [f64;2],
}

#[derive(Clone, Deserialize, Debug)]
pub struct OverlayRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ViewportRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub visible: bool,
    #[serde(default)]
    pub overlays: Vec<OverlayRect>,
}

#[derive(Clone, Serialize, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RenderStatus {
    pub appearance_range: Option<[f64;2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_height_scale: Option<f32>,
    pub ready: bool,
    pub frame_presented: bool,
    pub paused: bool,
    pub samples: u32,
    pub target_samples: u32,
    pub preview: bool,
    pub preview_width: u32,
    pub preview_height: u32,
    pub width: u32,
    pub height: u32,
    pub map_name: String,
    pub map_width: u32,
    pub map_height: u32,
    pub source: Option<raster::RasterSource>,
    pub exporting: bool,
    pub export_progress: u32,
    /// Actual rendered pixel-samples, independent of preparation progress.
    pub export_work_progress: Option<f64>,
    pub gl_renderer: String,
    pub gl_version: String,
    pub error: Option<String>,
    pub texture_name: Option<String>,
    pub texture_width: u32,
    pub texture_height: u32,
}

mod page_scheduler;

mod page_prefetch;
