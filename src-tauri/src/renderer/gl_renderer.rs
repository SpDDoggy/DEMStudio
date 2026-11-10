//! Native progressive OpenGL baseline. Progressive heightfield rendering with embedded GLSL.
use super::{appearance::ColorTexture, HeightMap, OverlayRect, RenderParams};
use glow::HasContext;
use std::cell::{Cell, RefCell};
#[path = "stretch.rs"]
mod stretch;

#[path = "sample_job.rs"]
mod sample_job;
use sample_job::SampleJob;
#[path = "paged_gpu.rs"]
mod paged_gpu;
pub use paged_gpu::PagedExport;







const VERT: &str = "#version 330 compatibility\nout vec2 st; uniform vec2 aspect; uniform vec2 pixel_jitter; void main(){vec2 p=vec2((gl_VertexID<<1)&2,gl_VertexID&2); gl_Position=vec4(p*2.-1.,0.,1.);st=(p*2.-1.)*aspect+pixel_jitter;}";
// Presentation only: the exported output texture remains untouched. All mask
// geometry and the blur radius are in physical viewport pixels, independent of
// the temporary interaction render resolution.
const COPY: &str = r#"#version 330 compatibility
uniform sampler2D image;
uniform vec2 output_size, output_origin, view_size;
uniform int overlay_count;
uniform vec4 overlay_rects[16];
void main(){
    vec2 uv=(gl_FragCoord.xy-output_origin)/output_size;
    vec4 sharp=texture(image,uv);
    vec2 point=vec2(gl_FragCoord.x,view_size.y-gl_FragCoord.y);
    // The window chrome frosts our own live image, fading down from its top.
    float mask=1.-smoothstep(0.,min(180.,view_size.y*.25),point.y);
    for(int i=0;i<16;i++){
        if(i>=overlay_count)break;
        vec4 rect=overlay_rects[i];
        float radius=min(14.,min(rect.z,rect.w)*0.5);
        vec2 q=abs(point-(rect.xy+rect.zw*0.5))-(rect.zw*0.5-vec2(radius));
        float distance=length(max(q,vec2(0.)))+min(max(q.x,q.y),0.)-radius;
        mask=mix(mask,.65,1.-smoothstep(-3.,0.,distance));
    }
    if(mask<=0.){gl_FragColor=vec4(sharp.rgb,1.);return;}
    vec2 step_uv=vec2(16.*mask)/output_size;
    vec4 blurred=sharp*4.;
    blurred+=(texture(image,uv+vec2(step_uv.x,0.))+texture(image,uv-vec2(step_uv.x,0.))
             +texture(image,uv+vec2(0.,step_uv.y))+texture(image,uv-vec2(0.,step_uv.y)))*2.;
    blurred+=texture(image,uv+step_uv)+texture(image,uv-step_uv)
             +texture(image,uv+vec2(step_uv.x,-step_uv.y))
             +texture(image,uv+vec2(-step_uv.x,step_uv.y));
    gl_FragColor=vec4(mix(sharp,blurred/16.,mask).rgb,1.);
}"#;

const MAX_GLASS_RECTS: usize = 16;

/// Cover preserves aspect while cropping excess equally at both sides. Signed
/// origins deliberately support views narrower or shorter than the image.
fn cover_mapping(view: [u32; 2], image: [u32; 2]) -> ([f32; 2], [f32; 2]) {
    let scale =
        (view[0] as f64 / image[0].max(1) as f64).max(view[1] as f64 / image[1].max(1) as f64);
    let size = [image[0] as f64 * scale, image[1] as f64 * scale];
    (
        [size[0] as f32, size[1] as f32],
        [
            ((view[0] as f64 - size[0]) * 0.5) as f32,
            ((view[1] as f64 - size[1]) * 0.5) as f32,
        ],
    )
}

fn glass_uniforms(overlays: &[OverlayRect], view: [u32; 2]) -> Vec<f32> {
    overlays
        .iter()
        .filter(|r| {
            r.width > 0
                && r.height > 0
                && (r.x as i64) < view[0] as i64
                && (r.y as i64) < view[1] as i64
                && r.x as i64 + r.width as i64 > 0
                && r.y as i64 + r.height as i64 > 0
        })
        .take(MAX_GLASS_RECTS)
        .flat_map(|r| [r.x as f32, r.y as f32, r.width as f32, r.height as f32])
        .collect()
}

#[derive(Clone, Copy)]
struct MapGeometry {
    width: u32,
    height: u32,
    span: f32,
    source_elevation: [f64; 2],
    source_size: [u32; 2],
}

pub struct GlRenderer {
    gl: glow::Context,
    path: glow::Program,
    post: glow::Program,
    color_post: glow::Program,
    output_posts: [glow::Program; 2],
    elevation_post: glow::Program,
    elevation_output_post: glow::Program,
    copy: glow::Program,
    stretch_program: glow::Program,
    surface_info: glow::Texture,
    surface_fbo: glow::Framebuffer,
    filtered: glow::Texture,
    filtered_fbo: glow::Framebuffer,
    stats_tex: glow::Texture,
    stats_fbo: glow::Framebuffer,
    lut_tex: glow::Texture,
    surface_ready: Cell<bool>,
    output_dirty: Cell<bool>,
    stretch_lut: RefCell<Option<Vec<u8>>>,
    stretch_count: Cell<u32>,
    stretch_export: Cell<bool>,
    stretch_frozen: Cell<bool>,
    vao: glow::VertexArray,
    height_tex: glow::Texture,
    contour_tex: glow::Texture,
    contour_ready: Cell<bool>,
    // Unwarped, display-normalized texels. Curves are always rebuilt from this
    // base so toggling modes never modifies the source or compounds rounding.
    height_base: Vec<f32>,
    paged_geometry: bool,
    paged_coarse_cache: Option<paged_gpu::CoarseCache>,
    rand_tex: glow::Texture,
    color_tex: glow::Texture,
    sky_tex: glow::Texture,
    sky_altitude: Option<f32>,
    sky_mean: [f32; 3],
    color_size: [u32; 2],
    accum: glow::Texture,
    output: glow::Texture,
    accum_fbo: glow::Framebuffer,
    output_fbo: glow::Framebuffer,
    params: RenderParams,
    map: Option<MapGeometry>,
    geo_tile: Option<[u32; 4]>,
    geo_grid: Option<[u32; 4]>,
    output_tile: Option<([u32; 2], [u32; 4])>,
    padded: u32,
    levels: u32,
    count: u32,
    sample_job: SampleJob,
    tile_fence: Option<glow::Fence>,
    tile_discarded: bool,
    rng: [u32; 4],
    driver_info: (String, String),
}

impl GlRenderer {
    pub unsafe fn new(gl: glow::Context) -> Result<Self, String> {
        let driver_info = (
            gl.get_parameter_string(glow::RENDERER),
            gl.get_parameter_string(glow::VERSION),
        );
        let params = RenderParams::default();
        let path = program(&gl, &path_source(&params, false))?;
        let post = program(&gl, &post_source(false))?;
        let color_post = program(&gl, &post_source(true))?;
        let output_posts = [
            program(&gl, &output_post_source(false))?,
            program(&gl, &output_post_source(true))?,
        ];
        let elevation_post=program(&gl,&elevation_post_source())?;
        let elevation_output_post=program(&gl,&filter_post_source(elevation_post_source()))?;
        let copy = program(&gl, COPY)?;
        let stretch_program = program(&gl, &stretch_post_source())?;
        let surface_info = gl.create_texture()?;
        let surface_fbo = gl.create_framebuffer()?;
        let filtered = gl.create_texture()?;
        let filtered_fbo = gl.create_framebuffer()?;
        let stats_tex = gl.create_texture()?;
        let stats_fbo = gl.create_framebuffer()?;
        let lut_tex = gl.create_texture()?;
        let vao = gl.create_vertex_array()?;
        let height_tex = gl.create_texture()?;
        let contour_tex = gl.create_texture()?;
        let rand_tex = gl.create_texture()?;
        let color_tex = gl.create_texture()?;
        let sky_tex = gl.create_texture()?;
        let accum = gl.create_texture()?;
        let output = gl.create_texture()?;
        let accum_fbo = gl.create_framebuffer()?;
        let output_fbo = gl.create_framebuffer()?;
        let mut s = Self {
            gl,
            path,
            post,
            color_post,
            output_posts, elevation_post, elevation_output_post,
            copy,
            stretch_program, surface_info, surface_fbo, filtered, filtered_fbo, stats_tex, stats_fbo, lut_tex,
            surface_ready: Cell::new(false), output_dirty: Cell::new(false), stretch_lut: RefCell::new(None), stretch_count: Cell::new(0),
            stretch_export: Cell::new(false), stretch_frozen: Cell::new(false),
            vao,
            height_tex,
            contour_tex,
            contour_ready: Cell::new(false),
            height_base: Vec::new(),
            paged_geometry: false,
            paged_coarse_cache: None,
            rand_tex,
            color_tex,
            sky_tex,
            sky_altitude: None,
            sky_mean: [1.; 3],
            color_size: [1, 1],
            accum,
            output,
            accum_fbo,
            output_fbo,
            params,
            map: None,
            geo_tile: None,
            geo_grid: None,
            output_tile: None,
            padded: 1,
            levels: 0,
            count: 0,
            sample_job: SampleJob::default(),
            tile_fence: None,
            tile_discarded: false,
            rng: [123456789, 362436069, 521288629, 88675123],
            driver_info,
        };
        s.gl.bind_vertex_array(Some(vao));
        let noise: Vec<f32> = (0..256 * 256).map(|_| s.random()).collect();
        s.gl.bind_texture(glow::TEXTURE_2D, Some(rand_tex));
        s.gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::R32F as i32,
            256,
            256,
            0,
            glow::RED,
            glow::FLOAT,
            glow::PixelUnpackData::Slice(Some(bytes(&noise))),
        );
        s.texture_filter(glow::NEAREST, glow::REPEAT);
        s.gl.bind_texture(glow::TEXTURE_2D,Some(contour_tex));
        s.texture_filter(glow::LINEAR,glow::CLAMP_TO_EDGE);
        s.gl.tex_image_2d(glow::TEXTURE_2D,0,glow::RG32F as i32,1,1,0,glow::RG,glow::FLOAT,
            glow::PixelUnpackData::Slice(Some(bytes(&[0.,0.]))));
        s.gl.bind_texture(glow::TEXTURE_2D, Some(color_tex));
        s.gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
            1,
            1,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(Some(&[0, 0, 0, 0])),
        );
        s.texture_filter(glow::NEAREST, glow::CLAMP_TO_EDGE);
        s.gl.bind_texture(glow::TEXTURE_2D, Some(sky_tex));
        s.gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGB32F as i32, 1, 1, 0,
            glow::RGB, glow::FLOAT, glow::PixelUnpackData::Slice(Some(bytes(&[1_f32; 3]))));
        s.texture_filter(glow::LINEAR, glow::CLAMP_TO_EDGE);
        s.allocate()?;
        Ok(s)
    }

    pub unsafe fn upload_color(&mut self, texture: &ColorTexture) -> Result<(), String> {
        let max = self.gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE) as u32;
        if texture.width == 0
            || texture.height == 0
            || texture.width > max
            || texture.height > max
            || texture.rgba.len() != texture.width as usize * texture.height as usize * 4
        {
            return Err("贴图尺寸或像素数据无效".into());
        }
        let pixels = color_texture_rows(texture);
        let uploaded = self.gl.create_texture()?;
        self.gl.bind_texture(glow::TEXTURE_2D, Some(uploaded));
        self.gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::SRGB8_ALPHA8 as i32,
            texture.width as i32,
            texture.height as i32,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(Some(&pixels)),
        );
        self.texture_filter(glow::LINEAR, glow::CLAMP_TO_EDGE);
        if let Err(error) = self.check("上传贴图") {
            self.gl.delete_texture(uploaded);
            return Err(error);
        }
        let old = std::mem::replace(&mut self.color_tex, uploaded);
        self.gl.delete_texture(old);
        self.color_size = [texture.width, texture.height];
        self.reset();
        Ok(())
    }

    pub unsafe fn clear_color(&mut self) -> Result<(), String> {
        self.upload_color(&ColorTexture {
            width: 1,
            height: 1,
            rgba: vec![0, 0, 0, 0],
            name: String::new(),
        })?;
        self.params.appearance_mode = "material".into();
        self.reset();
        Ok(())
    }

    fn random(&mut self) -> f32 {
        let t = self.rng[0] ^ (self.rng[0] << 11);
        self.rng = [
            self.rng[1],
            self.rng[2],
            self.rng[3],
            self.rng[3] ^ (self.rng[3] >> 19) ^ t ^ (t >> 8),
        ];
        (self.rng[3] & 0x7fffff) as f32 / 8388608.
    }

    pub unsafe fn upload_map(&mut self, map: &HeightMap) -> Result<(), String> {
        if map.width < 2
            || map.height < 2
            || map.values.len() != map.width as usize * map.height as usize
            || map
                .values
                .iter()
                .any(|v| !v.is_finite() || *v < 0. || *v > 1.)
            || !map.span.is_finite()
            || map.span <= 0.
        {
            return Err("无效的归一化高度图".into());
        }
        let size = map
            .width
            .max(map.height)
            .checked_next_power_of_two()
            .ok_or("高度图尺寸溢出")?;
        if size > self.gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE) as u32 {
            return Err("高度图超过 GPU 纹理尺寸上限".into());
        }
        self.padded = size;
        self.paged_geometry = false;
        self.levels = size.ilog2();
        let mut data = padded_height_texture(map, size);
        for value in &mut data {
            *value = display_sample(*value, map.display_range);
        }
        self.contour_ready.set(false);
        self.height_base = data;
        self.upload_height_levels(effective_height_curve(&self.params));
        self.map = Some(MapGeometry {
            width: map.width,
            height: map.height,
            span: map.width.max(map.height) as f32 * 0.2,
            source_elevation: source_elevation_range(map),
            source_size:map.source.as_ref().map(|v|[v.width,v.height]).unwrap_or([map.width,map.height]),
        });
        self.reset();
        self.check("上传高度图")
    }

    unsafe fn upload_height_levels(&self, curve: f32) {
        let mut data = curved_height_texture(&self.height_base, curve);
        self.gl
            .bind_texture(glow::TEXTURE_2D, Some(self.height_tex));
        self.texture_filter(glow::NEAREST_MIPMAP_NEAREST, glow::CLAMP_TO_EDGE);
        self.gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MAG_FILTER,
            glow::NEAREST as i32,
        );
        self.gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MAX_LEVEL,
            self.levels as i32,
        );
        let mut n = self.padded;
        for level in 0..=self.levels {
            self.gl.tex_image_2d(
                glow::TEXTURE_2D,
                level as i32,
                glow::R32F as i32,
                n as i32,
                n as i32,
                0,
                glow::RED,
                glow::FLOAT,
                glow::PixelUnpackData::Slice(Some(bytes(&data))),
            );
            if n == 1 {
                break;
            }
            let next = n / 2;
            let reduced = conservative_mip(&data, n);
            data = reduced;
            n = next;
        }
    }

    pub unsafe fn heightmap_edge_limit(&self) -> u32 {
        (self.gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE) as u32)
            .min(super::raster::MAX_DECODE_EDGE)
    }

    /// Apply display edits to the actual integrated scene, even if a newer
    /// camera/geometry update is deferred until the in-flight strip completes.
    pub unsafe fn set_display_params(&mut self,p: &RenderParams) -> Result<(),String> {
        let mut next=self.params.clone();
        copy_display_fields(p,&mut next);
        self.set_params(&next)
    }

    pub unsafe fn set_params(&mut self, p: &RenderParams) -> Result<(), String> {
        if p.fog_density != 0. || p.bloom != 0. {
            return Err("OpenGL 基线尚未启用雾和 Bloom".into());
        }
        if !["cube", "poly", "bilinear", "bicubic"].contains(&p.surface.as_str()) {
            return Err("未知表面类型".into());
        }
        if !["perspective", "orthographic", "isometric"].contains(&p.projection.as_str()) {
            return Err("未知投影类型".into());
        }
        let floats = [
            p.sun_altitude,
            p.sun_azimuth,
            p.sun_size,
            p.sun_intensity,
            p.sky_intensity,
            p.camera_yaw,
            p.camera_pitch,
            p.camera_distance,
            p.field_of_view,
            p.exposure,
            p.gamma,
            p.height_scale,
            p.height_curve,
            p.height_offset,
            p.height_step,
        ];
        if floats
            .iter()
            .chain(p.camera_pan.iter())
            .any(|v| !v.is_finite())
            || !matches!(p.output_bit_depth, 8 | 16)
            || p.width == 0
            || p.height == 0
            || p.gamma <= 0.
            || p.height_scale <= 0.
            || !["linear", "accelerated", "nonlinear"].contains(&p.height_scale_mode.as_str())
            || !(0.01..=1000.).contains(&p.height_curve)
            || (p.height_scale_mode != "nonlinear" && p.height_curve != 1.)
            || !(0. ..=100.).contains(&p.height_offset)
            || p.camera_distance <= 0.
            || p.field_of_view <= 0.
            || p.field_of_view >= 175.
            || p.height_step < 0.
            || p.sun_size <= 0.
            || !(0. ..=90.).contains(&p.sun_altitude)
            || p.exposure < 0.
            || p.sun_intensity < 0.
            || p.sky_intensity < 0.
            || !["constant", "atmosphere"].contains(&p.sky_model.as_str())
            || !(1..=if p.experimental_bounces {100} else {15}).contains(&p.max_bounces)
            || !p.shadow_padding.is_finite()
            || !(1. ..=200.).contains(&p.shadow_padding)
        {
            return Err("渲染参数超出有效范围".into());
        }
        for c in [&p.sun_color, &p.sky_color, &p.material_color] {
            parse_color(c)?;
        }
        if output_adjustments(p)
            .iter()
            .any(|v| !v.is_finite() || !(-1. ..=1.).contains(v))
        {
            return Err("导出后处理参数超出范围".into());
        }
        validate_display_parameters(p)?;
        validate_contour_parameters(p)?;
        let max = self.gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE) as u32;
        if p.width > max || p.height > max {
            return Err("输出尺寸超过 GPU 限制".into());
        }
        if self.params == *p {
            return Ok(());
        }
        if same_scene_without_output_filter(&self.params, p) {
            self.params = p.clone();
            if !self.stretch_frozen.get() { self.stretch_lut.replace(None); }
            // Before the first integrated sample, a metadata pass would alter
            // path jitter while a bicubic strip is still in flight. The normal
            // sample completion applies these display parameters once safely.
            if self.count > 0 && !self.paged_geometry { self.postprocess(); }
            return Ok(());
        }
        let old = &self.params;
        if old.surface != p.surface
            || old.experimental_bounces != p.experimental_bounces
            || old.projection != p.projection
            || old.grid != p.grid
            || old.export_shadow != p.export_shadow
            || (old.height_step > 0.) != (p.height_step > 0.)
        {
            let new = program(&self.gl, &path_source(p, self.geo_tile.is_some()))?;
            self.gl.delete_program(self.path);
            self.path = new;
        }
        let resize = old.width != p.width || old.height != p.height
            || old.output_bit_depth != p.output_bit_depth
            || (old.stretch_type != "none") != (p.stretch_type != "none")
            || needs_surface(old) != needs_surface(p);
        let curve_changed = effective_height_curve(old) != effective_height_curve(p);
        if curve_changed && !self.height_base.is_empty() && !self.paged_geometry {
            self.upload_height_levels(effective_height_curve(p));
            self.check("更新高度曲线")?;
        }
        self.params = p.clone();
        if resize {
            self.allocate()?;
        } else {
            self.reset();
        }
        Ok(())
    }

    /// Tile coordinates are in the original raster's top-to-bottom pixel grid.
    /// A virtual replicated one-pixel halo leaves the physical GPU texture unchanged.
    pub unsafe fn set_geo_tile(&mut self, tile: Option<[u32; 4]>) -> Result<(), String> {
        if let Some([x, y, w, h]) = tile {
            let map = self.map.ok_or("尚未导入高度图")?;
            let [sw, sh, left, top] = self.geo_grid.unwrap_or([map.width, map.height, 0, 0]);
            if w == 0
                || h == 0
                || x.checked_add(w)
                    .is_none_or(|v| u64::from(v) > u64::from(sw) + 2 * u64::from(left))
                || y.checked_add(h)
                    .is_none_or(|v| u64::from(v) > u64::from(sh) + 2 * u64::from(top))
            {
                return Err("正射分块范围无效".into());
            }
        }
        if self.geo_tile.is_some() != tile.is_some() {
            let next = program(&self.gl, &path_source(&self.params, tile.is_some()))?;
            self.gl.delete_program(self.path);
            self.path = next;
        }
        if self.geo_tile != tile {
            self.geo_tile = tile;
            self.reset();
        }
        if tile.is_none() {
            self.geo_grid = None;
        }
        Ok(())
    }

    /// Source output grid and its integer pixel margin, independent of GPU geometry.
    pub unsafe fn set_geo_grid(&mut self, grid: Option<[u32; 4]>) -> Result<(), String> {
        if grid.is_some_and(|g| g[0] == 0 || g[1] == 0) {
            return Err("正射源网格无效".into());
        }
        if self.geo_grid != grid {
            self.geo_grid = grid;
            self.reset();
        }
        Ok(())
    }

    /// Render a camera-image tile without changing the full-frame projection.
    pub unsafe fn set_output_tile(
        &mut self,
        tile: Option<([u32; 2], [u32; 4])>,
    ) -> Result<(), String> {
        if let Some(([width, height], [x, y, w, h])) = tile {
            if width == 0
                || height == 0
                || w != self.params.width
                || h != self.params.height
                || x.checked_add(w).is_none_or(|v| v > width)
                || y.checked_add(h).is_none_or(|v| v > height)
            {
                return Err("PNG 相机分块尺寸无效".into());
            }
            // All tiles use the same per-sample light/jitter sequence. The noise
            // texture below is addressed with global pixels, avoiding tile seams.
            self.rng = [123456789, 362436069, 521288629, 88675123];
        }
        self.output_tile = tile;
        self.reset();
        Ok(())
    }

    pub unsafe fn render_sample(&mut self) -> Result<bool, String> {
        if self.map.is_none() || self.count >= self.params.samples {
            return Ok(false);
        }
        // Explicit export/capture is a queue barrier and needs a complete sample.
        while !self.render_sample_step()? {}
        Ok(true)
    }

    /// A single bounded submission/poll. False means no complete new sample yet.
    /// The window worker checks commands between calls, so obsolete frames never
    /// build up as an entire frame's worth of GPU work in the command queue.
    pub unsafe fn render_sample_step(&mut self) -> Result<bool, String> {
        self.render_sample_step_mode(false)
    }

    /// Offline export keeps every original path sample and small GPU draw, but
    /// batches up to four strips per fence and defers display work until readback.
    /// Callers can check cancellation between these bounded batches.
    pub unsafe fn render_export_sample_step(&mut self) -> Result<bool, String> {
        self.render_sample_step_mode(true)
    }

    unsafe fn render_sample_step_mode(&mut self, export: bool) -> Result<bool, String> {
        if self.map.is_none() || self.count >= self.params.samples {
            return Ok(false);
        }
        if let Some(fence) = self.tile_fence {
            match self.gl.client_wait_sync(fence, 0, 1_000_000) {
                glow::TIMEOUT_EXPIRED => return Ok(false),
                glow::WAIT_FAILED => return Err("等待渲染分块失败".into()),
                _ => {}
            }
            self.gl.delete_sync(fence);
            self.tile_fence = None;
            let discarded = std::mem::take(&mut self.tile_discarded);
            if !discarded && self.sample_job.complete(self.params.height) {
                self.finish_sample(export)?;
                return Ok(true);
            }
        }
        if self.sample_job.row == 0 {
            // Random uniforms advance once per full sample, never per strip.
            self.prepare_sample()?;
        }
        let tiled = self.params.surface == "bicubic";
        let batch = self
            .sample_job
            .next_batch(self.params.width, self.params.height, tiled, export)
            .ok_or("渲染分块状态无效")?;
        let end = batch.end;
        self.begin_sample_batch();
        for (y, rows) in batch { self.draw_sample_strip(y, rows); }
        self.end_sample_batch();
        self.sample_job.submit(end);
        if tiled {
            self.tile_fence = Some(self.gl.fence_sync(glow::SYNC_GPU_COMMANDS_COMPLETE, 0)?);
            self.gl.flush();
            self.check("提交渲染分块")?;
            Ok(false)
        } else {
            self.sample_job.complete(self.params.height);
            self.finish_sample(export)?;
            Ok(true)
        }
    }

    unsafe fn finish_sample(&mut self, export: bool) -> Result<(), String> {
        self.sample_job.reset();
        self.count += 1;
        self.output_dirty.set(true);
        if !export { self.postprocess(); }
        self.check("路径采样")
    }

    unsafe fn begin_sample_batch(&self) {
        self.gl.use_program(Some(self.path));
        self.gl.bind_vertex_array(Some(self.vao));
        self.gl
            .bind_framebuffer(glow::FRAMEBUFFER, Some(self.accum_fbo));
        // Keep full-frame viewport/interpolants; scissor only limits coverage.
        self.gl
            .viewport(0, 0, self.params.width as i32, self.params.height as i32);
        // Present/postprocess bind other textures between samples; restore the
        // path samplers once per batch without changing their random uniforms.
        self.bind_tex(self.path, "mipmap", 0, self.height_tex);
        self.bind_tex(self.path, "tex_rand", 1, self.rand_tex);
        self.bind_tex(self.path, "tex_color", 2, self.color_tex);
        self.bind_tex(self.path, "tex_sky_model", 3, self.sky_tex);
        self.gl.enable(glow::SCISSOR_TEST);
        self.gl.enable(glow::BLEND);
        let alpha = 1. / (self.count + 1) as f32;
        self.gl.blend_color(alpha, alpha, alpha, alpha);
        self.gl.blend_equation(glow::FUNC_ADD);
        self.gl
            .blend_func(glow::CONSTANT_ALPHA, glow::ONE_MINUS_CONSTANT_ALPHA);
    }

    unsafe fn draw_sample_strip(&self, y: u32, rows: u32) {
        self.gl.scissor(0, y as i32, self.params.width as i32, rows as i32);
        self.gl.draw_arrays(glow::TRIANGLES, 0, 3);
    }

    unsafe fn end_sample_batch(&self) {
        self.gl.disable(glow::SCISSOR_TEST);
        self.gl.disable(glow::BLEND);
    }

    // During a frozen export tile only the random sequence changes per sample.
    // Keep this shared with the ordinary path so call order remains identical.
    unsafe fn advance_sample_uniforms(&mut self, pr: glow::Program, p: &RenderParams) {
        let (full, _) = output_tile_mapping(self.output_tile, [p.width, p.height]);
        let shortest = full[0].min(full[1]);
        let jitter = if self.geo_tile.is_some() { [0., 0.] } else {
            [(self.random()-0.5)*2./shortest, (self.random()-0.5)*2./shortest]
        };
        self.v2(pr, "pixel_jitter", jitter);
        let segments=if p.experimental_bounces {101} else {16};
        let dices: Vec<f32> = (0..segments*4).map(|_| self.random()).collect();
        let lobes: Vec<f32> = (0..segments*2).map(|_| self.random()).collect();
        self.gl.uniform_4_f32_slice(self.gl.get_uniform_location(pr, "dices[0]").as_ref(), &dices);
        self.gl.uniform_2_f32_slice(self.gl.get_uniform_location(pr, "lobes[0]").as_ref(), &lobes);
        let sun_r = [self.random(), self.random()];
        self.v2(pr, "sun_r", sun_r);
        let env_r = self.random();
        self.f(pr, "env_r", env_r);
    }

    unsafe fn prepare_sample(&mut self) -> Result<(), String> {
        let p = self.params.clone();
        let map = self.map.as_ref().unwrap();
        let (map_width, map_height, map_span) = (map.width, map.height, map.span);
        let pr = self.path;
        self.gl.use_program(Some(pr));
        self.i(pr, "surface_pass", 0);
        self.i(pr, "appearance_color_blend", i32::from(elevation_color_blend(&p)));
        self.i(
            pr,
            "appearance_discrete",
            i32::from(p.ramp_interpolation == "discrete"),
        );
        self.gl.bind_vertex_array(Some(self.vao));
        self.gl
            .bind_framebuffer(glow::FRAMEBUFFER, Some(self.accum_fbo));
        self.gl.viewport(0, 0, p.width as i32, p.height as i32);
        self.gl.disable(glow::DEPTH_TEST);
        self.gl.disable(glow::CULL_FACE);
        self.gl.disable(glow::FRAMEBUFFER_SRGB);
        self.bind_tex(pr, "mipmap", 0, self.height_tex);
        self.bind_tex(pr, "tex_rand", 1, self.rand_tex);
        self.bind_tex(pr, "tex_color", 2, self.color_tex);
        let (full, origin) = output_tile_mapping(self.output_tile, [p.width, p.height]);
        self.v2(pr, "output_full_size", full);
        self.v2(pr, "output_tile_origin", origin);
        self.i(pr, "output_tiled", i32::from(self.output_tile.is_some()));
        let shortest = full[0].min(full[1]);
        self.v2(pr, "aspect", [full[0] / shortest, full[1] / shortest]);
        self.advance_sample_uniforms(pr, &p);
        self.v2(pr, "tex_rand_inv_size", [1. / 256., 1. / 256.]);
        self.v4(pr, "jitter_mult", [0.7548777, 0.5698403, 0.4382891, 0.328173]);
        let logical_width = map_width + if self.geo_tile.is_some() { 2 } else { 0 };
        let logical_height = map_height + if self.geo_tile.is_some() { 2 } else { 0 };
        self.v2(pr, "geo_source_size", [map_width as f32, map_height as f32]);
        self.i(
            pr,
            "geo_shadow_catcher",
            i32::from(self.geo_tile.is_some() && p.export_shadow),
        );
        self.v2(pr, "bc_source_size", [map_width as f32, map_height as f32]);
        self.v2(
            pr,
            "bc_source_offset",
            if self.geo_tile.is_some() {
                [1., 1.]
            } else {
                [0., 0.]
            },
        );
        let vertical = map_span * p.height_scale;
        let offset = height_offset(map_width, map_height, &p);
        let zmin = offset.min(0.) - 0.01;
        let zmax = vertical + offset + p.height_step * p.height_scale + 0.01;
        let shadow_export = self.geo_tile.is_some() && p.export_shadow;
        self.v3(
            pr,
            "vol_bound[0]",
            if shadow_export {
                [0.5, 0.5, zmin]
            } else {
                [0., 0., zmin]
            },
        );
        self.v3(
            pr,
            "vol_bound[1]",
            if shadow_export {
                [
                    map_width as f32 + 0.5,
                    map_height as f32 + 0.5,
                    zmax.max(0.01),
                ]
            } else {
                [logical_width as f32, logical_height as f32, zmax.max(0.01)]
            },
        );
        self.v2(pr, "mipmap_tex_inv_size", [1. / self.padded as f32; 2]);
        self.v4(
            pr,
            "mipmap_size",
            [
                logical_width as f32,
                logical_height as f32,
                vertical,
                offset,
            ],
        );
        self.v2(
            pr,
            "mipmap_lod",
            [
                if self.paged_geometry || self.geo_tile.is_some() || p.surface == "bicubic" {
                    0.
                } else {
                    p.lod.clamp(0, self.levels as i32) as f32
                },
                self.levels as f32,
            ],
        );
        self.f(
            pr,
            "mipmap_step",
            (p.height_step * p.height_scale).max(0.0001),
        );
        self.v4(pr, "K_diffuse", {
            let c = material_albedo(&p)?;
            [c[0], c[1], c[2], 1.]
        });
        self.v4(pr, "K_ground", [0.18, 0.18, 0.18, 1.]);
        self.i(
            pr,
            "appearance_mode",
            match p.appearance_mode.as_str() {
                "texture" => 1,
                "elevation" => 2,
                _ => 0,
            },
        );
        self.i(
            pr,
            "appearance_ramp",
            match p.color_ramp.as_str() {
                "grayscale" => 1,
                "spectral" => 2,
                "custom" => 3,
                _ => 0,
            },
        );
        let opacity = appearance_opacity(&p);
        self.f(pr, "appearance_opacity", opacity);
        self.v3(
            pr,
            "appearance_linear_base",
            parse_srgb_color(&p.material_color)?,
        );
        for (index, color) in p.ramp_colors.iter().enumerate() {
            self.v3(
                pr,
                &format!("appearance_colors[{index}]"),
                parse_color(color)?,
            );
        }
        self.v2(
            pr,
            "appearance_extent",
            [map_width as f32, map_height as f32],
        );
        self.v2(
            pr,
            "appearance_texture_size",
            self.color_size.map(|v| v as f32),
        );
        self.v2(pr, "appearance_height", [offset, vertical]);
        self.f(pr, "height_curve", effective_height_curve(&p));
        self.f(
            pr,
            "appearance_origin",
            if self.geo_tile.is_some() { 1. } else { 0. },
        );
        self.v2(pr, "tex_color_inv_size", [1., 1.]);
        self.v2(pr, "cmap_scale", [1., 1.]);
        self.v2(pr, "cmap_offset", [0., 0.]);
        self.f(pr, "cmap_gamma", 1.);
        if p.sky_model == "atmosphere" && self.sky_altitude != Some(p.sun_altitude) {
            let lut = super::sky::SkyLut::new(p.sun_altitude);
            self.gl.active_texture(glow::TEXTURE0 + 3);
            self.gl.bind_texture(glow::TEXTURE_2D, Some(self.sky_tex));
            self.gl.tex_image_2d(
                glow::TEXTURE_2D, 0, glow::RGB32F as i32,
                super::sky::SKY_WIDTH as i32, super::sky::SKY_HEIGHT as i32, 0,
                glow::RGB, glow::FLOAT, glow::PixelUnpackData::Slice(Some(bytes(&lut.pixels))),
            );
            self.texture_filter(glow::LINEAR, glow::CLAMP_TO_EDGE);
            self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::REPEAT as i32);
            self.sky_altitude = Some(lut.altitude);
            self.sky_mean = lut.mean_rgb;
        }
        self.i(pr, "sky_model", i32::from(p.sky_model == "atmosphere"));
        self.f(pr, "sky_azimuth", p.sun_azimuth.to_radians());
        self.i(pr, "max_num_diffuse", p.max_bounces as i32);
        self.i(pr, "max_num_specular", 0);
        self.i(pr, "max_num_scatter", 0);
        self.f(pr, "clamp_L", 10.);
        self.v2(pr, "light_mask_sun", [1., 1.]);
        self.v2(pr, "light_mask_sky", [0., 1.]);
        self.v4(pr, "surf_mask_diffuse", [1., 1., 0., 0.]);
        let sky = parse_color(&p.sky_color)?.map(|v| v * p.sky_intensity / 100. * 0.1);
        self.v3(pr, "Le", sky);
        self.v3(pr, "sky_mean_radiance", std::array::from_fn(|i|
            sky[i] * if p.sky_model == "atmosphere" {self.sky_mean[i]} else {1.}));
        let transmission = if p.sky_model == "atmosphere" {
            super::sky::direct_sun_transmittance(p.sun_altitude)
        } else { [1.; 3] };
        let sun_color = parse_color(&p.sun_color)?;
        let solar_cosine=super::sky::sun_mean_cosine(p.sun_altitude,p.sun_size);
        self.v3(pr,"sun_mean_radiance",std::array::from_fn(|i|
            sun_color[i]*transmission[i]*p.sun_intensity/100.*0.4*solar_cosine/std::f32::consts::PI));
        self.v3(
            pr,
            "sun_I",
            std::array::from_fn(|i| sun_color[i] * transmission[i] * p.sun_intensity / 100. * 0.4),
        );
        let az = p.sun_azimuth.to_radians();
        let alt = p.sun_altitude.to_radians();
        let sun = [alt.cos() * az.cos(), alt.cos() * az.sin(), alt.sin()];
        self.matrix(pr, "sun_rot", basis(sun));
        self.f(
            pr,
            "sun_cmax",
            (p.sun_size.clamp(0.01, 90.).to_radians() * 0.5).cos(),
        );
        let dim = map_width.max(map_height) as f32;
        let grid = dim / 10.;
        self.v4(pr, "parm_grid", [grid * 0.006, grid, 0., 1.]);
        self.v4(pr, "ground", [0., 0., 0., 0.]);
        self.v3(pr, "K_grid", [0.08, 0.08, 0.08]);
        if let Some(tile) = self.geo_tile {
            let grid = self.geo_grid.unwrap_or([map_width, map_height, 0, 0]);
            let (origin, pixel_step) = geo_grid_mapping(
                [map_width, map_height],
                grid,
                tile,
                [p.width, p.height],
                zmax.max(0.01) + 1.,
            );
            self.v3(pr, "view_origin", origin);
            self.v2(pr, "geo_pixel_step", pixel_step);
            self.matrix(pr, "view_rot", [1., 0., 0., 0., 1., 0., 0., 0., 1.]);
            self.f(pr, "view_fov", shortest * 0.5);
        } else {
            let iso = p.projection == "isometric";
            let pitch = if iso {
                35.26439
            } else {
                p.camera_pitch.clamp(0., 90.)
            }
            .to_radians();
            let yaw = p.camera_yaw.to_radians();
            let back = [
                yaw.cos() * pitch.cos(),
                yaw.sin() * pitch.cos(),
                pitch.sin(),
            ];
            let rot = camera_basis(yaw, pitch);
            let distance = dim * 1.4 * p.camera_distance / 10.;
            let target = camera_target(map_width, map_height, vertical, &p);
            self.v3(
                pr,
                "view_origin",
                [
                    target[0] + back[0] * distance,
                    target[1] + back[1] * distance,
                    target[2] + back[2] * distance,
                ],
            );
            self.matrix(pr, "view_rot", rot);
            let fov = (p.field_of_view.to_radians() * 0.5).tan();
            self.f(
                pr,
                "view_fov",
                if p.projection == "perspective" {
                    fov
                } else {
                    distance * fov
                },
            );
        }
        Ok(())
    }

    pub unsafe fn present(
        &self,
        view_width: u32,
        view_height: u32,
        overlays: &[OverlayRect],
    ) -> Result<(), String> {
        if view_width == 0 || view_height == 0 {
            return Ok(());
        }
        self.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        self.gl
            .viewport(0, 0, view_width as i32, view_height as i32);
        self.gl.clear_color(0.075, 0.078, 0.085, 1.0);
        self.gl.clear(glow::COLOR_BUFFER_BIT);
        let (size, origin) = cover_mapping(
            [view_width, view_height],
            [self.params.width, self.params.height],
        );
        let glass = glass_uniforms(overlays, [view_width, view_height]);
        self.gl.disable(glow::BLEND);
        self.gl.use_program(Some(self.copy));
        self.gl.bind_vertex_array(Some(self.vao));
        self.bind_tex(self.copy, "image", 0, if self.params.stretch_type != "none" { self.filtered } else { self.output });
        self.v2(self.copy, "output_size", size);
        self.v2(self.copy, "output_origin", origin);
        self.v2(
            self.copy,
            "view_size",
            [view_width as f32, view_height as f32],
        );
        self.i(self.copy, "overlay_count", (glass.len() / 4) as i32);
        if !glass.is_empty() {
            self.gl.uniform_4_f32_slice(
                self.gl
                    .get_uniform_location(self.copy, "overlay_rects[0]")
                    .as_ref(),
                &glass,
            );
        }
        self.gl.draw_arrays(glow::TRIANGLES, 0, 3);
        self.check("显示")
    }

    /// Export-only filters never enter native viewport presentation or raw capture.
    pub unsafe fn export_pixels(&self) -> Result<Vec<u8>, String> {
        self.export_pixels_depth(8)
    }
    /// File exports retain the selected code precision; previews always use u8.
    pub unsafe fn export_pixels_for_output(&self) -> Result<Vec<u8>, String> {
        self.export_pixels_depth(self.params.output_bit_depth)
    }
    unsafe fn export_pixels_depth(&self, depth: u8) -> Result<Vec<u8>, String> {
        let export_pass = needs_export_pass(&self.params,self.geo_tile.is_some());
        if self.output_dirty.get() || export_pass {
            self.postprocess_output(export_pass);
        }
        let result = self.pixels_depth(depth);
        // Preserve the pre-existing capture contract for legacy filters and
        // transparent-shadow alpha. Ordinary exports need only the final pass.
        if export_pass { self.postprocess(); }
        result
    }
    pub unsafe fn pixels(&self) -> Result<Vec<u8>, String> {
        self.pixels_depth(8)
    }
    unsafe fn pixels_depth(&self, depth: u8) -> Result<Vec<u8>, String> {
        let (w, h) = (self.params.width as usize, self.params.height as usize);
        let bytes = usize::from(depth / 8);
        let mut out = vec![0; w * h * 4 * bytes];
        self.gl
            .bind_framebuffer(glow::FRAMEBUFFER, Some(if self.params.stretch_type != "none" { self.filtered_fbo } else { self.output_fbo }));
        self.gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
        self.gl.read_pixels(
            0,
            0,
            w as i32,
            h as i32,
            glow::RGBA,
            if depth == 16 { glow::UNSIGNED_SHORT } else { glow::UNSIGNED_BYTE },
            glow::PixelPackData::Slice(Some(&mut out)),
        );
        self.check("图像读回")?;
        clear_transparent_rgb(&mut out, depth);
        let row = w * 4 * bytes;
        for y in 0..h / 2 {
            let (a, b) = out.split_at_mut((h - 1 - y) * row);
            a[y * row..(y + 1) * row].swap_with_slice(&mut b[..row]);
        }
        Ok(out)
    }
    pub fn samples(&self) -> u32 {
        self.count
    }
    pub fn sample_pending(&self) -> bool {
        self.tile_fence.is_some() || self.sample_job.row > 0
    }
    pub fn dimensions(&self) -> (u32, u32) {
        (self.params.width, self.params.height)
    }
    pub unsafe fn reset_accumulation(&mut self) {
        self.reset();
    }
    pub fn driver(&self) -> (String, String) {
        self.driver_info.clone()
    }

    unsafe fn texture_filter(&self, filter: u32, wrap: u32) {
        self.gl
            .tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, filter as i32);
        let mag = if filter == glow::LINEAR {
            glow::LINEAR
        } else {
            glow::NEAREST
        };
        self.gl
            .tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, mag as i32);
        for axis in [glow::TEXTURE_WRAP_S, glow::TEXTURE_WRAP_T] {
            self.gl
                .tex_parameter_i32(glow::TEXTURE_2D, axis, wrap as i32);
        }
    }
    unsafe fn allocate(&mut self) -> Result<(), String> {
        let high = self.params.output_bit_depth == 16;
        let output_format = if high { glow::RGBA16 } else { glow::RGBA8 };
        let output_kind = if high { glow::UNSIGNED_SHORT } else { glow::UNSIGNED_BYTE };
        for (tex, fbo, format, kind) in [
            (self.accum, self.accum_fbo, glow::RGBA32F, glow::FLOAT),
            (
                self.output,
                self.output_fbo,
                output_format,
                output_kind,
            ),
        ] {
            self.gl.bind_texture(glow::TEXTURE_2D, Some(tex));
            self.texture_filter(glow::LINEAR, glow::CLAMP_TO_EDGE);
            self.gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                format as i32,
                self.params.width as i32,
                self.params.height as i32,
                0,
                glow::RGBA,
                kind,
                glow::PixelUnpackData::Slice(None),
            );
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            self.gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(tex),
                0,
            );
            if self.gl.check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE {
                return Err("渲染帧缓冲创建失败".into());
            }
        }
        self.gl
            .bind_framebuffer(glow::FRAMEBUFFER, Some(self.output_fbo));
        self.gl.clear_color(0., 0., 0., 1.);
        self.gl.clear(glow::COLOR_BUFFER_BIT);
        let enabled = needs_surface(&self.params);
        for (tex, fbo, w, h, format, kind) in [
            (self.surface_info,self.surface_fbo,if enabled {self.params.width} else {1},if enabled {self.params.height} else {1},glow::RGBA32F,glow::FLOAT),
            (self.filtered,self.filtered_fbo,if self.params.stretch_type != "none" {self.params.width} else {1},if self.params.stretch_type != "none" {self.params.height} else {1},output_format,output_kind),
            (self.stats_tex,self.stats_fbo,256,256,output_format,output_kind),
        ] {
            self.gl.bind_texture(glow::TEXTURE_2D,Some(tex));
            self.texture_filter(glow::NEAREST,glow::CLAMP_TO_EDGE);
            self.gl.tex_image_2d(glow::TEXTURE_2D,0,format as i32,w as i32,h as i32,0,glow::RGBA,kind,glow::PixelUnpackData::Slice(None));
            self.gl.bind_framebuffer(glow::FRAMEBUFFER,Some(fbo));
            self.gl.framebuffer_texture_2d(glow::FRAMEBUFFER,glow::COLOR_ATTACHMENT0,glow::TEXTURE_2D,Some(tex),0);
            if self.gl.check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE { return Err("显示增强缓冲创建失败".into()); }
        }
        self.gl.bind_texture(glow::TEXTURE_2D,Some(self.lut_tex));
        self.texture_filter(glow::LINEAR,glow::CLAMP_TO_EDGE);
        // RGBA16 and the full precision LUT avoid any intermediate 8-bit bottleneck.
        let identity = if high { u16_bytes(&stretch::build_lut16(&[],None,"none",2.,2.,2.)) }
            else { stretch::build_lut(&[],None,"none",2.,2.,2.) };
        self.gl.tex_image_2d(glow::TEXTURE_2D,0,if high {glow::RGB16} else {glow::RGB8} as i32,256,if high {256} else {1},0,glow::RGB,output_kind,glow::PixelUnpackData::Slice(Some(&identity)));
        self.reset();
        self.check("分配渲染缓冲")
    }
    unsafe fn reset(&mut self) {
        // The GPU cannot interrupt an issued strip. Retain its fence so the
        // replacement frame cannot queue more work until that strip finishes.
        self.tile_discarded = self.tile_fence.is_some();
        self.sample_job.reset();
        self.count = 0;
        self.output_dirty.set(false);
        self.surface_ready.set(false);
        self.stretch_count.set(0);
        if !self.stretch_frozen.get() { self.stretch_lut.replace(None); }
        self.gl.clear_color(0., 0., 0., 1.);
        // Keep the last complete display image during camera/light resets.
        // Only newly allocated output storage is initialized by allocate().
        for fbo in [self.accum_fbo] {
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            self.gl.clear(glow::COLOR_BUFFER_BIT);
        }
    }
    unsafe fn postprocess(&self) {
        self.postprocess_output(false);
    }
    unsafe fn postprocess_output(&self, export: bool) {
        if self.params.contour_enabled && !self.contour_ready.get() {
            if let Some(map)=self.map {
                // Reuse the unwarped cached height field; default-off scenes
                // never allocate a second full-size source texture or CPU copy.
                let data=contour_source_pixels(&self.height_base,[map.width,map.height],self.padded);
                self.gl.bind_texture(glow::TEXTURE_2D,Some(self.contour_tex));
                self.texture_filter(glow::LINEAR,glow::CLAMP_TO_EDGE);
                self.gl.tex_image_2d(glow::TEXTURE_2D,0,glow::RG32F as i32,map.width as i32,map.height as i32,
                    0,glow::RG,glow::FLOAT,glow::PixelUnpackData::Slice(Some(bytes(&data))));
                self.contour_ready.set(true);
            }
        }
        if needs_surface(&self.params) && !self.surface_ready.get() {
            self.gl.bind_framebuffer(glow::FRAMEBUFFER,Some(self.surface_fbo));
            self.gl.viewport(0,0,self.params.width as i32,self.params.height as i32);
            self.gl.use_program(Some(self.path));
            self.bind_tex(self.path,"mipmap",0,self.height_tex);
            self.bind_tex(self.path,"tex_rand",1,self.rand_tex);
            self.bind_tex(self.path,"tex_color",2,self.color_tex);
            self.i(self.path,"surface_pass",1);
            self.v2(self.path,"pixel_jitter",[0.,0.]);
            self.gl.draw_arrays(glow::TRIANGLES,0,3);
            self.i(self.path,"surface_pass",0);
            self.surface_ready.set(true);
        }
        self.gl
            .viewport(0, 0, self.params.width as i32, self.params.height as i32);
        let p = if export {
            if elevation_color_blend(&self.params) { self.elevation_output_post } else { self.output_posts[uses_color_display(&self.params) as usize] }
        } else if elevation_color_blend(&self.params) {
            self.elevation_post
        } else if uses_color_display(&self.params) {
            self.color_post
        } else {
            self.post
        };
        self.gl
            .bind_framebuffer(glow::FRAMEBUFFER, Some(self.output_fbo));
        self.gl.use_program(Some(p));
        self.bind_tex(p, "tex_accum", 0, self.accum);
        self.bind_tex(p, "tex_last", 1, self.accum);
        self.bind_tex(p, "tex_bloom", 2, self.accum);
        self.v2(
            p,
            "tex_size",
            [self.params.width as f32, self.params.height as f32],
        );
        self.v2(
            p,
            "image_size",
            [self.params.width as f32, self.params.height as f32],
        );
        self.v2(p, "frag_origin", [0., 0.]);
        self.f(p, "weight", 1.);
        self.f(p, "gamma", 1. / 2.2);
        self.f(p, "expo", 5.);
        self.f(p, "vig", 0.);
        self.f(p, "aces", 0.);
        self.f(p, "bloom", 0.);
        self.f(p, "display_mix", appearance_opacity(&self.params));
        self.bind_tex(p,"surface_info",3,self.surface_info);
        self.i(p,"elevation_blend_enabled",i32::from(elevation_color_blend(&self.params)));
        self.f(p,"elevation_blend_opacity",appearance_opacity(&self.params));
        self.i(p,"appearance_ramp",match self.params.color_ramp.as_str(){"grayscale"=>1,"spectral"=>2,"custom"=>3,_=>0});
        self.i(p,"appearance_discrete",i32::from(self.params.ramp_interpolation=="discrete"));
        for (index,color) in self.params.ramp_colors.iter().enumerate(){self.v3(p,&format!("appearance_colors[{index}]"),parse_color(color).unwrap_or([0.;3]));}
        self.bind_tex(p,"workflow_surface",4,self.surface_info);
        self.bind_tex(p,"workflow_texture",5,self.color_tex);
        self.v2(p,"workflow_texture_size",[self.color_size[0].max(1) as f32,self.color_size[1].max(1) as f32]);
        self.i(p,"texture_overlay_enabled",i32::from(self.params.appearance_mode=="texture"));
        self.f(p,"texture_opacity",self.params.texture_opacity/100.);
        self.bind_tex(p,"contour_source",6,self.contour_tex);
        self.v2(p,"contour_source_size",self.map.map(|m|[m.width as f32,m.height as f32]).unwrap_or([1.,1.]));
        self.v2(p,"contour_source_step",self.map.map(|m|contour_reference_step(m.source_size)).unwrap_or([1.,1.]));
        self.f(p,"contour_smoothing",self.params.contour_smoothing);
        self.i(p,"contour_enabled",i32::from(self.params.contour_enabled));
        self.v2(p,"contour_elevation",contour_levels(self.map.map(|m|m.source_elevation).unwrap_or([0.,0.]),self.params.contour_interval,self.params.contour_major_every));
        self.f(p,"contour_interval",self.params.contour_interval);
        self.f(p,"contour_width",self.params.contour_width);
        self.f(p,"contour_opacity",self.params.contour_opacity/100.);
        self.f(p,"contour_major_every",self.params.contour_major_every as f32);
        self.f(p,"contour_major_width",self.params.contour_major_width);
        self.v3(p,"contour_color",parse_color(&self.params.contour_color).unwrap_or([0.;3]));
        for (name,value) in display_uniforms(&self.params) { self.f(p,name,value); }
        if export {
            self.i(p,"output_filter_enabled",i32::from(self.params.stretch_type=="none"));
            self.f(p,"output_exposure",self.params.exposure);
            self.f(p,"output_gamma",self.params.gamma);
            self.i(
                p,
                "export_shadow_matte",
                i32::from(self.geo_tile.is_some() && self.params.export_shadow),
            );
            for (name, value) in [
                "output_contrast",
                "output_saturation",
                "output_temperature",
                "output_shadows",
                "output_highlights",
            ]
            .into_iter()
            .zip(output_adjustments(&self.params))
            {
                self.f(p, name, value);
            }
        }
        self.gl.draw_arrays(glow::TRIANGLES, 0, 3);
        if self.params.stretch_type != "none" { self.apply_stretch(export); }
        self.output_dirty.set(false);
    }
    unsafe fn bind_tex(&self, p: glow::Program, name: &str, unit: u32, tex: glow::Texture) {
        self.gl.active_texture(glow::TEXTURE0 + unit);
        self.gl.bind_texture(glow::TEXTURE_2D, Some(tex));
        self.i(p, name, unit as i32);
    }
    unsafe fn f(&self, p: glow::Program, n: &str, v: f32) {
        self.gl
            .uniform_1_f32(self.gl.get_uniform_location(p, n).as_ref(), v);
    }
    unsafe fn i(&self, p: glow::Program, n: &str, v: i32) {
        self.gl
            .uniform_1_i32(self.gl.get_uniform_location(p, n).as_ref(), v);
    }
    unsafe fn v2(&self, p: glow::Program, n: &str, v: [f32; 2]) {
        self.gl
            .uniform_2_f32_slice(self.gl.get_uniform_location(p, n).as_ref(), &v);
    }
    unsafe fn v3(&self, p: glow::Program, n: &str, v: [f32; 3]) {
        self.gl
            .uniform_3_f32_slice(self.gl.get_uniform_location(p, n).as_ref(), &v);
    }
    unsafe fn v4(&self, p: glow::Program, n: &str, v: [f32; 4]) {
        self.gl
            .uniform_4_f32_slice(self.gl.get_uniform_location(p, n).as_ref(), &v);
    }
    unsafe fn matrix(&self, p: glow::Program, n: &str, v: [f32; 9]) {
        self.gl
            .uniform_matrix_3_f32_slice(self.gl.get_uniform_location(p, n).as_ref(), false, &v);
    }
    unsafe fn check(&self, stage: &str) -> Result<(), String> {
        let e = self.gl.get_error();
        if e == glow::NO_ERROR {
            Ok(())
        } else {
            Err(format!("OpenGL {stage}: 0x{e:04x}"))
        }
    }
}

impl GlRenderer {
    // Statistics readback is capped at 256² pixels, independently of source/output size.
    unsafe fn statistics_pixels(&self, source: glow::Framebuffer, high: bool) -> Vec<u8> {
        self.gl.bind_framebuffer(glow::READ_FRAMEBUFFER,Some(source));
        self.gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER,Some(self.stats_fbo));
        self.gl.blit_framebuffer(0,0,self.params.width as i32,self.params.height as i32,0,0,256,256,glow::COLOR_BUFFER_BIT,glow::NEAREST);
        self.gl.bind_framebuffer(glow::FRAMEBUFFER,Some(self.stats_fbo));
        let mut pixels=vec![0;256*256*4*if high {2} else {1}];
        self.gl.pixel_store_i32(glow::PACK_ALIGNMENT,1);
        self.gl.read_pixels(0,0,256,256,glow::RGBA,if high {glow::UNSIGNED_SHORT} else {glow::UNSIGNED_BYTE},glow::PixelPackData::Slice(Some(&mut pixels)));
        pixels
    }
    unsafe fn apply_stretch(&self, export: bool) {
        let refresh = self.stretch_lut.borrow().is_none() || (!self.stretch_frozen.get()
            && (export != self.stretch_export.get() || (self.stretch_count.get()!=self.count
                && (self.count.is_power_of_two() || self.count==self.params.samples))));
        if refresh {
            let high=self.params.output_bit_depth==16;
            let colors=self.statistics_pixels(self.output_fbo,high);
            let mask:Vec<u8>=self.statistics_pixels(self.surface_fbo,false).chunks_exact(4).map(|p|p[1]).collect();
            let lut=if high {
                let words:Vec<u16>=colors.chunks_exact(2).map(|p|u16::from_ne_bytes([p[0],p[1]])).collect();
                u16_bytes(&stretch::build_lut16(&words,Some(&mask),&self.params.stretch_type,self.params.stretch_deviations,self.params.stretch_low,self.params.stretch_high))
            } else {stretch::build_lut(&colors,Some(&mask),&self.params.stretch_type,self.params.stretch_deviations,self.params.stretch_low,self.params.stretch_high)};
            self.stretch_lut.replace(Some(lut));
            self.stretch_count.set(self.count);self.stretch_export.set(export);
        }
        self.gl.bind_texture(glow::TEXTURE_2D,Some(self.lut_tex));
        self.gl.pixel_store_i32(glow::UNPACK_ALIGNMENT,1);
        let lut=self.stretch_lut.borrow();
        let high=self.params.output_bit_depth==16;
        self.gl.tex_sub_image_2d(glow::TEXTURE_2D,0,0,0,256,if high {256} else {1},glow::RGB,if high {glow::UNSIGNED_SHORT} else {glow::UNSIGNED_BYTE},glow::PixelUnpackData::Slice(lut.as_deref()));
        self.gl.bind_framebuffer(glow::FRAMEBUFFER,Some(self.filtered_fbo));
        self.gl.viewport(0,0,self.params.width as i32,self.params.height as i32);
        self.gl.use_program(Some(self.stretch_program));
        self.bind_tex(self.stretch_program,"image",0,self.output);
        self.bind_tex(self.stretch_program,"surface_info",1,self.surface_info);
        self.bind_tex(self.stretch_program,"stretch_lut",2,self.lut_tex);
        self.i(self.stretch_program,"stretch_high_precision",i32::from(high));
        self.v2(self.stretch_program,"image_size",[self.params.width as f32,self.params.height as f32]);
        self.i(self.stretch_program,"output_filter_enabled",i32::from(export));
        self.i(self.stretch_program,"export_shadow_matte",i32::from(self.geo_tile.is_some() && self.params.export_shadow));
        self.f(self.stretch_program,"output_exposure",self.params.exposure);
        self.f(self.stretch_program,"output_gamma",self.params.gamma);
        for (name,value) in ["output_contrast","output_saturation","output_temperature","output_shadows","output_highlights"].into_iter().zip(output_adjustments(&self.params)) { self.f(self.stretch_program,name,value); }
        self.gl.draw_arrays(glow::TRIANGLES,0,3);
    }
    pub unsafe fn freeze_stretch_from_current(&self) {
        if self.params.stretch_type != "none" {
            self.postprocess_output(true);
            self.stretch_frozen.set(true);
        }
    }
    pub fn clear_frozen_stretch(&self) {
        self.stretch_frozen.set(false);self.stretch_lut.replace(None);self.stretch_count.set(0);
    }
    pub fn restart_sampling_sequence(&mut self) {
        self.rng = [123456789, 362436069, 521288629, 88675123];
    }
}

impl Drop for GlRenderer {
    fn drop(&mut self) {
        unsafe {
            if let Some(fence) = self.tile_fence.take() {
                self.gl.delete_sync(fence);
            }
            for p in [self.path, self.post, self.color_post, self.copy, self.stretch_program, self.elevation_post, self.elevation_output_post]
                .into_iter()
                .chain(self.output_posts)
            {
                self.gl.delete_program(p);
            }
            for t in [
                self.height_tex,
                self.contour_tex,
                self.rand_tex,
                self.color_tex,
                self.sky_tex,
                self.accum,
                self.output, self.surface_info, self.filtered, self.stats_tex, self.lut_tex,
            ] {
                self.gl.delete_texture(t);
            }
            for f in [self.accum_fbo, self.output_fbo, self.surface_fbo, self.filtered_fbo, self.stats_fbo] {
                self.gl.delete_framebuffer(f);
            }
            self.gl.delete_vertex_array(self.vao);
        }
    }
}

unsafe fn program(gl: &glow::Context, fragment: &str) -> Result<glow::Program, String> {
    let p = gl.create_program()?;
    let mut shaders = Vec::new();
    for (kind, source) in [
        (glow::VERTEX_SHADER, VERT),
        (glow::FRAGMENT_SHADER, fragment),
    ] {
        let s = gl.create_shader(kind)?;
        gl.shader_source(s, source);
        gl.compile_shader(s);
        if !gl.get_shader_compile_status(s) {
            let log = gl.get_shader_info_log(s);
            gl.delete_shader(s);
            for old in shaders {
                gl.delete_shader(old);
            }
            gl.delete_program(p);
            return Err(format!("GLSL 编译失败: {log}"));
        }
        gl.attach_shader(p, s);
        shaders.push(s);
    }
    gl.link_program(p);
    let ok = gl.get_program_link_status(p);
    let log = gl.get_program_info_log(p);
    for s in shaders {
        gl.detach_shader(p, s);
        gl.delete_shader(s);
    }
    if !ok {
        gl.delete_program(p);
        return Err(format!("GLSL 链接失败: {log}"));
    }
    Ok(p)
}
// Higher mip levels bound every source mip cell touched by a shifted logical
// cell. The logical interval [k*S,(k+1)*S] maps to [k*S-1,(k+1)*S-1],
// intersecting at most two source cells per axis. Existing stride-2 3x3 maxima
// conservatively include each cell's bilinear endpoint samples.
const GEO_HEIGHT_SAMPLER: &str = r#"
uniform sampler2D mipmap;
uniform vec2 geo_source_size;
uniform vec2 mipmap_tex_inv_size;
float geo_height(vec2 p,float level){
    if(level<0.5){
        vec2 q=clamp(p-vec2(1.),vec2(0.),geo_source_size-vec2(1.));
        return texture2DLod(mipmap,(q+vec2(.5))*mipmap_tex_inv_size,0.).x;
    }
    float scale=exp2(level);
    vec2 cell=floor(p/scale)*scale;
    vec2 lo=clamp(cell-vec2(1.),vec2(0.),geo_source_size-vec2(1.));
    vec2 hi=clamp(cell+vec2(scale-1.),vec2(0.),geo_source_size-vec2(1.));
    lo=floor(lo/scale)*scale;hi=floor(hi/scale)*scale;
    vec2 a=(lo+vec2(.5))*mipmap_tex_inv_size;
    vec2 b=(hi+vec2(.5))*mipmap_tex_inv_size;
    return max(max(texture2DLod(mipmap,a,level).x,texture2DLod(mipmap,b,level).x),
               max(texture2DLod(mipmap,vec2(a.x,b.y),level).x,texture2DLod(mipmap,vec2(b.x,a.y),level).x));
}
"#;
fn conservative_mip(data: &[f32], n: u32) -> Vec<f32> {
    let next = n / 2;
    let mut reduced = vec![0.; (next * next) as usize];
    // 3x3 stride-2 maxima conservatively bound the bilinear cell's four corners.
    for y in 0..next {
        for x in 0..next {
            let mut h = 0f32;
            for dy in 0..3 {
                for dx in 0..3 {
                    h = h.max(
                        data[((y * 2 + dy).min(n - 1) * n + (x * 2 + dx).min(n - 1)) as usize],
                    );
                }
            }
            reduced[(y * next + x) as usize] = h;
        }
    }
    reduced
}
fn camera_target(width: u32, height: u32, _vertical: f32, p: &RenderParams) -> [f32; 3] {
    let dim = width.max(height) as f32;
    [
        width as f32 * 0.5 + p.camera_pan[0] * dim,
        height as f32 * 0.5 + p.camera_pan[1] * dim,
        dim * 0.2 * 0.3 + p.camera_pan[2] * dim,
    ]
}
fn output_tile_mapping(
    tile: Option<([u32; 2], [u32; 4])>,
    fallback: [u32; 2],
) -> ([f32; 2], [f32; 2]) {
    match tile {
        Some(([w, h], [x, y, _, rows])) => {
            ([w as f32, h as f32], [x as f32, (h - y - rows) as f32])
        }
        None => (fallback.map(|v| v as f32), [0.; 2]),
    }
}

fn geo_tile_origin(source_height: u32, [x, y, w, h]: [u32; 4], z: f32) -> [f32; 3] {
    [
        x as f32 + w as f32 * 0.5 + 0.5,
        source_height as f32 - y as f32 - h as f32 * 0.5 + 0.5,
        z,
    ]
}

fn geo_grid_mapping(
    map: [u32; 2],
    [sw, sh, left, top]: [u32; 4],
    [x, y, w, h]: [u32; 4],
    output: [u32; 2],
    z: f32,
) -> ([f32; 3], [f32; 2]) {
    let sx = f64::from(map[0]) / f64::from(sw);
    let sy = f64::from(map[1]) / f64::from(sh);
    (
        [
            (0.5 + (f64::from(x) - f64::from(left) + f64::from(w) * 0.5) * sx) as f32,
            (0.5 + f64::from(map[1]) - (f64::from(y) - f64::from(top) + f64::from(h) * 0.5) * sy)
                as f32,
            z,
        ],
        [
            (sx * f64::from(w) / f64::from(output[0])) as f32,
            (sy * f64::from(h) / f64::from(output[1])) as f32,
        ],
    )
}

pub(super) fn path_source(p: &RenderParams, geo: bool) -> String {
    let mut source = String::from(
        "#version 330 compatibility\n#define OPT_SUN\n#define OPT_SUN_DISK\n",
    );
    source.push_str("uniform int surface_pass;\nuniform float height_curve;\nfloat source_height(float t){if(height_curve==1.)return t;float q=clamp((t-1e-6)/(1.-1e-6),0.,1.);return (height_curve*q/((1.-q)+height_curve*q))*(1.-1e-6)+1e-6;}\n");
    source.push_str("uniform vec2 output_full_size,output_tile_origin;uniform int output_tiled;uniform vec2 pixel_jitter;\nvec2 output_pixel(){return gl_FragCoord.xy+(output_tiled!=0?output_tile_origin:vec2(0.));}\nvec2 output_st(vec2 original){if(output_tiled==0)return original;return (output_pixel()*2.-output_full_size)/min(output_full_size.x,output_full_size.y)+pixel_jitter;}\n");
    source.push_str(match p.surface.as_str() {
        "cube" => "#define PRIM_CUBE\n",
        "poly" => "#define PRIM_POLY\n",
        "bicubic" => "#define PRIM_BICUBIC\n",
        _ => "#define PRIM_SURF\n",
    });
    source.push_str(if p.projection == "perspective" && !geo {
        "#define OPT_PERS\n"
    } else {
        "#define OPT_ORTH\n"
    });
    if p.grid && !geo {
        source.push_str("#define OPT_GRID\n#define OPT_GROUND\n");
    }
    if geo {
        source.push_str("uniform vec2 geo_pixel_step;uniform int geo_shadow_catcher;\n");
    }
    if p.height_step > 0. {
        source.push_str("#define OPT_LEVEL\n");
    }
    for block in [
        include_str!("shaders/00_math.glsl"),
        include_str!("shaders/01_jitter.glsl"),
        include_str!("shaders/02_sampling.glsl"),
        include_str!("shaders/03_microfacet_helpers.glsl"),
        include_str!("shaders/04_path_state.glsl"),
        include_str!("shaders/05_heightfield_traversal.glsl"),
        include_str!("shaders/07_atmosphere_lighting.glsl"),
        include_str!("shaders/08_materials.glsl"),
        include_str!("shaders/09_camera.glsl"),
        include_str!("shaders/11_path_tracing.glsl"),
    ] {
        let block = if p.experimental_bounces {
            block.replace("MAX_SEGMENTS=16", "MAX_SEGMENTS=101")
                .replace("max_bounce=15", "max_bounce=100")
        } else {block.to_string()};
        let block = if block.contains("uniform vec3 Le;") {
            block.replace("return Le;", "return scene_sky(dir);")
                .replace("uniform vec3 Le;", &format!("uniform vec3 Le;\n{}\n", include_str!("sky_environment.glsl")))
        } else { block.to_string() };
        let block = block
            .replace(
                "gl_FragCoord.xy*tex_rand_inv_size",
                "output_pixel()*tex_rand_inv_size",
            )
            .replace(
                "sample_ray(st,o,dir,A)",
                "sample_ray(output_st(st),o,dir,A)",
            )
            .replace(
                "Lo+=eval_light_sun(A,dir,INF,n,inv_pdf_o_brdf,0.0,0.0,path_mask.xy);",
                "if(bounce>0)Lo+=eval_light_sun(A,dir,INF,n,inv_pdf_o_brdf,0.0,0.0,path_mask.xy);",
            );
        let block = if geo {
            block.replace(
                "eye=proj_orth(st);",
                "eye=proj_orth(st);eye.xy*=geo_pixel_step;",
            )
        } else {
            block
        };
        let block = if geo && p.export_shadow && block.contains("vec4 pt(vec3 o") {
            block.replace("varying vec2 st;void main(void)", &format!("{}\nvarying vec2 st;void main(void)", include_str!("shadow_catcher.glsl")))
                .replace("gl_FragColor=max(pt(o,dir,A),0.0);", "if(geo_shadow_catcher!=0&&(any(lessThan(o.xy,vec2(.5)))||any(greaterThanEqual(o.xy,geo_source_size+vec2(.5))))){gl_FragColor=catch_shadow(vec3(o.xy,0.));return;}gl_FragColor=max(pt(o,dir,A),0.0);")
        } else {
            block
        };
        let block = if block.contains("vec3 color(float id,vec3 p)") {
            let start = block.find("vec3 color(float id,vec3 p)").unwrap();
            let end = block[start..].find("vec4 material(float id)").unwrap() + start;
            format!(
                "{}\n{}\n{}",
                &block[..start],
                include_str!("appearance.glsl").replace("float t=(p.z-appearance_height.x)/max(appearance_height.y,1e-12);", "float t=source_height((p.z-appearance_height.x)/max(appearance_height.y,1e-12));"),
                &block[end..]
            )
        } else {
            block.to_string()
        };
        let block = if geo && block.contains("float heightLod(") {
            block
                .replace(
                    "uniform sampler2D mipmap;uniform vec2 mipmap_tex_inv_size;",
                    GEO_HEIGHT_SAMPLER,
                )
                .replace(
                    "texture2DLod(mipmap,(i_pos+0.5)*mipmap_tex_inv_size,level).x",
                    "geo_height(i_pos,level)",
                )
        } else {
            block
        };
        let block = if p.surface == "bicubic" && block.contains("float heightLod(") {
            block.replace("return h+mipmap_size.w;}", &format!("return h+mipmap_size.w;}}\n{}\n#define heightLod bc_bound\n", include_str!("bicubic.glsl")))
                .replace("vec4 H=vec4(heightLod(i_pos,lod),heightLod(i_pos+vec2(scale,0.0),lod),heightLod(i_pos+vec2(0.0,scale),lod),heightLod(i_pos+vec2(scale,scale),lod));", "vec4 H=vec4(bc_bound(i_cell,lod));")
        } else {
            block
        };
        let block = if block.contains("vec4 grid_line(") {
            let start = block.find("vec4 grid_line(").unwrap();
            let end = block[start..].find("\n#endif").unwrap() + start;
            // Replace through the function's closing brace after nested OPT_LEVEL.
            let end = block[end..].find("}\n#endif").unwrap() + end + 1;
            format!("{}{}{}", &block[..start], GROUND_GRID, &block[end..])
        } else {
            block
        };
        source.push_str(&block);
        source.push('\n');
    }
    source.replace("sample_ray(output_st(st),o,dir,A);", "sample_ray(output_st(st),o,dir,A);if(surface_pass!=0){float d=INF;vec3 n=vec3(0.);vec2 id=ray_vs_world(o,dir,d,n);if(is_map(id.y)&&!is_n_zero(n)){float t=source_height((o.z+dir.z*d-appearance_height.x)/max(appearance_height.y,1e-12));vec2 tex_st=clamp((o.xy+dir.xy*d-vec2(appearance_origin))/max(appearance_extent-1.,vec2(1.)),0.,1.);gl_FragColor=vec4(clamp(t,0.,1.),1.,tex_st);}else gl_FragColor=vec4(0.);return;}")
}
fn copy_display_fields(from: &RenderParams,to: &mut RenderParams) {
    to.exposure=from.exposure;
    to.gamma=from.gamma;
    to.scene_exposure=from.scene_exposure;
    to.scene_gamma=from.scene_gamma;
    to.scene_brightness=from.scene_brightness;
    to.scene_contrast=from.scene_contrast;
    to.scene_saturation=from.scene_saturation;
    to.scene_temperature=from.scene_temperature;
    to.scene_shadows=from.scene_shadows;
    to.scene_highlights=from.scene_highlights;
    to.texture_exposure=from.texture_exposure;
    to.texture_contrast=from.texture_contrast;
    to.texture_shadows=from.texture_shadows;
    to.texture_highlights=from.texture_highlights;
    to.texture_saturation=from.texture_saturation;
    to.texture_opacity=from.texture_opacity;
    to.contour_interval=from.contour_interval;
    to.contour_width=from.contour_width;
    to.contour_smoothing=from.contour_smoothing;
    to.contour_color=from.contour_color.clone();
    to.contour_opacity=from.contour_opacity;
    to.contour_major_every=from.contour_major_every;
    to.contour_major_width=from.contour_major_width;
    to.output_contrast=from.output_contrast;
    to.output_saturation=from.output_saturation;
    to.output_temperature=from.output_temperature;
    to.output_shadows=from.output_shadows;
    to.output_highlights=from.output_highlights;
}
pub(super) fn same_scene_without_output_filter(old: &RenderParams, next: &RenderParams) -> bool {
    let mut comparison = next.clone();
    copy_display_fields(old,&mut comparison);
    comparison == *old
}
fn u16_bytes(values: &[u16]) -> Vec<u8> {
    values.iter().flat_map(|v|v.to_ne_bytes()).collect()
}
fn clear_transparent_rgb(pixels: &mut [u8], depth: u8) {
    let bytes=usize::from(depth/8);
    for p in pixels.chunks_exact_mut(4*bytes) {
        if p[3*bytes..].iter().all(|v|*v==0) {p[..3*bytes].fill(0);}
    }
}
fn contour_reference_step(size: [u32;2]) -> [f32;2] {
    let base=(size[0].max(size[1]).saturating_sub(1) as f64/2047.).max(1.);
    size.map(|axis|(base/f64::from(axis.saturating_sub(1).max(1))) as f32)
}
fn contour_source_pixels(encoded: &[f32],size: [u32;2],stride: u32) -> Vec<f32> {
    let mut out=Vec::with_capacity(size[0] as usize*size[1] as usize*2);
    for y in 0..size[1] as usize {
        for x in 0..size[0] as usize {
            let value=encoded[y*stride as usize+x];
            let valid=if value>0. {1.}else{0.};
            out.extend([if valid>0. {((value-1e-6)/(1.-1e-6)).clamp(0.,1.)}else{0.},valid]);
        }
    }
    out
}


fn source_elevation_range(map: &HeightMap) -> [f64;2] {
    if let Some([low,high])=map.source_elevation_range {return [low,(high-low).max(0.)];}
    // Images supply relative brightness, not physical elevation. Use a stable
    // 0..100 range independent of preview dimensions and legacy world scale.
    [100.*f64::from(map.display_range[0]),100.*f64::from(map.display_range[1]-map.display_range[0])]
}
fn contour_levels([bottom,span]: [f64;2],interval: f32,major_every: u32) -> [f32;2] {
    let interval=f64::from(interval);
    [(bottom/interval).rem_euclid(f64::from(major_every)) as f32,(span/interval) as f32]
}
pub(crate) fn validate_contour_parameters(p: &RenderParams) -> Result<(),String> {
    for (value,low,high) in [(p.contour_interval,0.01,1e6),(p.contour_width,0.1,30.),
        (p.contour_major_width,0.1,30.),(p.contour_opacity,0.,100.),(p.contour_smoothing,0.,5.)] {
        if !value.is_finite() || !(low..=high).contains(&value) {
            return Err("等高线参数超出范围".into());
        }
    }
    if !(1..=20).contains(&p.contour_major_every) {return Err("主等高线间隔超出范围".into());}
    parse_color(&p.contour_color)?;
    Ok(())
}
fn needs_surface(p: &RenderParams) -> bool {
    p.stretch_type!="none" || elevation_color_blend(p) || p.appearance_mode=="texture" || p.contour_enabled
}
fn needs_export_pass(p: &RenderParams, georeferenced: bool) -> bool {
    !output_is_neutral(p) || (georeferenced && p.export_shadow)
}
fn output_is_neutral(p: &RenderParams) -> bool {
    p.exposure==1. && p.gamma==2.2 && output_adjustments(p).iter().all(|v| *v==0.)
}
fn display_uniforms(p: &RenderParams) -> [(&'static str,f32);13] {
    [
        ("scene_exposure",p.scene_exposure),("scene_gamma",p.scene_gamma),
        ("scene_brightness",p.scene_brightness/100.),("scene_contrast",p.scene_contrast/100.),
        ("scene_saturation",p.scene_saturation/100.),("scene_temperature",p.scene_temperature/100.),
        ("scene_shadows",p.scene_shadows/100.),("scene_highlights",p.scene_highlights/100.),
        ("texture_exposure",p.texture_exposure),("texture_contrast",p.texture_contrast/100.),
        ("texture_saturation",p.texture_saturation/100.),("texture_shadows",p.texture_shadows/100.),
        ("texture_highlights",p.texture_highlights/100.),
    ]
}
fn validate_display_parameters(p: &RenderParams) -> Result<(),String> {
    for (name,value) in display_uniforms(p) {
        let valid=match name {
            "scene_gamma" => (0.1..=4.).contains(&value),
            "scene_exposure"|"texture_exposure" => (-4. ..=4.).contains(&value),
            _ => (-1. ..=1.).contains(&value),
        };
        if !value.is_finite() || !valid { return Err("调色参数超出范围".into()); }
    }
    Ok(())
}
fn stretch_post_source() -> String {
    include_str!("stretch_display.glsl")
        .replace("void main(){",&format!("{}\nuniform int output_filter_enabled, export_shadow_matte;\nvoid main(){{",include_str!("output_filter.glsl")))
        .replace("gl_FragColor=color;","if(output_filter_enabled!=0){vec3 incoming=color.rgb;color.rgb=output_filter(color.rgb);if(export_shadow_matte!=0&&all(equal(incoming,vec3(0.))))color.rgb=vec3(0.);}gl_FragColor=color;")
}
fn output_adjustments(p: &RenderParams) -> [f32; 5] {
    [
        p.output_contrast,
        p.output_saturation,
        p.output_temperature,
        p.output_shadows,
        p.output_highlights,
    ]
    .map(|v| v / 100.)
}
fn output_post_source(color_mode: bool) -> String {
    filter_post_source(post_source(color_mode))
}
fn filter_post_source(source: String) -> String {
    source
        .replace(
            "void main(){",
            &format!("{}\nuniform int export_shadow_matte, output_filter_enabled;\nvoid main(){{", include_str!("output_filter.glsl")),
        )
        .replace(
            "gl_FragColor.rgb=scene_filter(contour_display(texture_display_overlay(gl_FragColor.rgb,uv),uv));",
            "gl_FragColor.rgb=scene_filter(contour_display(texture_display_overlay(gl_FragColor.rgb,uv),uv));if(output_filter_enabled!=0)gl_FragColor.rgb=output_filter(gl_FragColor.rgb);if(export_shadow_matte!=0&&all(equal(texture2D(tex_accum,uv).rgb,vec3(0.))))gl_FragColor.rgb=vec3(0.);",
        )
}

fn elevation_post_source() -> String {
    let palette=include_str!("appearance.glsl").split("vec3 color(float id,vec3 p)").next().unwrap();
    post_source(false).replace("void main(){",&format!("{}\n{}\nvoid main(){{",palette,include_str!("color_blend.glsl")))
        .replace("dst=clamp(dst,0.0,1.0);", "dst=clamp(dst,0.0,1.0);dst.xyz=elevation_display_blend(dst.xyz,uv);")
}
fn post_source(color_mode: bool) -> String {
    let original = include_str!("shaders/12_postprocess.glsl");
    let source=if color_mode {
        original.replace("void main(){",&format!("{}\nvoid main(){{",include_str!("tonemap.glsl")))
            .replace("dst.xyz=pow(post(dst.xyz),vec3(gamma));","dst.xyz=relief_display(dst.xyz);")
    } else { original.to_owned() };
    let source=source.replace("void main(){",&format!("{}\nvoid main(){{",include_str!("display_workflow.glsl")))
        .replace("gl_FragColor=mix(src,dst,weight);","gl_FragColor=mix(src,dst,weight);gl_FragColor.rgb=scene_filter(contour_display(texture_display_overlay(gl_FragColor.rgb,uv),uv));");
    format!("#version 330 compatibility\n{source}")
}

fn color_texture_rows(texture: &ColorTexture) -> Vec<u8> {
    let stride = texture.width as usize * 4;
    texture
        .rgba
        .chunks_exact(stride)
        .rev()
        .flatten()
        .copied()
        .collect()
}

fn parse_color(s: &str) -> Result<[f32; 3], String> {
    let s = s.strip_prefix('#').unwrap_or(s);
    if s.len() != 6 || !s.is_ascii() {
        return Err("颜色必须是 #RRGGBB".into());
    }
    let mut c = [0.; 3];
    for i in 0..3 {
        c[i] = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).map_err(|_| "无效颜色")? as f32 / 255.;
    }
    Ok(c)
}

fn parse_srgb_color(s: &str) -> Result<[f32; 3], String> {
    Ok(parse_color(s)?.map(|c| {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }))
}

fn material_albedo(p: &RenderParams) -> Result<[f32; 3], String> {
    let color = parse_color(&p.material_color)?;
    if p.appearance_mode == "elevation" {
        return Ok(color);
    }
    let base = parse_color("#d5d2c9")?;
    let opacity = (p.material_opacity/100.).clamp(0.,1.);
    // Preserve the legacy endpoint exactly, without subtract/add round-off.
    if opacity >= 1. {
        return Ok(color);
    }
    Ok(std::array::from_fn(|i| {
        base[i] * (1. - opacity) + color[i] * opacity
    }))
}

fn appearance_opacity(p: &RenderParams) -> f32 {
    (match p.appearance_mode.as_str() {
        "texture" => p.texture_opacity,
        "elevation" => p.elevation_opacity,
        _ => p.material_opacity,
    } / 100.)
        .clamp(0., 1.)
}

fn elevation_color_blend(p: &RenderParams) -> bool { p.appearance_mode == "elevation" && p.elevation_blend == "color" }
fn uses_color_display(p: &RenderParams) -> bool {
    p.appearance_mode == "elevation" && !elevation_color_blend(p)
}
// Decoders and ASC use top-to-bottom rows; the terrain uses +Y toward
// the image's top. Convert once for both viewport and export rendering.
const GROUND_GRID: &str = "vec4 grid_line(vec2 id,vec3 v,vec3 n){if(!is_ground(id.y)){return vec4(0.0);}vec2 q=abs(v.xy-floor(v.xy/parm_grid.y+0.5)*parm_grid.y);float line=1.0-step(parm_grid.x,min(q.x,q.y));return vec4(K_grid,line);}";
fn height_offset(width: u32, height: u32, p: &RenderParams) -> f32 {
    p.height_offset * width.max(height) as f32 * 0.01
}
fn display_sample(value: f32, [minimum, maximum]: [f32; 2]) -> f32 {
    if value == 0. {
        return 0.;
    }
    let q = if maximum > minimum {
        ((value - minimum) / (maximum - minimum)).clamp(0., 1.)
    } else {
        0.
    };
    q * (1. - 1e-6) + 1e-6
}
fn effective_height_curve(p: &RenderParams) -> f32 {
    if p.height_scale_mode == "nonlinear" { p.height_curve } else { 1. }
}

fn curved_height_texture(base: &[f32], curve: f32) -> Vec<f32> {
    if curve == 1. { return base.to_vec(); }
    base.iter().map(|&value| {
        // Zero is NoData; valid valley pixels retain the positive sentinel.
        if value == 0. { return 0.; }
        let q = ((value - 1e-6) / (1. - 1e-6)).clamp(0., 1.);
        // Combined with the amplitude multiplier, s*t/(s*(1-t)+t)
        // grows monotonically with s at every elevation. A power curve
        // would instead lower mid-elevation terrain at larger values.
        (q / (curve * (1. - q) + q)) * (1. - 1e-6) + 1e-6
    }).collect()
}
fn padded_height_texture(map: &HeightMap, size: u32) -> Vec<f32> {
    let mut data = vec![0.; size as usize * size as usize];
    for y in 0..size {
        // Clamp in world coordinates before reversing the valid image rows.
        // Reversing the padded square would move non-square maps into padding.
        let source_y = map.height - 1 - y.min(map.height - 1);
        for x in 0..size {
            data[(y * size + x) as usize] =
                map.values[(source_y * map.width + x.min(map.width - 1)) as usize];
        }
    }
    data
}

fn basis(z: [f32; 3]) -> [f32; 9] {
    let norm = (z[0] * z[0] + z[1] * z[1]).sqrt().max(0.000001);
    let x = [-z[1] / norm, z[0] / norm, 0.];
    let y = [-z[2] * x[1], z[2] * x[0], z[0] * x[1] - z[1] * x[0]];
    [x[0], x[1], x[2], y[0], y[1], y[2], z[0], z[1], z[2]]
}
fn camera_basis(yaw: f32, pitch: f32) -> [f32; 9] {
    // Derive right from azimuth, so the exact top view never normalizes a
    // vanishing horizontal back vector or loses its requested orientation.
    let (sy, cy) = yaw.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    [-sy, cy, 0., -sp * cy, -sp * sy, cp, cy * cp, sy * cp, sp]
}
fn bytes(v: &[f32]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) }
}
