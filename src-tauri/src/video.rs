use std::{path::{Path,PathBuf},sync::{Arc,atomic::{AtomicBool,AtomicU32,Ordering}}};
use serde::Serialize;
use windows::{core::PCWSTR,Win32::{Media::MediaFoundation::*,System::Com::*}};
#[derive(Default,Clone)]
pub struct VideoState { pub busy:Arc<AtomicBool>,pub cancel:Arc<AtomicBool>,pub completed:Arc<AtomicU32>,pub total:Arc<AtomicU32> }
#[derive(Serialize)]
#[serde(rename_all="camelCase")]
pub struct VideoProgress {pub busy:bool,pub completed:u32,pub total:u32}
impl VideoState {pub fn progress(&self)->VideoProgress{VideoProgress{busy:self.busy.load(Ordering::Acquire),completed:self.completed.load(Ordering::Acquire),total:self.total.load(Ordering::Acquire)}}}
struct MediaRuntime;
impl MediaRuntime {fn start()->Result<Self,String>{unsafe{CoInitializeEx(None,COINIT_MULTITHREADED).ok().map_err(|e|e.to_string())?;if let Err(e)=MFStartup(MF_VERSION,MFSTARTUP_FULL){CoUninitialize();return Err(e.to_string())}Ok(Self)}}}
impl Drop for MediaRuntime {fn drop(&mut self){unsafe{let _=MFShutdown();CoUninitialize();}}}
pub struct Encoder {writer:IMFSinkWriter,stream:u32,width:u32,height:u32,fps:u32,index:u32,_runtime:MediaRuntime}
impl Encoder {
 pub fn new(path:&Path,width:u32,height:u32,fps:u32)->Result<Self,String>{(unsafe{Self::create(path,width,height,fps)}).map_err(|e|format!("视频编码初始化失败：{e}"))}
 unsafe fn create(path:&Path,width:u32,height:u32,fps:u32)->Result<Self,String>{
  let runtime=MediaRuntime::start()?;
  let url:Vec<u16>=path.as_os_str().encode_wide().chain(Some(0)).collect();
  let result=(||->windows::core::Result<_>{
   let writer=MFCreateSinkWriterFromURL(PCWSTR(url.as_ptr()),None,None)?;
   let output=MFCreateMediaType()?;
   output.SetGUID(&MF_MT_MAJOR_TYPE,&MFMediaType_Video)?;
   output.SetGUID(&MF_MT_SUBTYPE,&MFVideoFormat_H264)?;
   output.SetUINT32(&MF_MT_AVG_BITRATE,((width as u64*height as u64*fps as u64)/5).clamp(1_000_000,40_000_000) as u32)?;
   output.SetUINT32(&MF_MT_INTERLACE_MODE,MFVideoInterlace_Progressive.0 as u32)?;
   output.SetUINT64(&MF_MT_FRAME_SIZE,((width as u64)<<32)|height as u64)?;
   output.SetUINT64(&MF_MT_FRAME_RATE,((fps as u64)<<32)|1)?;
   output.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO,(1u64<<32)|1)?;
   let stream=writer.AddStream(&output)?;
   let input=MFCreateMediaType()?;
   input.SetGUID(&MF_MT_MAJOR_TYPE,&MFMediaType_Video)?;
   input.SetGUID(&MF_MT_SUBTYPE,&MFVideoFormat_RGB32)?;
   input.SetUINT32(&MF_MT_INTERLACE_MODE,MFVideoInterlace_Progressive.0 as u32)?;
   input.SetUINT64(&MF_MT_FRAME_SIZE,((width as u64)<<32)|height as u64)?;
   input.SetUINT64(&MF_MT_FRAME_RATE,((fps as u64)<<32)|1)?;
   input.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO,(1u64<<32)|1)?;
   input.SetUINT32(&MF_MT_DEFAULT_STRIDE,width*4)?;
   writer.SetInputMediaType(stream,&input,None)?;
   writer.BeginWriting()?;
   Ok((writer,stream))
  })().map_err(|e|e.to_string())?;
  Ok(Self{writer:result.0,stream:result.1,width,height,fps,index:0,_runtime:runtime})
 }
 pub fn write(&mut self,rgba:&[u8])->Result<(),String>{
  if rgba.len()!=self.width as usize*self.height as usize*4{return Err("视频帧尺寸不一致".into())}
  unsafe{(   ||->windows::core::Result<()>{
   let buffer=MFCreateMemoryBuffer(rgba.len() as u32)?;let mut ptr=std::ptr::null_mut();buffer.Lock(&mut ptr,None,None)?;
   let target=std::slice::from_raw_parts_mut(ptr,rgba.len());
   for (src,dst) in rgba.chunks_exact(4).zip(target.chunks_exact_mut(4)){dst.copy_from_slice(&[src[2],src[1],src[0],255]);}
   buffer.Unlock()?;buffer.SetCurrentLength(rgba.len() as u32)?;
   let sample=MFCreateSample()?;sample.AddBuffer(&buffer)?;
   let start=self.index as i64*10_000_000/self.fps as i64;
   let end=(self.index as i64+1)*10_000_000/self.fps as i64;
   sample.SetSampleTime(start)?;sample.SetSampleDuration(end-start)?;self.writer.WriteSample(self.stream,&sample)?;Ok(())
  })().map_err(|e|format!("视频帧编码失败：{e}"))?;}
  self.index+=1;Ok(())
 }
 pub fn finish(self)->Result<(),String>{unsafe{self.writer.Finalize().map_err(|e|format!("视频封装失败：{e}"))}}
}
use std::os::windows::ffi::OsStrExt;
pub fn temporary_path(path:&Path)->PathBuf{path.with_file_name(format!(".demstudio-video-{}-{}.mp4",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos()))}
