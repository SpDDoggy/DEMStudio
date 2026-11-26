export function normalizeHex(value) {
  const raw=String(value).trim().replace(/^#/, '');
  if (/^[0-9a-f]{3}$/i.test(raw)) return '#'+[...raw].map(c=>c+c).join('').toLowerCase();
  return /^[0-9a-f]{6}$/i.test(raw)?'#'+raw.toLowerCase():null;
}
export function rgbToHsv(r,g,b) {
  r/=255;g/=255;b/=255;const max=Math.max(r,g,b),min=Math.min(r,g,b),d=max-min;
  let h=0;if(d)h=max===r?((g-b)/d)%6:max===g?(b-r)/d+2:(r-g)/d+4;
  return [(h*60+360)%360,max?d/max:0,max];
}
export function hsvToHex(h,s,v) {
  const c=v*s,x=c*(1-Math.abs(((h%360)/60)%2-1)),m=v-c;
  const rgb=h<60?[c,x,0]:h<120?[x,c,0]:h<180?[0,c,x]:h<240?[0,x,c]:h<300?[x,0,c]:[c,0,x];
  return '#'+rgb.map(n=>Math.round((n+m)*255).toString(16).padStart(2,'0')).join('');
}
export function createColorPicker({document:doc=globalThis.document,onOverlayChange=()=>{},isLocked=()=>false}={}) {
  const win=doc.defaultView;let active=null;
  const node=(tag,cls)=>{const n=doc.createElement(tag);n.className=cls;return n;};
  function current(){if(!active)return null;if(active.target.isConnected)return active.target;
    const next=[...doc.querySelectorAll('input[type="color"]')].find(n=>active.id?n.id===active.id:n.getAttribute('aria-label')===active.label);
    if(next)active.target=next;return next;
  }
  function close(focus=false){if(!active)return;const target=current();active.popup.remove();active=null;onOverlayChange();if(focus)target?.focus();}
  function position(){if(!active)return;const target=current();if(!target||target.disabled||isLocked()){close();return;}const r=target.getBoundingClientRect(),b=active.popup.getBoundingClientRect();
    const owner=target.closest('.palette-panel,.panel,.camera-popover,#background');
    const anchor=owner?.getBoundingClientRect()??r;
    const left=anchor.right+10+b.width<=win.innerWidth-8?anchor.right+10:owner?anchor.left-b.width-10:r.right+10;
    active.popup.style.left=`${Math.max(8,Math.min(left,win.innerWidth-b.width-8))}px`;
    active.popup.style.top=`${Math.max(8,Math.min(r.top,win.innerHeight-b.height-8))}px`;onOverlayChange();
  }
  function paint(hex,emit=true,keepHue=false){hex=normalizeHex(hex);if(!active||!hex)return;
    const rgb=[1,3,5].map(i=>parseInt(hex.slice(i,i+2),16)),hsv=rgbToHsv(...rgb);if(!keepHue)active.h=hsv[1]?hsv[0]:active.h;
    active.s=hsv[1];active.v=hsv[2];active.hex.value=hex.toUpperCase();active.hex.setCustomValidity('');
    active.rgb.forEach((n,i)=>{n.value=rgb[i];n.setCustomValidity('');});active.hue.value=active.h;
    active.sv.style.backgroundColor=hsvToHex(active.h,1,1);active.cursor.style.left=`${active.s*100}%`;active.cursor.style.top=`${(1-active.v)*100}%`;active.preview.style.background=hex;
    if(emit){let target=current();if(!target||target.disabled||isLocked()){close();return;}if(target.value===hex)return;
      target.value=hex;target.dispatchEvent(new win.Event('input',{bubbles:true}));target=current();if(target){target.value=hex;target.dispatchEvent(new win.Event('change',{bubbles:true}));}}
  }
  function open(target){close();if(target.disabled||isLocked())return;const popup=node('div','color-picker-popup');popup.setAttribute('data-native-overlay','');popup.setAttribute('role','dialog');popup.setAttribute('aria-label','颜色');
    const sv=node('div','color-picker-sv');sv.tabIndex=0;sv.setAttribute('role','slider');sv.setAttribute('aria-label','饱和度与明度');const cursor=node('i','color-picker-cursor');sv.append(cursor);
    const hue=node('input','color-picker-hue');hue.type='range';hue.min=0;hue.max=359;hue.step=1;hue.setAttribute('aria-label','色相');
    const preview=node('div','color-picker-preview');const row=node('div','color-picker-rgb');const rgb=['R','G','B'].map(label=>{const wrap=node('label','');const field=node('input','');field.type='number';field.min=0;field.max=255;field.step=1;field.setAttribute('aria-label',label);const text=node('span','');text.textContent=label;wrap.append(field,text);row.append(wrap);return field;});
    const hexRow=node('label','color-picker-hex');const hexLabel=node('span','');hexLabel.textContent='HEX';const hex=node('input','');hex.type='text';hex.maxLength=7;hex.spellcheck=false;hex.setAttribute('aria-label','HEX');hexRow.append(hexLabel,hex);
    popup.append(sv,hue,preview,row,hexRow);(target.closest('.palette-panel')??target.closest('dialog[open]')??doc.body).append(popup);
    active={target,id:target.id,label:target.getAttribute('aria-label'),popup,sv,cursor,hue,preview,rgb,hex,h:0,s:0,v:0};paint(target.value,false);position();
    const readHex=expand=>{const color=normalizeHex(hex.value);hex.setCustomValidity(color?'':'HEX');if(color&&(expand||hex.value.trim().replace(/^#/, '').length===6))paint(color);};
    hex.addEventListener('input',()=>readHex(false));hex.addEventListener('change',()=>readHex(true));hex.addEventListener('keydown',e=>{if(e.key==='Enter'){e.preventDefault();readHex(true);}});
    rgb.forEach(field=>field.addEventListener('input',()=>{const valid=rgb.every(n=>/^\d+$/.test(n.value)&&Number(n.value)<=255);field.setCustomValidity(valid?'':'RGB');if(valid)paint('#'+rgb.map(n=>Number(n.value).toString(16).padStart(2,'0')).join(''));}));
    hue.addEventListener('input',()=>{active.h=Number(hue.value);paint(hsvToHex(active.h,active.s,active.v),true,true);});
    const sample=e=>{if(!active)return;const r=sv.getBoundingClientRect();active.s=Math.min(1,Math.max(0,(e.clientX-r.left)/r.width));active.v=1-Math.min(1,Math.max(0,(e.clientY-r.top)/r.height));paint(hsvToHex(active.h,active.s,active.v),true,true);};
    sv.addEventListener('pointerdown',e=>{if(e.button!==0)return;e.preventDefault();sv.setPointerCapture(e.pointerId);sample(e);sv.onpointermove=sample;});
    const stop=()=>{sv.onpointermove=null;};sv.addEventListener('pointerup',stop);sv.addEventListener('pointercancel',stop);
    sv.addEventListener('keydown',e=>{if(!active||!['ArrowLeft','ArrowRight','ArrowUp','ArrowDown'].includes(e.key))return;e.preventDefault();const d=e.shiftKey?.1:.01;active.s=Math.max(0,Math.min(1,active.s+(e.key==='ArrowRight'?d:e.key==='ArrowLeft'?-d:0)));active.v=Math.max(0,Math.min(1,active.v+(e.key==='ArrowUp'?d:e.key==='ArrowDown'?-d:0)));paint(hsvToHex(active.h,active.s,active.v),true,true);});
    hex.focus();
  }
  doc.addEventListener('click',event=>{const target=event.target.closest?.('input[type="color"]');if(!target)return;event.preventDefault();open(target);},true);
  doc.addEventListener('pointerdown',event=>{if(active&&!active.popup.contains(event.target)&&event.target!==current())close();},true);
  doc.addEventListener('keydown',event=>{if(active&&event.key==='Escape'){event.preventDefault();event.stopPropagation();close(true);}},true);
  win.addEventListener('resize',position);doc.addEventListener('scroll',position,true);
  return {close,sync(){if(active){if(isLocked()||!current())close();else position();}},containsOwnedPopup:(target,owner)=>!!active?.popup.contains(target)&&(!owner||!!owner.contains(current()))};
}
