import { rampStops } from './state.js';
import { defaultPaletteDocument, normalizePaletteDocument, palettePreview } from './palette-library.js';
export function createPalettePanel({select,getParameters,onApply,invoke,onError=()=>{},onOverlayChange=()=>{},isLocked=()=>false}) {
  const doc=select.ownerDocument,win=doc.defaultView;
  select.dataset.customCombo='palette';select.hidden=true;
  const trigger=doc.createElement('button');trigger.type='button';trigger.className='palette-trigger';trigger.setAttribute('aria-label','色带');trigger.setAttribute('aria-expanded','false');trigger.setAttribute('aria-haspopup','dialog');
  const thumbnail=doc.createElement('span');thumbnail.className='palette-thumbnail';
  const title=doc.createElement('span');trigger.append(thumbnail,title);select.after(trigger);
  let library=normalizePaletteDocument(defaultPaletteDocument),panel=null,editing=null,busy=false,lastMode=null;
  const copy=stops=>stops.map(s=>({...s}));
  function element(tag,className,text) {const node=doc.createElement(tag);if(className)node.className=className;if(text!==undefined)node.textContent=text;return node;}
  const icons={
    '导入':'M12 3v12m-4-4 4 4 4-4M5 16v4h14v-4',
    '导出':'M12 16V4m-4 4 4-4 4 4M5 16v4h14v-4',
    '新建':'M12 5v14M5 12h14','添加色标':'M12 5v14M5 12h14',
    '关闭':'m6 6 12 12M6 18 18 6','取消':'m6 6 12 12M6 18 18 6',
    '编辑':'m15 5 4 4M4 20l4-1L20 7a2.8 2.8 0 0 0-4-4L4 15v5',
    '保存':'m5 12 4 4L19 6','−':'M5 12h14'
  };
  function button(text,action) {
    const node=element('button',icons[text]?'palette-icon':'',icons[text]?undefined:text);node.type='button';
    if(icons[text]){node.title=text;node.setAttribute('aria-label',text);node.innerHTML=`<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="${icons[text]}"/></svg>`;}
    node.addEventListener('click',action);return node;
  }
  function current() {const p=getParameters(),stops=p.colorRamp==='custom'&&p.rampStops?.length?p.rampStops:rampStops(p);return library.palettes.find(item=>JSON.stringify(item.stops)===JSON.stringify(stops))??library.palettes.find(item=>item.id===p.colorRamp);}
  function sync() {const p=getParameters();title.textContent=current()?.name??'自定义';thumbnail.style.background=palettePreview(rampStops(p),p.rampInterpolation,p.rampReverse);trigger.disabled=select.disabled||isLocked();if(trigger.disabled)close();if(panel&&!editing&&lastMode!==p.rampInterpolation)render();}
  function position() {if(!panel)return;const rect=trigger.getBoundingClientRect(),width=Math.min(400,win.innerWidth-16);panel.style.width=`${width}px`;panel.style.left=`${Math.max(8,Math.min(rect.right+12,win.innerWidth-width-8))}px`;panel.style.top=`${Math.max(8,Math.min(rect.top-120,win.innerHeight-460))}px`;panel.style.maxHeight=`${Math.max(80,win.innerHeight-parseFloat(panel.style.top)-8)}px`;onOverlayChange();}
  function close() {if(!panel)return;panel.remove();panel=null;editing=null;trigger.setAttribute('aria-expanded','false');onOverlayChange();}
  async function run(action) {if(busy)return;busy=true;try{await action();}catch(error){onError(error);}finally{busy=false;}}
  async function persist(next) {next=normalizePaletteDocument(next);await invoke('palette_library_save',{document:next});library=next;}
  function edit(palette) {const source=copy(palette.stops),low=source[0].position,span=source.at(-1).position-low;editing={...palette,stops:source.map(s=>({...s,position:span>0?(s.position-low)/span:s.position}))};render();}
  function render() {
    if(!panel)return;lastMode=getParameters().rampInterpolation;panel.replaceChildren();
    const head=element('div','palette-heading');head.append(element('strong','',editing?'编辑配色':'配色'),button('关闭',close));panel.append(head);
    if(editing) {
      const form=element('form','palette-editor');
      const name=element('input');name.value=editing.name;name.required=true;name.maxLength=80;name.setAttribute('aria-label','配色名称');name.addEventListener('input',()=>editing.name=name.value);form.append(name);
      const preview=element('div','palette-preview');preview.style.background=palettePreview(editing.stops,getParameters().rampInterpolation);form.append(preview);
      editing.stops.forEach((stop,index)=>{const row=element('div','palette-stop');const color=element('input');color.type='color';color.value=stop.color;color.setAttribute('aria-label',`色标 ${index+1} 颜色`);const hex=element('input');hex.value=stop.color.toUpperCase();hex.pattern='#[0-9a-fA-F]{6}';hex.required=true;hex.setAttribute('aria-label',`色标 ${index+1} HEX`);const pos=element('input');pos.type='number';pos.min='0';pos.max='100';pos.step='0.1';pos.value=String(Math.round(stop.position*1000)/10);pos.required=true;pos.setAttribute('aria-label',`色标 ${index+1} 位置`);color.addEventListener('input',()=>{stop.color=color.value;hex.value=color.value.toUpperCase();preview.style.background=palettePreview(editing.stops,getParameters().rampInterpolation);});hex.addEventListener('input',()=>{if(/^#[\da-f]{6}$/i.test(hex.value)){stop.color=hex.value.toLowerCase();color.value=stop.color;preview.style.background=palettePreview(editing.stops,getParameters().rampInterpolation);}});pos.addEventListener('input',()=>stop.position=Number(pos.value)/100);const remove=button('−',()=>{editing.stops.splice(index,1);render();});remove.disabled=editing.stops.length<=2;row.append(color,hex,pos,element('span','','%'),remove);form.append(row);});
      const actions=element('div','palette-actions');const add=button('添加色标',()=>{const sorted=[...editing.stops].sort((a,b)=>a.position-b.position);let best=0;for(let i=1;i<sorted.length;i++)if(sorted[i].position-sorted[i-1].position>sorted[best+1].position-sorted[best].position)best=i-1;editing.stops.push({position:(sorted[best].position+sorted[best+1].position)/2,color:sorted[best].color});editing.stops.sort((a,b)=>a.position-b.position);render();});add.disabled=editing.stops.length>=16;const save=button('保存',()=>{});save.className+=' palette-save';save.type='submit';actions.append(add,button('取消',()=>{editing=null;render();}),save);form.append(actions);form.addEventListener('submit',event=>{event.preventDefault();run(async()=>{const saved={...editing,stops:copy(editing.stops)};await persist({version:1,palettes:library.palettes.some(p=>p.id===saved.id)?library.palettes.map(p=>p.id===saved.id?saved:p):[...library.palettes,saved]});editing=null;render();sync();});});panel.append(form);
    }else {
      const actions=element('div','palette-actions');actions.append(button('导入',()=>run(async()=>{const imported=await invoke('palette_library_import');if(imported){library=normalizePaletteDocument(imported);render();sync();}})),button('导出',()=>run(()=>invoke('palette_library_export',{document:library}))),button('新建',()=>edit({id:`user-${Date.now()}`,name:'新配色',stops:copy(rampStops(getParameters()))})));head.insertBefore(actions,head.children[1]);
      const list=element('div','palette-list');for(const palette of library.palettes){const row=element('div','palette-item');const choose=button('',()=>{if(isLocked())return;onApply(copy(palette.stops));sync();close();});choose.className='palette-choice';const preview=element('span','palette-preview');preview.style.background=palettePreview(palette.stops,getParameters().rampInterpolation);choose.append(element('span','',palette.name),preview);const editButton=button('编辑',()=>edit(palette));editButton.className+=' palette-edit';editButton.setAttribute('aria-label',`编辑 ${palette.name}`);row.append(choose,editButton);list.append(row);}panel.append(list);
    }
    position();
  }
  function open() {if(trigger.disabled||isLocked())return;panel=element('section','palette-panel');panel.setAttribute('role','dialog');panel.setAttribute('aria-label','配色');panel.setAttribute('data-native-overlay','');doc.body.append(panel);trigger.setAttribute('aria-expanded','true');render();}
  trigger.addEventListener('click',()=>panel?close():open());
  doc.addEventListener('pointerdown',event=>{if(panel&&!panel.contains(event.target)&&!trigger.contains(event.target)&&!event.target.closest?.('.color-picker'))close();});
  doc.addEventListener('keydown',event=>{if(panel&&event.key==='Escape'){event.preventDefault();event.stopPropagation();close();trigger.focus();}});
  win.addEventListener('resize',position);doc.addEventListener('scroll',event=>{if(panel&&!panel.contains(event.target))position();},true);
  Promise.resolve().then(()=>invoke('palette_library_load',{defaults:defaultPaletteDocument})).then(result=>{library=normalizePaletteDocument(result);sync();}).catch(onError);
  sync();return {sync,close,containsOwnedPopup:target=>Boolean(panel?.contains(target)||trigger.contains(target))};
}
