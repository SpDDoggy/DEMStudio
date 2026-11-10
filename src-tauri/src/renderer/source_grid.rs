//! Original-resolution DEM windows used by the bounded paged export renderer.
//! TIFF uses bounded chunks; other formats reuse their import decode budgets.
//! No disk cache. Normalization preserves each format's original import values.
use super::tiff_stream::ChunkReader;
use std::{collections::VecDeque, fs::File, io::BufReader, path::Path};
use tiff::{decoder::Decoder, tags::Tag};

pub(super) const PAGE_EDGE: u32 = 512;
const MAX_WINDOW_PIXELS: u64 = 1024 * 1024;
type Tiff = Decoder<BufReader<File>>;

#[derive(Clone, Debug)]
pub(super) struct SourceMetadata {
    pub width: u32,
    pub height: u32,
    pub nodata: Option<f64>,
    pub elevation_range: [f64; 2],
    pub normalized_input: bool,
}
impl SourceMetadata {
    pub fn normalized(&self, value: f64) -> f32 {
        if !value.is_finite() || self.nodata.is_some_and(|n| n == value) {
            return 0.;
        }
        if self.normalized_input { return value as f32; }
        let [low, high] = self.elevation_range;
        let q = if high > low {
            ((value - low) / (high - low)) as f32
        } else {
            0.
        };
        q * (1. - 1e-6) + 1e-6
    }
}
#[derive(Debug)]
pub(super) struct SourceWindow {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub values: Vec<f64>,
}
impl SourceWindow {
    pub fn normalized(&self, metadata: &SourceMetadata) -> Vec<f32> {
        self.values
            .iter()
            .map(|&v| metadata.normalized(v))
            .collect()
    }
    fn copy_row(&mut self, x: u32, y: u32, values: &[f64]) {
        if y < self.y || y >= self.y + self.height {
            return;
        }
        let left = x.max(self.x);
        let right = (x + values.len() as u32).min(self.x + self.width);
        if left >= right {
            return;
        }
        let dst = ((y - self.y) * self.width + left - self.x) as usize;
        let src = (left - x) as usize;
        self.values[dst..dst + (right - left) as usize]
            .copy_from_slice(&values[src..src + (right - left) as usize]);
    }
}
struct Chunk {
    index: u32,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    values: Vec<f64>,
}
impl Chunk {
    fn copy_to(&self, window: &mut SourceWindow) {
        for row in 0..self.height {
            let start = (row * self.width) as usize;
            window.copy_row(
                self.x,
                self.y + row,
                &self.values[start..start + self.width as usize],
            );
        }
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct CacheStats {
    pub bytes: usize,
    pub hits: u64,
    pub decodes: u64,
    pub evictions: u64,
    pub uncached_decodes: u64,
}
pub(super) struct SourceGrid {
    decoder: Tiff,
    reader: ChunkReader,
    pub metadata: SourceMetadata,
    budget: usize,
    chunks: VecDeque<Chunk>,
    stats: CacheStats,
}
impl SourceGrid {
    /// Range must come from the already completed original-source import scan.
    /// Budget bounds retained decoded f64 chunk samples, not decoder row scratch
    /// or the separately bounded returned window (at most 8 MiB).
    pub fn open(
        path: &Path,
        elevation_range: [f64; 2],
        cache_budget: usize,
    ) -> Result<Self, String> {
        let [low, high] = elevation_range;
        if !low.is_finite() || !high.is_finite() || high < low || !(high - low).is_finite() {
            return Err("源高程范围无效".into());
        }
        let mut decoder =
            Decoder::new(BufReader::new(File::open(path).map_err(|e| e.to_string())?))
                .map_err(|e| e.to_string())?;
        let (width, height) = decoder.dimensions().map_err(|e| e.to_string())?;
        if width < 2 || height < 2 {
            return Err(format!(
                "TIFF 尺寸 {width}×{height} 无法构建地形，宽高均需至少 2 像元"
            ));
        }
        if decoder.get_tag_u32(Tag::Orientation).unwrap_or(1) != 1 {
            return Err("暂不支持含旋转 Orientation 标签的 TIFF，请先转为左上角原点".into());
        }
        if !matches!(
            decoder.colortype().map_err(|e| e.to_string())?,
            tiff::ColorType::Gray(_)
        ) || decoder.get_tag_u32(Tag::SamplesPerPixel).unwrap_or(1) != 1
        {
            return Err("源窗口目前仅支持单波段 DEM TIFF".into());
        }
        let float32 = decoder
            .get_tag_u16_vec(Tag::BitsPerSample)
            .ok()
            .and_then(|v| v.first().copied())
            == Some(32)
            && decoder
                .get_tag_u32_vec(Tag::SampleFormat)
                .ok()
                .is_some_and(|v| v.first() == Some(&3));
        let nodata = decoder
            .get_tag_ascii_string(Tag::GdalNodata)
            .ok()
            .and_then(|s| s.trim_matches('\0').trim().parse::<f64>().ok())
            .map(|v| if float32 { f64::from(v as f32) } else { v });
        let reader = ChunkReader::new(&mut decoder)?;
        Ok(Self {
            decoder,
            reader,
            metadata: SourceMetadata {
                width,
                height,
                nodata,
                elevation_range,
                normalized_input: false,
            },
            budget: cache_budget,
            chunks: VecDeque::new(),
            stats: CacheStats::default(),
        })
    }
    pub fn reduced_cells_align_with_chunks(&self, factor: u32) -> bool {
        let [width, _, chunk_width, _] = self.reader.grid();
        chunk_width >= width || chunk_width % factor == 0
    }
    pub fn cache_stats(&self) -> CacheStats {
        self.stats
    }
    /// One physical strip/tile scan. The callback may arrive in tile order rather
    /// than global row order; consumers must not assume completed rows are adjacent.
    pub fn stream_rows(
        &mut self,
        emit: impl FnMut(u32, u32, &[f64]) -> Result<(), String>,
    ) -> Result<(), String> {
        super::tiff_stream::read_rows(&mut self.decoder, emit)
    }
    /// Page coordinates, not pixel coordinates. Tail pages preserve real dimensions.
    pub fn read_page(&mut self, page_x: u32, page_y: u32) -> Result<SourceWindow, String> {
        let x = page_x.checked_mul(PAGE_EDGE).ok_or("源分页坐标溢出")?;
        let y = page_y.checked_mul(PAGE_EDGE).ok_or("源分页坐标溢出")?;
        if x >= self.metadata.width || y >= self.metadata.height {
            return Err("源分页超出范围".into());
        }
        self.read_window(
            x,
            y,
            PAGE_EDGE.min(self.metadata.width - x),
            PAGE_EDGE.min(self.metadata.height - y),
        )
    }
    /// No resampling, padding or edge clamping. Invalid rectangles fail explicitly.
    pub fn read_window(
        &mut self,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    ) -> Result<SourceWindow, String> {
        let right = x.checked_add(width).ok_or("源窗口坐标溢出")?;
        let bottom = y.checked_add(height).ok_or("源窗口坐标溢出")?;
        if width == 0
            || height == 0
            || right > self.metadata.width
            || bottom > self.metadata.height
            || u64::from(width) * u64::from(height) > MAX_WINDOW_PIXELS
        {
            return Err("源窗口超出范围或内存上限".into());
        }
        let mut window = SourceWindow {
            x,
            y,
            width,
            height,
            values: vec![f64::NAN; (u64::from(width) * u64::from(height)) as usize],
        };
        let [sw, sh, cw, ch] = self.reader.grid();
        let nx = sw.div_ceil(cw);
        for cy in y / ch..=(bottom - 1) / ch {
            for cx in x / cw..=(right - 1) / cw {
                let index = cy * nx + cx;
                if let Some(position) = self.chunks.iter().position(|c| c.index == index) {
                    self.stats.hits += 1;
                    let chunk = self.chunks.remove(position).unwrap();
                    chunk.copy_to(&mut window);
                    self.chunks.push_back(chunk);
                    continue;
                }
                self.stats.decodes += 1;
                let (chunk_x, chunk_y) = (cx * cw, cy * ch);
                let (chunk_w, chunk_h) = (cw.min(sw - chunk_x), ch.min(sh - chunk_y));
                let byte_count = u64::from(chunk_w) * u64::from(chunk_h) * 8;
                if byte_count > self.budget as u64 {
                    // Even a multi-gigabyte compressed strip uses only row scratch;
                    // decode once for this request, copying only the requested window.
                    self.stats.uncached_decodes += 1;
                    self.reader
                        .read(&mut self.decoder, index, |rx, ry, values| {
                            window.copy_row(rx, ry, values);
                            Ok(())
                        })?;
                    continue;
                }
                let bytes = byte_count as usize;
                while self.stats.bytes > self.budget - bytes {
                    let oldest = self.chunks.pop_front().ok_or("源分块缓存状态无效")?;
                    self.stats.bytes -= oldest.values.len() * 8;
                    self.stats.evictions += 1;
                }
                let mut chunk = Chunk {
                    index,
                    x: chunk_x,
                    y: chunk_y,
                    width: chunk_w,
                    height: chunk_h,
                    values: vec![f64::NAN; (byte_count / 8) as usize],
                };
                self.reader
                    .read(&mut self.decoder, index, |_, ry, values| {
                        let start = ((ry - chunk_y) * chunk_w) as usize;
                        chunk.values[start..start + chunk_w as usize].copy_from_slice(values);
                        Ok(())
                    })?;
                chunk.copy_to(&mut window);
                self.stats.bytes += bytes;
                self.chunks.push_back(chunk);
            }
        }
        Ok(window)
    }
}

/// Format-neutral source access for the existing paged terrain hierarchy.
/// PNG stays scanline-bounded, JPEG/ASC retain their existing decode budgets.
pub(super) struct ExportGrid {
    pub metadata: SourceMetadata,
    backend: ExportBackend,
}
enum ExportBackend {
    Tiff(SourceGrid),
    Image { path: std::path::PathBuf, scans: u64 },
}
impl ExportGrid {
    pub fn open(path: &Path, range: [f64; 2], budget: usize) -> Result<Self, String> {
        if is_tiff(path) {
            let grid=SourceGrid::open(path,range,budget)?;
            return Ok(Self {metadata:grid.metadata.clone(),backend:ExportBackend::Tiff(grid)});
        }
        let (width,height)=image_dimensions(path)?;
        Ok(Self {metadata:SourceMetadata {width,height,nodata:None,elevation_range:[0.,1.],normalized_input:true},
            backend:ExportBackend::Image {path:path.to_owned(),scans:0}})
    }
    pub fn reduced_cells_align_with_chunks(&self, factor:u32)->bool {
        match &self.backend {
            ExportBackend::Tiff(grid)=>grid.reduced_cells_align_with_chunks(factor),
            ExportBackend::Image {path,..}=> factor == 1 || image_extension(path)!="png"
                || png_reader(path).is_ok_and(|reader|!reader.info().interlaced),
        }
    }
    pub fn cache_stats(&self)->CacheStats {
        match &self.backend {ExportBackend::Tiff(grid)=>grid.cache_stats(),ExportBackend::Image {scans,..}=>CacheStats {decodes:*scans,..Default::default()}}
    }
    pub fn stream_rows(&mut self, emit:impl FnMut(u32,u32,&[f64])->Result<(),String>)->Result<(),String> {
        match &mut self.backend {
            ExportBackend::Tiff(grid)=>grid.stream_rows(emit),
            ExportBackend::Image {path,scans}=>{*scans+=1;image_rows(path,[self.metadata.width,self.metadata.height],emit)}
        }
    }
    pub fn read_window(&mut self,x:u32,y:u32,width:u32,height:u32)->Result<SourceWindow,String> {
        if let ExportBackend::Tiff(grid)=&mut self.backend { return grid.read_window(x,y,width,height); }
        if width==0 || height==0 || x.checked_add(width).is_none_or(|v|v>self.metadata.width)
            || y.checked_add(height).is_none_or(|v|v>self.metadata.height)
            || u64::from(width)*u64::from(height)>MAX_WINDOW_PIXELS {return Err("源窗口超出范围或内存上限".into());}
        let mut window=SourceWindow {x,y,width,height,values:vec![f64::NAN;(width*height) as usize]};
        self.stream_rows(|x,y,values|{window.copy_row(x,y,values);Ok(())})?;
        Ok(window)
    }
}
pub(super) fn is_tiff(path:&Path)->bool {
    path.extension().and_then(|e|e.to_str()).is_some_and(|e|e.eq_ignore_ascii_case("tif")||e.eq_ignore_ascii_case("tiff"))
}
fn image_extension(path:&Path)->String {
    path.extension().unwrap_or_default().to_string_lossy().to_ascii_lowercase()
}
fn png_reader(path:&Path)->Result<png::Reader<BufReader<File>>,String> {
    let mut decoder=png::Decoder::new(BufReader::new(File::open(path).map_err(|e|e.to_string())?));
    decoder.set_transformations(png::Transformations::EXPAND);
    decoder.read_info().map_err(|e|e.to_string())
}
fn asc_map(path:&Path)->Result<super::HeightMap,String> {
    use std::io::Read;
    let mut text=String::new();
    File::open(path).map_err(|e|e.to_string())?.take(super::raster::ASC_TEXT_BYTES+1)
        .read_to_string(&mut text).map_err(|e|e.to_string())?;
    if text.len() as u64>super::raster::ASC_TEXT_BYTES {return Err("ASC 文本超过 256 MiB，请转换为分块 TIFF".into());}
    super::heightmap::parse_asc(&text,path.file_name().unwrap_or_default().to_string_lossy().into_owned())
}
fn image_dimensions(path:&Path)->Result<(u32,u32),String> {
    let dimensions=match image_extension(path).as_str() {
        "png"=>{let reader=png_reader(path)?;(reader.info().width,reader.info().height)},
        "jpg"|"jpeg"=>{
            let mut decoder=jpeg_decoder::Decoder::new(BufReader::new(File::open(path).map_err(|e|e.to_string())?));
            decoder.set_max_decoding_buffer_size(super::raster::JPEG_DECODE_BYTES);
            decoder.read_info().map_err(|e|e.to_string())?;
            let info=decoder.info().ok_or("JPEG 缺少尺寸")?;
            let dimensions=(u32::from(info.width),u32::from(info.height));
            super::raster::jpeg_dimensions(dimensions.0,dimensions.1)?;
            dimensions
        },
        "asc"=>{let map=asc_map(path)?;(map.width,map.height)},
        _=>return Err("支持 JPG、PNG、TIFF 和 ASC".into()),
    };
    super::raster::dimensions(dimensions.0,dimensions.1)?;
    Ok(dimensions)
}
fn image_rows(path:&Path,expected:[u32;2],mut emit:impl FnMut(u32,u32,&[f64])->Result<(),String>)->Result<(),String> {
    match image_extension(path).as_str() {
        "png"=>{
            let mut reader=png_reader(path)?;
            let (w,h)=(reader.info().width,reader.info().height);
            if [w,h]!=expected {return Err("源文件尺寸已改变，请重新导入".into());}
            let interlaced=reader.info().interlaced;
            let (color,depth)=reader.output_color_type();
            if !matches!(depth,png::BitDepth::Eight|png::BitDepth::Sixteen) {return Err("PNG 位深未展开".into());}
            let bytes=if depth==png::BitDepth::Sixteen {2}else{1};
            let channels=color.samples();
            let passes:&[(u32,u32,u32,u32)]=if interlaced {&[(0,0,8,8),(4,0,8,8),(0,4,4,8),(2,0,4,4),(0,2,2,4),(1,0,2,2),(0,1,1,2)]}else{&[(0,0,1,1)]};
            for &(sx,sy,dx,dy) in passes {
                if sx>=w || sy>=h {continue;}
                for y in (sy..h).step_by(dy as usize) {
                    let row=reader.next_row().map_err(|e|e.to_string())?.ok_or("PNG 行数不足")?;
                    let mut values=Vec::with_capacity((w-sx).div_ceil(dx) as usize);
                    for i in 0..(w-sx).div_ceil(dx) as usize {
                        let offset=i*channels*bytes;
                        let value=|c:usize|->f32 {let p=offset+c*bytes;if bytes==2 {u16::from_be_bytes([row.data()[p],row.data()[p+1]]) as f32/65535.}else{f32::from(row.data()[p])/255.}};
                        let q=match color {png::ColorType::Grayscale|png::ColorType::GrayscaleAlpha=>value(0),_=>0.2126*value(0)+0.7152*value(1)+0.0722*value(2)};
                        values.push(f64::from(q));
                    }
                    if dx==1 {emit(sx,y,&values)?;} else {for (i,value) in values.iter().enumerate(){emit(sx+i as u32*dx,y,std::slice::from_ref(value))?;}}
                }
            }
            if reader.next_row().map_err(|e|e.to_string())?.is_some(){return Err("PNG 行数不符".into());}
        },
        "jpg"|"jpeg"=>{
            let mut decoder=jpeg_decoder::Decoder::new(BufReader::new(File::open(path).map_err(|e|e.to_string())?));
            decoder.set_max_decoding_buffer_size(super::raster::JPEG_DECODE_BYTES);
            decoder.read_info().map_err(|e|e.to_string())?;
            let info=decoder.info().ok_or("JPEG 缺少尺寸")?;
            let (w,h)=(u32::from(info.width),u32::from(info.height));
            if [w,h]!=expected {return Err("源文件尺寸已改变，请重新导入".into());}
            super::raster::jpeg_dimensions(w,h)?;
            let pixels=decoder.decode().map_err(|e|e.to_string())?;
            let info=decoder.info().ok_or("JPEG 缺少图像信息")?;
            let channels=info.pixel_format.pixel_bytes();
            for y in 0..h {
                let mut values=Vec::with_capacity(w as usize);
                for x in 0..w {
                    let i=(y as usize*w as usize+x as usize)*channels;
                    let q=match info.pixel_format {
                        jpeg_decoder::PixelFormat::L8=>f32::from(pixels[i])/255.,
                        jpeg_decoder::PixelFormat::L16=>u16::from_ne_bytes([pixels[i],pixels[i+1]]) as f32/65535.,
                        jpeg_decoder::PixelFormat::RGB24=>(0.2126*f32::from(pixels[i])+0.7152*f32::from(pixels[i+1])+0.0722*f32::from(pixels[i+2]))/255.,
                        jpeg_decoder::PixelFormat::CMYK32=>{let k=f32::from(pixels[i+3])/255.;(0.2126*(1.-f32::from(pixels[i])/255.)+0.7152*(1.-f32::from(pixels[i+1])/255.)+0.0722*(1.-f32::from(pixels[i+2])/255.))*(1.-k)},
                    };
                    values.push(f64::from(q));
                }
                emit(0,y,&values)?;
            }
        },
        "asc"=>{
            let map=asc_map(path)?;
            if [map.width,map.height]!=expected {return Err("源文件尺寸已改变，请重新导入".into());}
            for y in 0..map.height {
                let start=y as usize*map.width as usize;
                let values:Vec<_>=map.values[start..start+map.width as usize].iter().map(|v|f64::from(*v)).collect();
                emit(0,y,&values)?;
            }
        },
        _=>return Err("支持 JPG、PNG、TIFF 和 ASC".into()),
    }
    Ok(())
}
