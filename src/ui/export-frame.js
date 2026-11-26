const clamp=(value,min,max)=>Math.max(min,Math.min(max,value));
const finite=(value,fallback)=>Number.isFinite(Number(value))?Number(value):fallback;
export function normalizeFrame(frame={}) {
  frame=frame??{};
  const crop=frame.crop||{};
  const width=clamp(finite(crop.width,.8),.02,1),height=clamp(finite(crop.height,.8),.02,1);
  return {ratio:Math.max(0,finite(frame.ratio,0)),crop:{x:clamp(finite(crop.x,(1-width)/2),0,1-width),y:clamp(finite(crop.y,(1-height)/2),0,1-height),width,height}};
}
export function fitFrame(frame,viewportWidth,viewportHeight,ratio=frame.ratio) {
  const next=normalizeFrame({...frame,ratio});
  if(!next.ratio||viewportWidth<=0||viewportHeight<=0)return next;
  const c=next.crop,cx=c.x+c.width/2,cy=c.y+c.height/2,normalizedRatio=next.ratio*viewportHeight/viewportWidth;
  let width=c.width,height=width/normalizedRatio;
  if(height>c.height){height=c.height;width=height*normalizedRatio;}
  c.width=width;c.height=height;c.x=clamp(cx-width/2,0,1-width);c.y=clamp(cy-height/2,0,1-height);
  return next;
}
export function moveFrame(frame,dx,dy){const next=normalizeFrame(frame);next.crop.x=clamp(next.crop.x+dx,0,1-next.crop.width);next.crop.y=clamp(next.crop.y+dy,0,1-next.crop.height);return next;}
export function resizeFrame(frame,corner,dx,dy,viewportWidth,viewportHeight){
  const next=normalizeFrame(frame),c=next.crop,left=corner.includes('w'),top=corner.includes('n');
  const ax=left?c.x+c.width:c.x,ay=top?c.y+c.height:c.y;
  let width=clamp(c.width+(left?-dx:dx),.02,left?ax:1-ax),height=clamp(c.height+(top?-dy:dy),.02,top?ay:1-ay);
  if(next.ratio){const r=next.ratio*viewportHeight/viewportWidth; if(Math.abs(dx)>Math.abs(dy)){height=width/r;}else{width=height*r;} const s=Math.min(1,(left?ax:1-ax)/width,(top?ay:1-ay)/height);width*=s;height*=s;}
  c.width=width;c.height=height;c.x=left?ax-width:ax;c.y=top?ay-height:ay;return next;
}
export function createExportFrame({document,onConfirm=()=>{},onCancel=()=>{},onOverlayChange=()=>{},isLocked=()=>false}){
  const win=document.defaultView,viewport=document.querySelector('#viewport');
  let frame=normalizeFrame(),opened=false,drag=null;
  const root=document.createElement('div');root.className='export-frame-mode';root.hidden=true;
  root.innerHTML=`<div class="export-frame-outline">${['n','e','s','w'].map(edge=>`<div class="export-frame-edge edge-${edge}" data-native-overlay></div>`).join('')}${['nw','ne','sw','se'].map(corner=>`<div class="export-frame-handle handle-${corner}" data-corner="${corner}" data-native-overlay></div>`).join('')}</div><div class="export-frame-toolbar glass" data-native-overlay><span>导出视口</span><select aria-label="长宽比"><option value="0">自由</option><option value="1">1:1</option><option value="1.3333333333333333">4:3</option><option value="1.5">3:2</option><option value="1.7777777777777777">16:9</option><option value="0.5625">9:16</option><option value="custom">自定义</option></select><span class="export-frame-custom" hidden><input type="number" min="1" value="16" aria-label="比例宽度"><span>:</span><input type="number" min="1" value="9" aria-label="比例高度"></span><span class="divider"></span><button type="button" class="icon" data-action="cancel" title="取消" aria-label="取消"><svg viewBox="0 0 24 24"><path d="m6 6 12 12M18 6 6 18"/></svg></button><button type="button" class="icon" data-action="confirm" title="确认视口" aria-label="确认视口"><svg viewBox="0 0 24 24"><path d="m5 12 4 4L19 6"/></svg></button></div>`;
  document.body.append(root);
  const outline=root.querySelector('.export-frame-outline'),toolbar=root.querySelector('.export-frame-toolbar'),select=root.querySelector('select'),custom=root.querySelector('.export-frame-custom'),inputs=[...custom.querySelectorAll('input')];
  const rect=()=>viewport.getBoundingClientRect();
  function paint(){if(!opened)return;const v=rect(),c=frame.crop;Object.assign(outline.style,{left:`${v.left+c.x*v.width}px`,top:`${v.top+c.y*v.height}px`,width:`${c.width*v.width}px`,height:`${c.height*v.height}px`});toolbar.style.left=`${v.left+v.width/2}px`;toolbar.style.top=`${Math.max(v.top+48,v.top+v.height-126)}px`;onOverlayChange();}
  function close(){opened=false;drag=null;root.hidden=true;onOverlayChange();}
  function cancel(){if(isLocked())return;close();onCancel();}
  function confirm(){if(isLocked())return;const result=normalizeFrame(frame);close();onConfirm(result);}
  root.addEventListener('pointerdown',event=>{if(event.button!==0||isLocked())return;const target=event.target.closest('.export-frame-edge,.export-frame-handle');if(!target)return;event.preventDefault();event.stopPropagation();drag={x:event.clientX,y:event.clientY,frame:normalizeFrame(frame),corner:target.dataset.corner};target.setPointerCapture?.(event.pointerId);});
  root.addEventListener('pointermove',event=>{if(!drag)return;const v=rect(),dx=(event.clientX-drag.x)/v.width,dy=(event.clientY-drag.y)/v.height;frame=drag.corner?resizeFrame(drag.frame,drag.corner,dx,dy,v.width,v.height):moveFrame(drag.frame,dx,dy);paint();});
  root.addEventListener('pointerup',()=>{drag=null;});root.addEventListener('pointercancel',()=>{drag=null;});
  root.querySelector('[data-action=cancel]').addEventListener('click',cancel);root.querySelector('[data-action=confirm]').addEventListener('click',confirm);
  function ratioChanged(){if(isLocked())return;custom.hidden=select.value!=='custom';const ratio=select.value==='custom'?Number(inputs[0].value)/Number(inputs[1].value):Number(select.value);if(!Number.isFinite(ratio)||ratio<0)return;const v=rect();frame=fitFrame(frame,v.width,v.height,ratio);paint();}
  select.addEventListener('change',ratioChanged);inputs.forEach(input=>input.addEventListener('change',ratioChanged));
  document.addEventListener('keydown',event=>{if(!opened||event.defaultPrevented)return;if(event.key==='Escape'){event.preventDefault();cancel();}else if(event.key==='Enter'&&!event.target.closest?.('input,select,textarea,[contenteditable=true]')){event.preventDefault();confirm();}});
  win.addEventListener('resize',()=>{if(!opened)return;const v=rect();frame=fitFrame(frame,v.width,v.height);paint();});
  return {open(value){if(isLocked())return;frame=fitFrame(normalizeFrame(value),rect().width,rect().height);const option=[...select.options].find(option=>option.value!=='custom'&&Math.abs(Number(option.value)-frame.ratio)<1e-8);select.value=option?option.value:'custom';custom.hidden=!!option;if(!option){inputs[0].value=String(Math.round(frame.ratio*1000));inputs[1].value='1000';}opened=true;root.hidden=false;paint();},close,isOpen:()=>opened,getFrame:()=>normalizeFrame(frame)};
}
