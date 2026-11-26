import {rampStops, resampleRamp, rampGradient, sampleRampColor} from './state.js';

export function createRampEditor({container,getParameters,getRange=()=>null,onChange,resizeStops=resampleRamp,onError=()=>{},isLocked=()=>false,onPickHeight,onPickTexture,canPickHeight=()=>true,canPickTexture=()=>true}) {
  let selected = 0, editing = null, samplingOpen = false;
  container.classList.add('professional-ramp');
  function commit(stops,kind="color") { editing=null; onChange(stops,{kind}); render(); }
  function resize(stops,count,mode) {try {commit(resizeStops(stops,count,mode),"resize");} catch(error) {render();onError(error);}}
  const element=(tag,className,text)=>{const node=document.createElement(tag);if(className)node.className=className;if(text!==undefined)node.textContent=text;return node;};
  const button=(text,label,handler)=>{const node=element('button','',text);node.type='button';node.setAttribute('aria-label',label);node.disabled=isLocked();node.addEventListener('click',handler);return node;};
  function render() {
    const parameters=getParameters(), mode=parameters.rampInterpolation, discrete=mode==='discrete';
    let stops=rampStops(parameters);
    const displayPosition=position=>parameters.rampReverse?1-position:position;
    selected=Math.min(selected,stops.length-1);
    container.replaceChildren();
    const card=element('div','ramp-editor-card');container.append(card);
    const header=element('div','ramp-count-row');header.append(element('span','',discrete?'分级':'色标'));
    const minus=button('−','减少一级',()=>{selected=Math.max(0,selected-1);resize(stops,stops.length-1,mode);});minus.disabled||=stops.length<=2;
    const count=element('input','ramp-count');count.type='number';count.min=2;count.max=16;count.step=1;count.value=stops.length;count.setAttribute('aria-label',discrete?'分级数量':'色标数量');count.disabled=isLocked();count.addEventListener('input',()=>{const value=count.valueAsNumber;if(Number.isInteger(value)&&value>=2&&value<=16&&value!==stops.length)resize(stops,value,mode);});count.addEventListener('change',()=>{resize(stops,count.valueAsNumber,mode);});
    const plus=button('+','增加一级',()=>resize(stops,stops.length+1,mode));plus.disabled||=stops.length>=16;const controls=element('div','ramp-count-controls');controls.append(minus,count,plus);header.append(controls);card.append(header);
    const preview=element('div','ramp-track');preview.id='ramp-preview';preview.style.background=rampGradient(stops,mode,parameters.rampReverse);preview.setAttribute('aria-label','色带');
    stops.forEach((stop,index)=>{
      const handle=button('',`色标 ${index+1}`,()=>{selected=index;render();});handle.className='ramp-node';handle.dataset.rampIndex=String(index);handle.style.left=`${displayPosition(stop.position)*100}%`;handle.style.background=stop.color;handle.setAttribute('aria-pressed',String(selected===index));
      handle.addEventListener('pointerdown',event=>{
        if(event.button!==0 || isLocked() || discrete&&index===0)return;
        selected=index;event.preventDefault();handle.setPointerCapture(event.pointerId);let next=stops.map(item=>({...item}));
        handle.onpointermove=e=>{const bounds=preview.getBoundingClientRect();const lower=index?next[index-1].position+.0001:0,upper=index+1<next.length?next[index+1].position-.0001:discrete?.9999:1;next[index].position=Math.min(upper,Math.max(lower,displayPosition((e.clientX-bounds.left)/bounds.width)));handle.style.left=`${displayPosition(next[index].position)*100}%`;preview.style.background=rampGradient(next,mode,parameters.rampReverse);};
        handle.onpointerup=()=>{handle.onpointermove=null;handle.onpointerup=null;commit(next,"position");};
      });preview.append(handle);
    });card.append(preview);
    const range=getRange();const validRange=!parameters.rampWrap&&Array.isArray(range)&&Number.isFinite(range[0])&&range[1]>range[0];
    const sourcePosition=position=>parameters.rampReverse?1-position-(parameters.rampOffset??0):position+(parameters.rampOffset??0);
    const colorPosition=value=>{const q=(value-range[0])/(range[1]-range[0]);return (parameters.rampReverse?1-q:q)-(parameters.rampOffset??0);};
    const format=position=>validRange?Number((range[0]+sourcePosition(position)*(range[1]-range[0])).toFixed(2)).toLocaleString():`${Number((position*100).toFixed(2))}%`;
    const list=element('div','ramp-class-list');
    stops.forEach((stop,index)=>{
      const row=element('div',`ramp-class-row${selected===index?' selected':''}`);
      row.dataset.rampIndex=String(index);
      const swatch=element('input','ramp-swatch');swatch.type='color';swatch.value=stop.color;swatch.disabled=isLocked();swatch.setAttribute('aria-label',`第 ${index+1} ${discrete?'级':'色标'}颜色`);swatch.addEventListener('click',()=>{selected=index;});swatch.addEventListener('change',()=>{const next=stops.map(item=>({...item}));next[index].color=swatch.value;commit(next);});
      const end=stops[index+1]?.position??1;
      const interval=parameters.rampReverse&&validRange?[end,stop.position]:[stop.position,end];
      const label=button(discrete?`${format(interval[0])} – ${format(interval[1])}`:format(stop.position),`选择色标 ${index+1}`,()=>{selected=index;editing=index;render();container.querySelector?.('.ramp-position')?.focus();container.querySelector?.('.ramp-position')?.select();});label.className='ramp-class-value';
      const remove=button('×',`删除色标 ${index+1}`,()=>{let next=stops.filter((_,i)=>i!==index);if(discrete)next[0].position=0;selected=Math.max(0,index-1);commit(next,"delete");});remove.disabled||=stops.length<=2;remove.className='ramp-delete';
      row.append(swatch,label,remove);
      if(editing===index){
    const position=element('input','ramp-position');position.type='number';position.step='any';position.value=validRange?Number((range[0]+sourcePosition(stop.position)*(range[1]-range[0])).toFixed(5)):Number((stop.position*100).toFixed(3));position.setAttribute('aria-label',validRange?'高程值':'位置 (%)');position.disabled=isLocked()||discrete&&index===0;
        let committedPosition=Number(position.value);
        const commitPosition=()=>{
          if(position.disabled||position.valueAsNumber===committedPosition)return;
          const value=validRange?colorPosition(position.valueAsNumber):position.valueAsNumber/100;
          const lower=index?stops[index-1].position:0,upper=index+1<stops.length?stops[index+1].position:discrete?1:1.000001;
          if(!Number.isFinite(value)||value<0||value>=upper||index&&value<=lower){position.setCustomValidity('位置无效');position.reportValidity();return;}
          position.setCustomValidity('');committedPosition=position.valueAsNumber;
          const next=stops.map(item=>({...item}));next[index].position=value;commit(next,"position");
        };
        position.addEventListener('change',commitPosition);
        position.addEventListener('blur',()=>{commitPosition();editing=null;render();});
        position.addEventListener('keydown',event=>{if(event.key==='Enter'){event.preventDefault();commitPosition();editing=null;render();}else if(event.key==='Escape'){event.preventDefault();editing=null;render();}});
        label.hidden=true;
        const tail=element('span','ramp-position-tail',discrete?` – ${format(end)}`:validRange?'':'%');
        row.replaceChildren(swatch,label,position,tail,remove);
      }
      list.append(row);
    });card.append(list);
    if(onPickHeight||onPickTexture){const details=element('details','ramp-sampling');details.open=samplingOpen;const summary=element('summary','','取样');details.addEventListener('toggle',()=>samplingOpen=details.open);details.append(summary);if(onPickHeight){const pick=button('地形高程','取地形高程',()=>onPickHeight(selected));pick.id='pick-ramp-height';pick.disabled||=!canPickHeight();pick.setAttribute('aria-pressed',String(!!parameters.appearancePick));details.append(pick);}if(onPickTexture){const pick=button('贴图颜色','取贴图颜色',()=>onPickTexture(color=>{const next=rampStops(getParameters());next[Math.min(selected,next.length-1)].color=color;commit(next);}));pick.id='pick-texture-color';pick.disabled||=!canPickTexture();details.append(pick);}container.append(details);}
  }
  return {render, addStop(position) {
    if(isLocked()||getParameters().rampInterpolation==='discrete')return;
    const stops=rampStops(getParameters());
    if(stops.length>=16||stops.some(stop=>Math.abs(stop.position-position)<.0001))return;
    const stop={position:Math.max(0,Math.min(1,position)),color:sampleRampColor(stops,position)};
    stops.push(stop);stops.sort((a,b)=>a.position-b.position);selected=stops.indexOf(stop);commit(stops,'position');
  }, equalize() {
    if(isLocked())return;const stops=rampStops(getParameters());
    const divisor=getParameters().rampInterpolation==='discrete'?stops.length:stops.length-1;
    commit(stops.map((stop,index)=>({...stop,position:index/divisor})),'position');
  }, getSelected:()=>selected, select:index=>{selected=index;editing=index;render();}, setSamplePosition(position) {
    const stops=rampStops(getParameters());const found=stops.findIndex(stop=>Math.abs(stop.position-position)<.0001);
    if(found>=0){selected=found;render();return;}
    if(getParameters().rampInterpolation==='discrete'&&selected===0){selected=stops.reduce((best,stop,index)=>Math.abs(stop.position-position)<Math.abs(stops[best].position-position)?index:best,0);render();return;}
    const updated=stops.map(stop=>({...stop}));updated[selected].position=Math.max(0,Math.min(getParameters().rampInterpolation==='discrete'?.9999:1,position));const chosen=updated[selected];updated.sort((a,b)=>a.position-b.position);selected=updated.indexOf(chosen);commit(updated,"position");
  }};
}
