import {validateNamedView,viewParameterKeys} from './named-views.js';
import {captureSceneSettings,sceneParameterKeys} from './scene-transition.js';
import {validateControlPatch,mergeControlParameters} from './mcp-control.js';
import {normalizeVideoDocument} from './video-workflow.js';

function viewName(value){
  if(typeof value!=='string'||!value.trim()||value.trim().length>80)throw new Error('视图名称无效');
  return value.trim();
}

// Build all records before the caller writes any of them. Every view starts
// from the same current view, rather than inheriting the preceding patch.
export function designControlViews(baseCurrentView,views){
  if(!Array.isArray(views)||views.length<1||views.length>200)throw new Error('视图批次必须为 1 至 200 项');
  const base=validateNamedView(baseCurrentView);
  const parameters={...base.settings?.parameters,...base.parameters},names=new Set();
  return views.map(row=>{
    const name=viewName(row?.name);
    if(names.has(name))throw new Error('视图名称重复');
    names.add(name);
    const patch=validateControlPatch(row.parameters===undefined?{}:row.parameters,[...viewParameterKeys,...sceneParameterKeys]);
    const next=mergeControlParameters(parameters,patch);
    const frame=Object.hasOwn(row,'frame')?row.frame:base.frame;
    if(frame===undefined)throw new Error('取景范围无效');
    return {name,view:validateNamedView({parameters:next,frame,settings:captureSceneSettings(next)})};
  });
}

export function sequenceControlViews(namedViews,frames,settings){
  if(!Array.isArray(namedViews))throw new Error('视图记录无效');
  if(!Array.isArray(frames)||frames.length<2||frames.length>200)throw new Error('视图帧必须为 2 至 200 项');
  const available=new Map(namedViews.map(row=>[row.name,row.view]));
  let previous=0;
  const timeline=frames.map((row,index)=>{
    const name=viewName(row?.name),time=row.time;
    if(!Number.isFinite(time)||time<0||time>600||(index===0?time!==0:time-previous<.1-1e-10))throw new Error('视图帧时间无效');
    if(!available.has(name))throw new Error(`视图不存在：${name}`);
    // Decimal timestamps such as .3 - .2 must retain the valid .1 interval.
    const duration=index?Number((time-previous).toFixed(10)):0;
    previous=time;
    return {name,view:available.get(name),duration};
  });
  return normalizeVideoDocument({settings:{...settings,mode:'views'},timeline});
}
