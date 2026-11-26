import {fields,normalizeParameter} from './state.js';
import {validateTransitionHeight} from './height-scaling.js';
import {captureTransitionBackground} from './background-transition.js';

import {captureTransitionCloud} from './cloud-transition.js';

export const controlTransitionKeys=['_transitionHeight','transitionBackgroundFrom','transitionBackgroundMix','transitionSkyFollow','transitionSkyModel','transitionCloudSeeds'];
export function controlTransitions(value={}){
  if(!value||typeof value!=='object'||Array.isArray(value)||Object.keys(value).some(key=>!controlTransitionKeys.includes(key)))throw new Error('过渡快照无效');
  return {...(value._transitionHeight!=null?{_transitionHeight:validateTransitionHeight(value._transitionHeight)}:{}),...captureTransitionBackground(value),...captureTransitionCloud(value)};
}
export function mergeControlParameters(current,patch,transition){
  const next=structuredClone({...current,...patch}),keys=Object.keys(patch);
  if(keys.some(key=>['heightScale','heightScaleMode'].includes(key)))delete next._transitionHeight;
  if(keys.some(key=>key.startsWith('background')||key.startsWith('sky')))for(const key of ['transitionBackgroundFrom','transitionBackgroundMix','transitionSkyFollow','transitionSkyModel'])delete next[key];
  if(keys.includes('cloudSeed'))delete next.transitionCloudSeeds;
  if(transition!==undefined){for(const key of controlTransitionKeys)delete next[key];Object.assign(next,controlTransitions(transition));}
  return next;
}

export function validateControlPatch(value,allowed=Object.keys(fields)){
  if(!value||typeof value!=='object'||Array.isArray(value))throw new Error('parameters必须为对象');
  const keys=new Set(allowed),next={};
  for(const [key,input] of Object.entries(value)){
    if(key==='exportOrthographic'&&keys.has(key)){if(typeof input!=='boolean')throw new Error('exportOrthographic必须为布尔值');next[key]=input;continue;}
    if(!keys.has(key)||!Object.hasOwn(fields,key))throw new Error(`未知参数：${key}`);
    next[key]=normalizeParameter(key,input);
  }
  return next;
}

export function createControlJobs({now=()=>new Date().toISOString(),onChange=()=>{}}={}){
  const jobs=new Map();let sequence=0,active=null;
  function read(id){const job=jobs.get(id);if(!job)throw new Error('任务不存在');return structuredClone(job);}
  return {
    get active(){return active;},
    read,
    start(kind,run,cancel){
      if(active)throw new Error(`任务进行中：${active}`);
      const id=`mcp-${Date.now().toString(36)}-${++sequence}`,job={jobId:id,kind,state:'running',startedAt:now(),progress:0};
      jobs.set(id,job);active=id;onChange(true,kind);
      Promise.resolve().then(run).then(result=>{job.state='succeeded';job.result=result;job.progress=100;},error=>{job.state=/已取消/.test(String(error))?'cancelled':'failed';job.error=String(error?.message??error);}).finally(()=>{job.finishedAt=now();active=null;job.cancel=null;onChange(false,kind,read(id));});
      Object.defineProperty(job,'cancel',{value:cancel,writable:true,enumerable:false});
      return read(id);
    },
    async cancel(id){const job=jobs.get(id);if(!job)throw new Error('任务不存在');if(job.state!=='running')return read(id);if(!job.cancel)throw new Error('此任务不支持取消');await job.cancel();job.cancelRequested=true;return read(id);},
    progress(id,value){const job=jobs.get(id);if(job?.state==='running'&&Number.isFinite(value))job.progress=Math.max(0,Math.min(99.9,value));},
  };
}

export async function attachRuntimeControl({listen,invoke,handlers,control}){
  let queue=Promise.resolve();
  const concurrent=new Set(['get_scene','get_export_settings','list_views','list_presets','get_video','job_status','cancel_job','end_control','get_control_state']);
  return listen('mcp-control-request',event=>{
    const {id,action,payload={},expiresAt}=event.payload??{};
    const execute=async()=>{
      try{
        if(Number.isFinite(expiresAt)&&Date.now()>expiresAt)throw new Error('请求已过期，未执行');
        if(!Object.hasOwn(handlers,action))throw new Error('不支持的控制操作');
        if(!payload||typeof payload!=='object'||Array.isArray(payload))throw new Error('payload必须为对象');
        if(!['begin_control','end_control','get_control_state'].includes(action))control?.touch();
        const result=await handlers[action](payload);
        await invoke('mcp_control_reply',{id,result:result??null,error:null});
      }catch(error){control?.end('request-error');await invoke('mcp_control_reply',{id,result:null,error:String(error?.message??error)}).catch(()=>{});}
    };
    if(concurrent.has(action))void execute();else queue=queue.then(execute,execute);
  });
}
