import {validateNamedView} from './named-views.js';
import {interpolateSceneParameters,mergeSceneView,sceneTransitionDuration} from './scene-transition.js';
const clone=value=>JSON.parse(JSON.stringify(value));
export function normalizeVideoDocument(value) {
 if(!value||typeof value!=='object'||!value.settings||typeof value.settings!=='object')throw new Error('视频文档无效');
 const s=value.settings;
 if(!['views','motion'].includes(s.mode)||!['orbit','push','pull','sun'].includes(s.motion))throw new Error('视频路径无效');
 for(const [key,min,max,integer] of [['width',1,8192,true],['height',1,8192,true],['fps',1,60,true],['samples',1,4096,true],['duration',1,600,false]]){
  if(!Number.isFinite(s[key])||s[key]<min||s[key]>max||(integer&&!Number.isInteger(s[key])))throw new Error('视频参数无效：'+key);
 }
 const settings=Object.fromEntries(['mode','motion','duration','fps','samples','width','height'].map(key=>[key,s[key]]));
 if(value.timeline!==undefined&&!Array.isArray(value.timeline))throw new Error('视频时间轴无效');
 const timeline=(value.timeline??[]).map((row,index)=>{
  if(!row||typeof row.name!=='string'||!row.name.trim()||!Number.isFinite(row.duration)||row.duration<(index===0?0:.1)||row.duration>600)throw new Error('视频关键帧无效');
  return {name:row.name,view:validateNamedView(row.view),duration:row.duration};
 });
 if(timeline.length>200||videoDuration(timeline)>600)throw new Error('视频时间轴过长');
 return {settings,timeline};
}
export function videoDuration(timeline){return timeline.slice(1).reduce((sum,row)=>sum+Number(row.duration||0),0);}
export function buildVideoTimeline(settings,views,current){
  if(settings.mode==='views'){
    const rows=views.map(row=>({name:row.name,view:clone(row.view),duration:Math.max(.1,Number(row.duration)||3)}));
    if(rows.length<2)throw new Error('请选择至少两个视图');
    return rows;
  }
  const start=clone(current),end=clone(current),p=end.parameters;
  if(settings.motion==='sun'){
    const azimuth=Number(current.parameters.sunAzimuth??current.settings?.parameters?.sunAzimuth??135);
    for(const [view,offset] of [[start,-90],[end,90]]){
      const light={sunAltitude:5,sunAzimuth:((azimuth+offset)%360+360)%360};
      view.settings={...view.settings,parameters:{...view.settings?.parameters,...light}};
      for(const key of Object.keys(light))if(Object.hasOwn(view.parameters,key))view.parameters[key]=light[key];
    }
  }else if(settings.motion==='orbit')p.cameraYaw+=360;
  else p.cameraDistance=Math.max(.1,Math.min(1000,p.cameraDistance*(settings.motion==='push'?.55:1.8)));
  if(!Number.isFinite(Number(settings.duration))||Number(settings.duration)<1||Number(settings.duration)>600)throw new Error('视频时长无效');
  return [{name:'起点',view:start,duration:0},{name:'终点',view:end,duration:Number(settings.duration)||8,motion:settings.motion}];
}
export function sampleVideoTimeline(timeline,time){
  const duration=videoDuration(timeline);time=Math.max(0,Math.min(duration,time));
  let elapsed=0;
  for(let i=1;i<timeline.length;i++){
    const row=timeline[i],length=row.duration;
    if(time<=elapsed+length||i===timeline.length-1){
      const t=Math.max(0,Math.min(1,(time-elapsed)/length)),a=timeline[i-1].view,b=row.view;
      const from={...a.settings?.parameters,...a.parameters},to=mergeSceneView(from,b);
      const parameters=row.motion==='sun'?clone(from):interpolateSceneParameters(from,to,t);
      if(row.motion==='orbit')parameters.cameraYaw=from.cameraYaw+360*t;
      if(row.motion==='sun'){
        parameters.sunAzimuth=((from.sunAzimuth+180*t)%360+360)%360;
        parameters.sunAltitude=t===0||t===1?5:5+60*Math.sin(Math.PI*t);
      }
      const full={x:0,y:0,width:1,height:1},ca=a.frame?.crop??full,cb=b.frame?.crop??full;
      const frame=row.motion==='sun'?clone(a.frame??null):t===0?clone(a.frame??null):t===1?clone(b.frame??null):{ratio:0,crop:Object.fromEntries(['x','y','width','height'].map(key=>[key,ca[key]+(cb[key]-ca[key])*t]))};
      return {parameters,frame};
    }
    elapsed+=length;
  }
  return clone(timeline[0].view);
}
export function buildVideoFrames(settings,views,current){
  const timeline=buildVideoTimeline(settings,views,current),duration=videoDuration(timeline),fps=Number(settings.fps),count=Math.max(2,Math.round(duration*fps));
  if(!Number.isFinite(fps)||fps<1||fps>60||count>18000)throw new Error('视频时长或帧率无效');
  return Array.from({length:count},(_,i)=>sampleVideoTimeline(timeline,duration*i/(count-1)));
}
export function timelineKeyTimes(rows){let time=0;return rows.map((row,index)=>{if(index)time+=Number(row.duration)||0;return time;});}
export function moveTimelineKey(rows,index,time){
  if(index<=0||index>=rows.length)return rows;
  const times=timelineKeyTimes(rows),next=times[index+1]??600;
  times[index]=Math.max(times[index-1]+.1,Math.min(next-.1,Number(time)||0));
  return rows.map((row,i)=>({...row,duration:i?Number((times[i]-times[i-1]).toFixed(3)):0}));
}
export function createVideoWorkflow({document,getViews,getCurrentView,onPreview=()=>{},onExport=()=>{},onCancel=()=>{},onOverlayChange=()=>{},onError=console.warn}){
 const host=document.querySelector('#video-workflow');
 host.innerHTML=`<div class="video-preview"><img id="video-preview-image" alt="视频预览" draggable="false"></div><div class="video-controls"><label>路径<select data-v="mode" aria-label="视频路径"><option value="views" selected>视图关键帧</option><option value="motion">默认运镜</option></select></label><label data-motion>运镜<select data-v="motion" aria-label="默认运镜"><option value="orbit">环绕</option><option value="push">推进</option><option value="pull">拉远</option><option value="sun">日照变化</option></select></label><label data-motion>时长<input data-v="duration" aria-label="视频时长" type="number" min="1" max="600" value="8"></label><label>帧率<select data-v="fps" aria-label="视频帧率"><option>24</option><option selected>30</option><option>60</option></select></label><label>分辨率<select data-v="resolution" aria-label="视频分辨率"><option value="1280x720">1280 × 720</option><option value="1920x1080" selected>1920 × 1080</option><option value="1080x1920">1080 × 1920</option><option value="1080x1080">1080 × 1080</option></select></label><label>采样<input data-v="samples" aria-label="视频采样" type="number" min="1" max="4096" value="256"></label></div><section class="video-sequence"><div class="video-add"><select aria-label="插入视图"></select><button type="button" aria-label="插入视图" title="插入视图">＋</button><span data-key-name></span><input data-key-time type="number" min="0" max="600" step=".1" aria-label="关键帧时间"><button data-delete-key type="button" aria-label="删除关键帧" title="删除关键帧">×</button></div><div class="video-timeline-scroll"><div class="video-timeline" role="group" aria-label="视频时间轴"><div class="video-ruler"></div><div class="video-track"></div><div class="video-playhead"></div></div></div></section><div class="video-playback"><button type="button" data-play aria-label="播放预览">▶</button><input type="range" data-time min="0" max="8" value="0" step=".01" aria-label="视频时间"><output data-clock>0.0 / 8.0 s</output></div><div class="video-export-progress" hidden><progress max="100" value="0" aria-label="视频导出进度"></progress><output data-progress></output></div><div class="video-export"><button type="button" data-cancel hidden>取消</button><button type="button" class="primary" data-export>导出 MP4</button></div>`;
 const q=s=>host.querySelector(s),fields=Object.fromEntries([...host.querySelectorAll('[data-v]')].map(e=>[e.dataset.v,e]));
 let rows=[],available=[],playing=false,raf=null,start=0,time=0,busy=false,active=false,selected=-1;
 const extent=()=>Math.max(8,length()+1.5);
 function getSettings(){const [width,height]=fields.resolution.value.split('x').map(Number);return {mode:fields.mode.value,motion:fields.motion.value,duration:Number(fields.duration.value),fps:Number(fields.fps.value),samples:Number(fields.samples.value),width,height};}
 function getTimeline(){return buildVideoTimeline(getSettings(),rows,getCurrentView());}
 function length(){if(fields.mode.value==='views')return videoDuration(rows);return Number(fields.duration.value)||8;}
 function clock(){const duration=length();time=Math.min(time,duration);q('[data-time]').max=duration;q('[data-time]').value=time;q('[data-clock]').textContent=`${time.toFixed(1)} / ${duration.toFixed(1)} s`;q('.video-playhead').style.left=`${time/extent()*100}%`;}
 function stopPreview(){playing=false;if(raf!==null)cancelAnimationFrame(raf);raf=null;q('[data-play]').textContent='▶';q('[data-play]').setAttribute('aria-label','播放预览');}
 function preview(){try{const result=onPreview({timeline:getTimeline(),settings:getSettings(),time,playing});if(result?.catch)result.catch(error=>{stopPreview();onError(error);});}catch(error){stopPreview();onError(error);}}
 function selection(){const row=rows[selected];q('[data-key-name]').textContent=row?.name??'';q('[data-key-time]').value=row?timelineKeyTimes(rows)[selected].toFixed(1):'';q('[data-key-time]').disabled=busy||selected<=0;q('[data-delete-key]').disabled=busy||!row;}
 function render(){
  const views=fields.mode.value==='views';q('.video-add button').disabled=busy||!available.length;host.querySelectorAll('[data-motion]').forEach(e=>e.hidden=views);q('.video-sequence').hidden=!views;
  const track=q('.video-track'),ruler=q('.video-ruler');track.replaceChildren();ruler.replaceChildren();
  q('.video-timeline').style.minWidth=`${Math.max(640,extent()*70)}px`;
  const step=extent()>60?10:extent()>20?5:1;
  for(let second=0;second<=extent();second+=step){const tick=document.createElement('span');tick.style.left=`${second/extent()*100}%`;tick.textContent=`${second}s`;ruler.append(tick);}
  timelineKeyTimes(rows).forEach((at,index)=>{const row=rows[index],key=document.createElement('button');key.type='button';key.className='video-key'+(selected===index?' selected':'');key.style.left=`${at/extent()*100}%`;key.setAttribute('aria-label',`${row.name} ${at.toFixed(1)} 秒`);key.title=`${row.name} · ${at.toFixed(1)}s`;key.disabled=busy;
   if(typeof row.thumbnail==='string'&&/^data:image\/jpeg;base64,/.test(row.thumbnail)){const image=document.createElement('img');image.src=row.thumbnail;image.alt='';image.draggable=false;key.append(image);}const name=document.createElement('span');name.textContent=row.name;key.append(name);
   key.addEventListener('click',()=>{selected=index;time=timelineKeyTimes(rows)[index];render();if(active)preview();});
   key.addEventListener('pointerdown',event=>{if(busy||index===0)return;event.stopPropagation();stopPreview();selected=index;const initial=event.clientX,atStart=timelineKeyTimes(rows)[index],scale=extent()/q('.video-timeline').getBoundingClientRect().width;key.setPointerCapture(event.pointerId);const move=e=>{rows=moveTimelineKey(rows,index,atStart+(e.clientX-initial)*scale);time=timelineKeyTimes(rows)[index];key.style.left=`${time/extent()*100}%`;selection();clock();if(active)preview();};const end=()=>{key.removeEventListener('pointermove',move);key.removeEventListener('pointerup',end);key.removeEventListener('pointercancel',end);render();};key.addEventListener('pointermove',move);key.addEventListener('pointerup',end);key.addEventListener('pointercancel',end);});track.append(key);
  });selection();clock();onOverlayChange();
 }
 function refresh({reset=false}={}){if(reset){stopPreview();rows=[];selected=-1;time=0;}available=getViews()??[];const select=q('.video-add select');select.replaceChildren();available.forEach((row,index)=>{const option=document.createElement('option');option.value=index;option.textContent=row.name;select.append(option);});q('.video-add button').disabled=busy||!available.length;render();}
 q('.video-add button').addEventListener('click',()=>{const row=available[Number(q('.video-add select').value)];if(!row)return;stopPreview();const times=timelineKeyTimes(rows);let at=rows.length?time:0;if(rows.length&&times.some(value=>Math.abs(value-at)<.1)){const prior=rows.at(-1).view,from={...prior.settings?.parameters,...prior.parameters},to=mergeSceneView(from,row.view);at=times.at(-1)+Math.max(.3,Math.round(sceneTransitionDuration(from,to)/10)/100);}const index=times.findIndex(value=>value>at),insert=index<0?rows.length:index;const records=rows.map((value,i)=>({...value,at:times[i]}));records.splice(insert,0,{...clone(row),at});rows=records.map((value,i)=>{const {at,...rest}=value;return {...rest,duration:i?at-records[i-1].at:0};});selected=insert;time=at;render();if(active&&rows.length>=2)preview();});
 q('[data-key-time]').addEventListener('change',()=>{stopPreview();rows=moveTimelineKey(rows,selected,Number(q('[data-key-time]').value));time=timelineKeyTimes(rows)[selected]??0;render();if(active&&rows.length>=2)preview();});
 q('[data-delete-key]').addEventListener('click',()=>{stopPreview();const times=timelineKeyTimes(rows);rows=rows.filter((_,i)=>i!==selected).map((row,i)=>({...row,at:times.filter((_,j)=>j!==selected)[i]}));const origin=rows[0]?.at??0;rows=rows.map((row,i,list)=>{const {at,...rest}=row;return {...rest,duration:i?at-list[i-1].at:0};});selected=Math.min(selected,rows.length-1);time=Math.max(0,time-origin);render();if(active&&rows.length>=2)preview();});
 Object.values(fields).forEach(e=>e.addEventListener('change',()=>{stopPreview();render();if(active&&(fields.mode.value!=='views'||rows.length>=2))preview();}));
 function scrub(value){stopPreview();time=Math.max(0,Math.min(length(),value));clock();if(rows.length>=2||fields.mode.value!=='views')preview();}
 q('[data-time]').addEventListener('input',()=>scrub(Number(q('[data-time]').value)));
 q('.video-timeline').addEventListener('pointerdown',event=>{if(busy||event.target.closest('.video-key'))return;const timeline=q('.video-timeline'),seek=e=>{const rect=timeline.getBoundingClientRect();scrub((e.clientX-rect.left)/rect.width*extent());};timeline.setPointerCapture(event.pointerId);seek(event);const end=()=>{timeline.removeEventListener('pointermove',seek);timeline.removeEventListener('pointerup',end);timeline.removeEventListener('pointercancel',end);};timeline.addEventListener('pointermove',seek);timeline.addEventListener('pointerup',end);timeline.addEventListener('pointercancel',end);});
 q('[data-play]').addEventListener('click',()=>{if(playing){stopPreview();return;}if(length()<=0){onError(new Error('请选择至少两个视图'));return;}playing=true;if(time>=length())time=0;start=performance.now()-time*1000;q('[data-play]').textContent='Ⅱ';q('[data-play]').setAttribute('aria-label','暂停预览');const step=now=>{if(!playing)return;time=Math.min(length(),(now-start)/1000);clock();preview();if(time>=length())stopPreview();else raf=requestAnimationFrame(step);};raf=requestAnimationFrame(step);});
 q('[data-export]').addEventListener('click',async()=>{try{stopPreview();const timeline=getTimeline();setProgress(0);await onExport({timeline,settings:getSettings()});}catch(error){finish();onError(error);}});
 q('[data-cancel]').addEventListener('click',()=>onCancel());
 function setProgress(value){busy=true;host.querySelectorAll('input,select,button').forEach(e=>e.disabled=true);q('[data-cancel]').disabled=false;q('[data-cancel]').hidden=false;q('.video-export-progress').hidden=false;const percent=Math.max(0,Math.min(100,value));q('progress').value=percent;q('[data-progress]').textContent=`${percent.toFixed(0)}%`;}
 function finish(){busy=false;host.querySelectorAll('input,select,button').forEach(e=>e.disabled=false);q('[data-cancel]').hidden=true;q('.video-export-progress').hidden=true;render();}
 function getDocument(){return {settings:getSettings(),timeline:clone(rows)};}
 function setDocument(value){
  if(busy)throw new Error('正在导出视频');
  const next=normalizeVideoDocument({...value,timeline:value?.timeline===undefined?rows:value.timeline});
  stopPreview();
  for(const [key,value] of Object.entries(next.settings)){
   if(key==='width'||key==='height')continue;
   const input=fields[key];
   if(key==='fps'&&![...input.options].some(o=>o.value===String(value))){const option=document.createElement('option');option.value=String(value);option.textContent=String(value);input.append(option);}
   input.value=String(value);
  }
  const resolution=`${next.settings.width}x${next.settings.height}`;
  if(![...fields.resolution.options].some(o=>o.value===resolution)){const option=document.createElement('option');option.value=resolution;option.textContent=`${next.settings.width} × ${next.settings.height}`;fields.resolution.append(option);}
  fields.resolution.value=resolution;rows=next.timeline;selected=-1;time=0;render();return getDocument();
 }
 return {getDocument,setDocument,setPreview(url){q('#video-preview-image').src=url;},setActive(value){active=!!value;host.hidden=!active;if(active)refresh();else stopPreview();onOverlayChange();},getSettings,getTimeline,getFrames(){return buildVideoFrames(getSettings(),rows,getCurrentView());},setProgress,finish,refresh,stopPreview,isBusy:()=>busy};
}
