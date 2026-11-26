import {sampleVideoTimeline,videoDuration} from './video-workflow.js';
import {freezeCurrentView} from './export-settings.js';
import {fitFrame} from './export-frame.js';
import {nativeHeightScaling} from './height-scaling.js';
import {nativeRamp} from './state.js';
export function videoFrameParameters(sample,base,viewport,settings,{preview=false,terrainScale='source'}={}){
  const p={...base};
  for(const key of ['transitionCloudSeeds','_transitionHeight','transitionBackgroundFrom','transitionBackgroundMix','transitionSkyFollow','transitionSkyModel'])delete p[key];
  Object.assign(p,sample.parameters);
  const frame=sample.frame?fitFrame(sample.frame,viewport.width,viewport.height):null;
  const camera=freezeCurrentView({...p,...nativeHeightScaling(p),...nativeRamp(p),surface:p.surface==='polygon'?'poly':p.surface},viewport,frame?.crop);
  const scale=preview?Math.min(1,480/Math.max(settings.width,settings.height)):1;
  return {...camera,width:Math.max(2,Math.round(settings.width*scale/2)*2),height:Math.max(2,Math.round(settings.height*scale/2)*2),samples:preview?4:settings.samples,
    outputBitDepth:8,exportTransparent:false,exportOrthographic:false,exportGeoreference:false,exportShadow:false,exportNormal:false,exportDepth:false,exportShadowLayer:false,
    exposure:1,gamma:2.2,outputContrast:0,outputSaturation:0,outputTemperature:0,outputShadows:0,outputHighlights:0,
    appearancePick:false,contourEnabled:false,exportTerrainScale:preview?'preview':terrainScale};
}
export function timelineFrames(timeline,settings,base,viewport,terrainScale='source'){
  const duration=videoDuration(timeline),count=Math.round(duration*settings.fps);
  if(!Number.isFinite(duration)||duration<=0||!Number.isInteger(settings.fps)||settings.fps<1||settings.fps>60||count<2||count>18000)throw new Error('视频时长或帧率无效');
  if(!Number.isInteger(settings.samples)||settings.samples<1||settings.samples>4096)throw new Error('视频采样无效');
  return Array.from({length:count},(_,i)=>videoFrameParameters(sampleVideoTimeline(timeline,duration*i/(count-1)),base,viewport,settings,{terrainScale}));
}
export function createVideoRendering({invoke,getBase,getViewport,getTerrainScale,onPreview,onProgress,onBusy,onComplete,onFinish,flush,onError}){
  let pending=null,previewBusy=false,previewPromise=Promise.resolve(),revision=0,url=null,busy=false;
  function stopPreview(){pending=null;revision++;}
  function preview(request){
    if(busy)return;
    pending=request;
    if(previewBusy)return;
    const run=async()=>{
      previewBusy=true;
      try{while(pending&&!busy){
        const request=pending;pending=null;const token=revision;
        const frame=videoFrameParameters(sampleVideoTimeline(request.timeline,request.time),getBase(),getViewport(),request.settings,{preview:true});
        const bytes=await invoke('renderer_video_preview',{params:frame});
        if(token!==revision||busy)continue;
        const next=URL.createObjectURL(new Blob([new Uint8Array(bytes)],{type:'image/png'}));onPreview(next);if(url)URL.revokeObjectURL(url);url=next;
      }}catch(error){pending=null;onError(error);}finally{previewBusy=false;}
    };
    previewPromise=run();
  }
  return {preview,stopPreview,async export({timeline,settings}){
    if(busy)return;
    stopPreview();busy=true;onBusy(true);let timer=null,polling=false;
    try{
      await previewPromise;await flush();
      const frames=timelineFrames(timeline,settings,getBase(),getViewport(),getTerrainScale());
      timer=setInterval(async()=>{if(polling)return;polling=true;try{const p=await invoke('renderer_video_progress');if(p?.total)onProgress(p.completed/p.total*100);}catch{}finally{polling=false;}},300);
      const path=await invoke('renderer_export_video',{frames,fps:settings.fps});await onComplete(path);
    }catch(error){if(!/已取消.*导出/.test(String(error?.message??error)))throw error;}
    finally{if(timer!==null)clearInterval(timer);busy=false;onBusy(false);await onFinish();}
  }};
}
