import {normalizeParameter} from './state.js';
import {validateSceneSettings} from './scene-transition.js';

export const namedViewsStorageKey = 'demstudio.named-views.v1';
export const builtinViewSourceKey = 'builtin';
export const viewParameterKeys = Object.freeze(['projection','cameraYaw','cameraPitch','cameraDistance','cameraPan','fieldOfView','cameraFrameX','cameraFrameY']);
const clone = value => JSON.parse(JSON.stringify(value));
function sourceKey(value) {
  if (typeof value !== 'string' || !value.trim()) throw new Error('当前地形不可保存视图');
  return value.trim();
}
export function validateNamedView(value) {
  if (!value?.parameters || typeof value.parameters !== 'object') throw new Error('视图无效');
  const parameters = Object.fromEntries(viewParameterKeys.map(key=>{
    const input=value.parameters[key]??(key.startsWith('cameraFrame')?0:undefined);
    if(key.startsWith('cameraFrame')) {
      if(!Number.isFinite(input)||Math.abs(input)>10000)throw new Error('镜头偏移无效');
      return [key,input];
    }
    const normalized=normalizeParameter(key,input);
    return [key,typeof input==='number'?input:normalized];
  }));
  let frame = null;
  if (value.frame != null) {
    if (!Number.isFinite(value.frame.ratio) || value.frame.ratio < 0 || value.frame.ratio > 100) throw new Error('取景比例无效');
    frame = {ratio:value.frame.ratio};
    if (value.frame.crop != null) {
      const crop=value.frame.crop;
      if (!['x','y','width','height'].every(key=>Number.isFinite(crop[key])) || crop.x<0 || crop.y<0 || crop.width<=0 || crop.height<=0 || crop.x+crop.width>1.000001 || crop.y+crop.height>1.000001) throw new Error('取景范围无效');
      frame.crop=Object.fromEntries(['x','y','width','height'].map(key=>[key,crop[key]]));
    }
  }
  return {parameters,frame,...(value.settings!=null?{settings:validateSceneSettings(value.settings)}:{})};
}
export function createNamedViewsStore(storage) {
  function read() {
    const raw=storage.getItem(namedViewsStorageKey);
    if (raw===null) return {version:1,sources:{}};
    const value=JSON.parse(raw);
    if (value?.version!==1 || !value.sources || typeof value.sources!=='object' || Array.isArray(value.sources)) throw new Error('视图记录无效');
    return value;
  }
  function records(value,key) {
    const rows=Object.hasOwn(value.sources,key)?value.sources[key]:[];
    if (!Array.isArray(rows)) throw new Error('视图记录无效');
    return rows.map(row=>{
      if (typeof row.name!=='string' || !row.name.trim() || row.name.length>80) throw new Error('视图名称无效');
      return {name:row.name,view:validateNamedView(row.view),...(validThumbnail(row.thumbnail)?{thumbnail:row.thumbnail}:{})};
    });
  }
  return {
    list(key) {return records(read(),sourceKey(key));},
    save(key,name,view,thumbnail) {
      key=sourceKey(key);name=String(name??'').trim();
      if (!name || name.length>80) throw new Error('请输入视图名称');
      const document=read(),rows=records(document,key),entry={name,view:validateNamedView(view),...(validThumbnail(thumbnail)?{thumbnail}:{})};
      const index=rows.findIndex(row=>row.name===name);
      if(index<0)rows.push(entry);else rows[index]=entry;
      Object.defineProperty(document.sources,key,{value:rows,enumerable:true,writable:true,configurable:true});
      storage.setItem(namedViewsStorageKey,JSON.stringify(document));
      return clone(entry);
    },
    saveMany(key,entries) {
      key=sourceKey(key);
      if(!Array.isArray(entries)||!entries.length||entries.length>200)throw new Error('视图批次无效');
      const document=read(),rows=records(document,key),names=new Set(rows.map(row=>row.name));
      const checked=entries.map(row=>{
        const name=typeof row?.name==='string'?row.name.trim():'';
        if(!name||name.length>80||names.has(name))throw new Error('视图名称重复或无效');
        names.add(name);return {name,view:validateNamedView(row.view)};
      });
      Object.defineProperty(document.sources,key,{value:[...rows,...checked],enumerable:true,writable:true,configurable:true});
      storage.setItem(namedViewsStorageKey,JSON.stringify(document));
      return clone(checked);
    },
    remove(key,name) {
      key=sourceKey(key);const document=read(),rows=records(document,key);
      Object.defineProperty(document.sources,key,{value:rows.filter(row=>row.name!==name),enumerable:true,writable:true,configurable:true});
      storage.setItem(namedViewsStorageKey,JSON.stringify(document));
    },
  };
}

function validThumbnail(value){return typeof value==='string' && value.length<160000 && /^data:image\/jpeg;base64,[A-Za-z0-9+/=]+$/.test(value);}

const icons={save:'<path d="M4 3h11l3 3v12H3V3z"/><path d="M7 3v6h7V3M7 18v-6h7v6"/>',apply:'<path d="m8 4 6 6-6 6"/>',remove:'<path d="M3 5h14M8 2h4M5 5l1 13h8l1-13M8 8v7M12 8v7"/>',close:'<path d="m5 5 10 10M15 5 5 15"/>',confirm:'<path d="m4 10 4 4 8-8"/>'};
export function createNamedViews({document,storage,getSourceKey,getView,applyView,captureThumbnail=async()=>null,onOverlayChange=()=>{},isLocked=()=>false,onError=console.warn}) {
  const store=createNamedViewsStore(storage),panel=document.querySelector('#named-views');
  const button=(action,title)=>`<button type="button" class="icon" data-view-action="${action}" title="${title}" aria-label="${title}"><svg viewBox="0 0 20 20" aria-hidden="true">${icons[action]}</svg></button>`;
  panel.innerHTML=`<header><span>视图</span>${button('save','保存视图')}</header><label class="named-view-speed"><span>速度</span><input data-view-speed type="range" min="0.25" max="5" step="0.05" value="1" aria-label="视图切换速度"><output>1×</output></label><form class="named-view-editor" hidden><input aria-label="视图名称" maxlength="80" autocomplete="off" placeholder="视图名称">${button('confirm','保存')}${button('close','取消')}</form><div class="named-view-list"></div>`;
  const list=panel.querySelector('.named-view-list'),form=panel.querySelector('form'),input=form.querySelector('input');
  const speed=panel.querySelector('[data-view-speed]'),speedOutput=panel.querySelector('.named-view-speed output');
  speed.addEventListener('input',()=>{speedOutput.textContent=`${Number(speed.value).toFixed(2).replace(/0+$/,'').replace(/\.$/,'')}×`;});
  let activeSource=null,rows=[],busy=false;
  function close(){form.hidden=true;onOverlayChange();}
  function refresh(){
    const key=getSourceKey();if(key!==activeSource)form.hidden=true;activeSource=key;rows=[];
    try{if(key)rows=store.list(key);}catch(error){onError(error);}
    list.replaceChildren();
    rows.forEach((row,index)=>{
      const card=document.createElement('article');card.className='named-view-card';
      const apply=document.createElement('button');apply.type='button';apply.className='named-view-preview';apply.dataset.viewIndex=index;apply.dataset.viewAction='apply';apply.title='应用视图';apply.setAttribute('aria-label',`应用视图 ${row.name}`);
      if(row.thumbnail){const img=document.createElement('img');img.src=row.thumbnail;img.alt='';apply.append(img);}
      else{const icon=document.createElement('span');icon.className='named-view-placeholder';icon.textContent='◇';apply.append(icon);}
      const name=document.createElement('span');name.textContent=row.name;name.className='named-view-name';apply.append(name);card.append(apply);
      const actions=document.createElement('div');actions.className='named-view-actions';
      const replace=document.createElement('button');replace.type='button';replace.className='named-view-replace';replace.dataset.viewAction='replace';replace.dataset.viewIndex=index;replace.textContent='用当前视图替换';actions.append(replace);
      const remove=document.createElement('div');remove.innerHTML=button('remove','删除视图');remove.firstChild.dataset.viewIndex=index;actions.append(remove.firstChild);card.append(actions);list.append(card);
    });
    panel.querySelectorAll('button,input').forEach(control=>{control.disabled=busy||isLocked()||!key;});onOverlayChange();
  }
  async function save(){
    if(busy||isLocked())return;
    const key=getSourceKey(),name=input.value,view=getView();if(key!==activeSource)return;
    busy=true;refresh();
    try{
      const thumbnail=await captureThumbnail(view);
      if(getSourceKey()!==key)throw new Error('地形已切换');
      store.save(key,name,view,thumbnail);form.hidden=true;
    }catch(error){onError(error);}finally{busy=false;refresh();}
  }
  async function replace(row){
    if(busy||isLocked())return;
    const key=getSourceKey();if(key!==activeSource){refresh();return;}
    busy=true;refresh();
    try{
      const view=validateNamedView(getView()),thumbnail=await captureThumbnail(view);
      if(getSourceKey()!==key)throw new Error('地形已切换');
      store.save(key,row.name,view,thumbnail);
    }catch(error){onError(error);}finally{busy=false;refresh();}
  }
  panel.addEventListener('click',event=>{
    const target=event.target.closest('[data-view-action]');if(!target||busy||isLocked())return;
    const action=target.dataset.viewAction;
    if(action==='save'){refresh();if(!activeSource)return;input.value='';form.hidden=false;input.focus();onOverlayChange();}
    if(action==='close')close();
    if(action==='confirm')void save();
    if(action==='apply'||action==='remove'||action==='replace'){
      if(getSourceKey()!==activeSource){refresh();return;}
      const row=rows[Number(target.dataset.viewIndex)];if(!row)return;
      try{if(action==='apply')applyView(clone(row.view));else if(action==='replace')void replace(row);else{store.remove(activeSource,row.name);refresh();}}catch(error){onError(error);}
    }
  });
  form.addEventListener('submit',event=>{event.preventDefault();void save();});
  panel.addEventListener('keydown',event=>{if(event.key==='Escape'){event.preventDefault();event.stopPropagation();close();}});
  refresh();return {close,refresh,getSpeed:()=>Number(speed.value),list(){const key=getSourceKey();return key?store.list(key):[];}};
}
