import {rampStops,resampleRamp} from './state.js';
import {classifyRamp,validRampRange} from './ramp-classification.js';

export function createRampWorkbench({container,getParameters,getRange,getDistribution,onChange,isLocked=()=>false,onError=()=>{}}) {
  let method='custom',centerEnabled=false,center=null;
  const node=(tag,className,text)=>{const el=document.createElement(tag);if(className)el.className=className;if(text!==undefined)el.textContent=text;return el;};
  const guarded=action=>()=>{if(isLocked())return;try{action();}catch(error){onError(error);}};
  const field=(parent,label,control)=>{const row=node('label','ramp-workbench-field');row.append(node('span','',label),control);parent.append(row);return control;};
  function select(label,options,value,change) {
    const el=node('select');el.setAttribute('aria-label',label);for(const [key,text] of Object.entries(options)){const option=node('option','',text);option.value=key;el.append(option);}el.value=value;el.disabled=isLocked();el.addEventListener('change',guarded(()=>change(el.value)));return el;
  }
  function button(text,action) {const el=node('button','',text);el.type='button';el.disabled=isLocked();el.addEventListener('click',guarded(action));return el;}
  function classify(stops,count,mode) {
    const result=classifyRamp({method,count,range:getRange(),distribution:getDistribution?.(),stops,mode,center:method==='equal'&&centerEnabled?center:null}).stops;
    if(method!=='custom'&&getParameters().rampReverse) {
      const colors=result.map(stop=>stop.color).reverse();
      return result.map((stop,index)=>({...stop,color:colors[index]}));
    }
    return result;
  }
  function classificationPatch(stops) {
    return {colorRamp:'custom',rampStops:stops,...(method==='custom'?{}:{rampReverse:false,rampOffset:0,rampWrap:false})};
  }
  function resize(stops,count,mode) {return method==='custom'?resampleRamp(stops,count,mode):classify(stops,count,mode);}
  function manualEdit(){method='custom';render();}
  function render() {
    const p=getParameters(),range=getRange(),locked=isLocked();container.replaceChildren();container.classList.add('ramp-workbench');
    const classification=node('div','ramp-workbench-classification');
    field(classification,'分级',select('分级方式',{equal:'等间距',quantile:'分位数',custom:'自定义'},method,value=>{method=value;render();}));
    classification.append(button('应用',()=>{const stops=classify(rampStops(p),rampStops(p).length,p.rampInterpolation);onChange(classificationPatch(stops));render();}));container.append(classification);
    const centerRow=node('div','ramp-workbench-center');const toggle=node('input');toggle.type='checkbox';toggle.checked=centerEnabled;toggle.disabled=locked||method!=='equal';toggle.setAttribute('aria-label','启用中心高程');toggle.addEventListener('change',guarded(()=>{centerEnabled=toggle.checked;if(center===null&&validRampRange(range))center=(range[0]+range[1])/2;render();}));
    const centerLabel=node('label');centerLabel.append(toggle,node('span','','中心'));centerRow.append(centerLabel);
    const value=node('input');value.type='number';value.step='any';value.setAttribute('aria-label','中心高程');value.value=center??(validRampRange(range)?(range[0]+range[1])/2:'');value.disabled=locked||method!=='equal'||!centerEnabled;value.addEventListener('change',guarded(()=>{center=value.valueAsNumber;}));centerRow.append(value);container.append(centerRow);
    const distribution=getDistribution?.();
    if(distribution?.bins?.length) {
      const histogram=node('div','ramp-workbench-distribution');histogram.append(node('span','','预览高程分布'));
      const svg=document.createElementNS('http://www.w3.org/2000/svg','svg');svg.setAttribute('viewBox','0 0 256 40');svg.setAttribute('role','img');svg.setAttribute('aria-label','预览高程分布');
      const bins=distribution.bins,max=Math.max(...bins,1);const path=document.createElementNS('http://www.w3.org/2000/svg','path');let d='';bins.forEach((count,i)=>{const x=i*256/bins.length,w=256/bins.length,h=Math.max(0,count)*38/max;d+=`M${x},40v${-h}h${w}v${h}z`;});path.setAttribute('d',d);svg.append(path);histogram.append(svg);container.append(histogram);
      if(validRampRange(distribution.range)) {
        const [low,high]=distribution.range;
        if(validRampRange(range)&&!p.rampWrap) {
          const positions=rampStops(p).map(stop=>stop.position);
          if(p.rampInterpolation==='discrete')positions.push(1);
          for(const position of positions) {
            const source=p.rampReverse?1-position-(p.rampOffset??0):position+(p.rampOffset??0);
            const height=range[0]+source*(range[1]-range[0]);
            if(height<low||height>high)continue;
            const x=(height-low)/(high-low)*256;
            const line=document.createElementNS('http://www.w3.org/2000/svg','line');
            line.setAttribute('x1',String(x));line.setAttribute('x2',String(x));line.setAttribute('y1','0');line.setAttribute('y2','40');line.setAttribute('class','ramp-workbench-boundary');line.setAttribute('data-height',String(height));svg.append(line);
          }
        }
        const axis=node('div','ramp-workbench-axis');axis.append(node('span','',Number(low.toFixed(2)).toLocaleString()),node('span','',Number(high.toFixed(2)).toLocaleString()));histogram.append(axis);
      }
    }

  }
  return {render,resize,manualEdit,classificationPatch,getMethod:()=>method};
}
