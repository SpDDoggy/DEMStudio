import {fields,rampStops,sampleRampColor} from './state.js';
import {presetParameterKeys,validatePresetSettings,capturePresetSettings} from './render-presets.js';
import {interpolateCamera,cameraTransitionDuration} from './camera-transition.js';
import {nativeHeightScaling,validateTransitionHeight} from './height-scaling.js';
import {transitionAppearance,applyLayerTransition} from './layer-transition.js';
import {applyBackgroundTransition,captureTransitionBackground} from './background-transition.js';

import {captureTransitionCloud,applyCloudTransition} from './cloud-transition.js';

const copy=value=>JSON.parse(JSON.stringify(value));
export const sceneParameterKeys=presetParameterKeys;
export function captureSceneSettings(parameters){
  const settings=capturePresetSettings(parameters);
  if(parameters._transitionHeight!=null)settings.parameters._transitionHeight=validateTransitionHeight(parameters._transitionHeight);
  Object.assign(settings.parameters,captureTransitionBackground(parameters),captureTransitionCloud(parameters));
  return settings;
}
export function validateSceneSettings(settings){
  const validated=validatePresetSettings(settings);
  if(settings.parameters?._transitionHeight!=null)validated.parameters._transitionHeight=validateTransitionHeight(settings.parameters._transitionHeight);
  Object.assign(validated.parameters,captureTransitionBackground(settings.parameters),captureTransitionCloud(settings.parameters));
  return validated;
}
const mix=(a,b,t)=>a+(b-a)*t;
function color(a,b,t){return '#'+[1,3,5].map(i=>Math.round(mix(parseInt(a.slice(i,i+2),16),parseInt(b.slice(i,i+2),16),t)).toString(16).padStart(2,'0')).join('');}
const angle=(a,b,t)=>a+((((b-a+180)%360)+360)%360-180)*t;

// Intermediates remain floating point: field step sizes belong to manual input,
// not animation sampling. Discrete renderer modes switch at the midpoint.
export function interpolateSceneParameters(from,to,progress,{easing='smooth'}={}){
  if(progress<=0)return copy(from);
  if(progress>=1)return copy(to);
  from=transitionAppearance(from);to=transitionAppearance(to);
  const t=easing==='linear'?progress:progress*progress*(3-2*progress);
  const result={...copy(from)};
  for(const key of sceneParameterKeys){
    const a=from[key],b=to[key];if(a===undefined||b===undefined)continue;
    if(key==='cloudSeed')result[key]=t<.5?a:b;
    else if(fields[key]?.type==='color')result[key]=color(a,b,t);
    else if(typeof a==='number'&&typeof b==='number'){
      result[key]=['sunAzimuth','backgroundAngle','textureRotation'].includes(key)?((angle(a,b,t)%360)+360)%360:mix(a,b,t);
      if(['maxBounces','lod','appearanceLayersVersion'].includes(key))result[key]=Math.round(result[key]);
    }
    else if(Array.isArray(a)&&Array.isArray(b)&&a.length===b.length&&a.every(Number.isFinite)&&b.every(Number.isFinite))result[key]=a.map((v,i)=>mix(v,b[i],t));
    else result[key]=copy(t<.5?a:b);
  }
  const heightFrom=nativeHeightScaling(from),heightTo=nativeHeightScaling(to);
  if(heightFrom.heightScale!==heightTo.heightScale || heightFrom.heightCurve!==heightTo.heightCurve || from._transitionHeight || to._transitionHeight){
    result._transitionHeight={heightScale:mix(heightFrom.heightScale,heightTo.heightScale,t),heightCurve:mix(heightFrom.heightCurve,heightTo.heightCurve,t)};
  }
  const rampChanged=['colorRamp','rampStops','rampColors','rampInterpolation','rampReverse'].some(key=>JSON.stringify(from[key])!==JSON.stringify(to[key]));
  if(from.colorRamp&&to.colorRamp&&rampChanged){
    const a=rampStops(from),b=rampStops(to);
    // A shared set of samples morphs ramps with different stop counts and
    // named palettes without an enum jump halfway through the transition.
    const count=Math.min(16,Math.max(a.length,b.length,5));
    result.colorRamp='custom';result.rampReverse=false;
    result.rampInterpolation='continuous';
    result.rampStops=Array.from({length:count},(_,i)=>{
      const position=i/(count-1);
      return {position,color:color(sampleRampColor(a,from.rampReverse?1-position:position,from.rampInterpolation),sampleRampColor(b,to.rampReverse?1-position:position,to.rampInterpolation),t)};
    });
    result.rampColors=Array.from({length:5},(_,i)=>sampleRampColor(result.rampStops,i/4));
  }
  if(from.projection&&to.projection){
    // interpolateCamera applies smoothstep internally; invert it by selecting
    // its new linear option so scene and camera share one timing curve.
    const camera=interpolateCamera(from,to,t,{easing:'linear'});
    for(const key of ['projection','cameraYaw','cameraPitch','cameraDistance','cameraPan','fieldOfView','cameraFrameX','cameraFrameY'])result[key]=camera[key];
  }
  applyBackgroundTransition(from,to,t,result);
  applyCloudTransition(from,to,t,result);
  for(const [enabled,strength,fallback] of [['waterEnabled','waterOpacity',80],['cloudShadowEnabled','cloudShadowStrength',50]]){
    if(from[enabled]!==to[enabled]){result[enabled]=true;result[strength]=mix(from[enabled]?(from[strength]??fallback):0,to[enabled]?(to[strength]??fallback):0,t);}
  }
  return applyLayerTransition(from,to,t,result);
}

export function sceneTransitionDuration(from,to){
  const different=key=>JSON.stringify(from[key])!==JSON.stringify(to[key]);
  const changed=sceneParameterKeys.filter(different);
  const cameraDuration=cameraTransitionDuration(from,to);
  const a=nativeHeightScaling(from),b=nativeHeightScaling(to);
  const scaleRatio=Math.max(a.heightScale,b.heightScale)/Math.max(.000001,Math.min(a.heightScale,b.heightScale));
  const curveRatio=Math.max(a.heightCurve,b.heightCurve)/Math.max(.000001,Math.min(a.heightCurve,b.heightCurve));
  const shapeChanged=scaleRatio>1+1e-10||curveRatio>1+1e-10||['heightOffset','heightStep','surface'].some(different);
  if(shapeChanged)return Math.max(cameraDuration,Math.min(3000,650+450*Math.log2(scaleRatio)+200*Math.log2(curveRatio)));
  const backdrop=key=>key.startsWith('background')||key.startsWith('sky')||key.startsWith('ground');
  const backgroundChanged=different('transitionBackgroundFrom')||different('transitionSkyFollow')||different('transitionSkyModel');
  const cloudChanged=different('transitionCloudSeeds');
  if(changed.length||backgroundChanged||cloudChanged)return Math.max(cameraDuration,!cloudChanged&&changed.every(backdrop)?360:480);
  return cameraDuration;
}

export function createSceneTransition({read,write,finish=()=>{},onCancel=()=>{},requestFrame=requestAnimationFrame,cancelFrame=cancelAnimationFrame,now=()=>performance.now(),duration=650,maxFrameDelta=Infinity,speed=()=>1}){
  let frame=null,generation=0;
  function cancel(){generation++;const active=frame!==null;if(active)cancelFrame(frame);frame=null;if(active)onCancel();}
  return {cancel,start(target){
    cancel();const token=generation,from=copy(read()),to=copy(target),durationMs=typeof duration==='function'?duration(from,to):duration;let previous=now(),elapsed=0;
    function step(time){if(token!==generation)return;elapsed+=Math.min(maxFrameDelta,Math.max(0,time-previous))*Math.max(.01,Number(speed())||1);previous=time;const progress=Math.min(1,Math.max(0,elapsed/durationMs));frame=null;write(interpolateSceneParameters(from,to,progress));if(token!==generation)return;if(progress===1)finish();else frame=requestFrame(step);}
    frame=requestFrame(step);
  }};
}

export function mergeSceneView(base,view){
  const result=copy(base);
  if(view.settings){for(const key of ['transitionCloudSeeds','_transitionHeight','transitionBackgroundFrom','transitionBackgroundMix','transitionSkyFollow','transitionSkyModel'])delete result[key];}
  return {...result,...view.settings?.parameters,...view.parameters};
}
