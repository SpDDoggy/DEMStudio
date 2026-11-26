import { fields, createSceneState, normalizeParameter, rampStops } from './state.js';
import { createExportSettings, cloneScene } from './export-settings.js';

import {captureTransitionCloud} from './cloud-transition.js';

export const presetStorageKey = 'demstudio.render-presets.v1';
const version = 1;
const excluded = new Set(['exportNormal','exportDepth','exportShadowLayer','appearanceLayersVersion','appearancePick','exportTransparent','experimentalBounces','width','height','samples','exposure','gamma','outputContrast','outputSaturation','outputTemperature','outputShadows','outputHighlights','exportShadow','exportGeoreference','shadowPadding','outputBitDepth','exportTerrainScale']);
const sceneKeys = Object.keys(fields).filter(key => !excluded.has(key));
function validateObject(value, keys) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('预设无效');
  return Object.fromEntries(keys.filter(key => Object.hasOwn(value,key)).map(key => [key, normalizeParameter(key,value[key])]));
}
export function defaultSnapshotSettings() {
  const parameters = createSceneState().parameters;
  return {parameters, output:createExportSettings(parameters), format:'png', framing:'view', scale:'1'};
}
export function validateSnapshotSettings(value) {
  if (!value || !['png','jpeg','bmp','tga','geotiff'].includes(value.format) || !['view','source'].includes(value.framing) || !['1','2','3','custom'].includes(value.scale)) throw new Error('预设无效');
  const defaults = defaultSnapshotSettings();
  if (value.parameters?.heightScaleMode === 'accelerated') {
    const oldScale = normalizeParameter('heightScale', value.parameters.heightScale ?? 1);
    value = {...value, parameters: {...value.parameters, heightScaleMode: 'linear', heightScale: normalizeParameter('heightScale', Math.min(500, Math.max(.1, oldScale * oldScale)))}};
  }
  const parameters = {...Object.fromEntries(sceneKeys.map(key=>[key,cloneScene(defaults.parameters[key])])), ...validateObject(value.parameters,sceneKeys)};
  for (const [key,mode] of [['materialEnabled','material'],['elevationEnabled','elevation'],['textureEnabled','texture']]) {
    if (!Object.hasOwn(value.parameters,key)) parameters[key] = key === 'materialEnabled' || parameters.appearanceMode === mode;
  }
  if (!Object.hasOwn(value.parameters, 'groundEnabled')) parameters.groundEnabled = parameters.grid !== false;
  if (!Object.hasOwn(value.parameters, 'rampStops')) parameters.rampStops = rampStops({...parameters,rampStops:null});
  if (parameters.fogEnd <= parameters.fogStart) throw new Error('雾结束距离必须大于起始距离');
  if (parameters.rampRangeMode === 'custom' && parameters.rampMax <= parameters.rampMin) throw new Error('高程范围无效');
  if (value.parameters.appearanceLayersVersion !== 1 && parameters.appearanceMode !== 'material') parameters.materialSpecular = 0;
  parameters.appearanceLayersVersion = 1;
  parameters.contourEnabled = false;
  // Old presets stored the native multiplier; migrate once using mode presence.
  if (!Object.hasOwn(value.parameters, 'heightScaleMode')) {
    parameters.heightScale = normalizeParameter('heightScale', (value.parameters.heightScale ?? 1) * 5);
    parameters.heightScaleMode = 'linear';
  }
  // Presets saved before sky models existed retain their original illumination.
  if (!Object.hasOwn(value.parameters, 'skyModel')) parameters.skyModel = 'constant';
  if (!Object.hasOwn(value.parameters, 'skyIntensity')) parameters.skyIntensity = parameters.skyModel === 'atmosphere' ? 45 : 100;
  Object.assign(parameters,captureTransitionCloud(value.parameters));
  const output = {...defaults.output, ...validateObject(value.output,Object.keys(defaults.output).filter(key => key!=='exportOrthographic'))};
  if (typeof value.output?.exportOrthographic !== 'boolean') throw new Error('预设无效');
  output.exportOrthographic = value.framing==='source';
  if (!Object.hasOwn(value.output ?? {}, 'exportGeoreference')) output.exportGeoreference = value.format==='geotiff';
  return cloneScene({parameters, output, format:value.format, framing:value.framing, scale:value.scale});
}
export function applySnapshotSettings(parameters, output, settings, textureAvailable=false) {
  const validated=validateSnapshotSettings(settings);
  validated.parameters.maxBounces = Math.min(validated.parameters.maxBounces, parameters.experimentalBounces ? 100 : 15);
  if (!textureAvailable) {
    validated.parameters.textureEnabled = false;
    if (validated.parameters.appearanceMode === 'texture') validated.parameters.appearanceMode = 'material';
  }
  delete parameters.transitionCloudSeeds;
  Object.assign(parameters,validated.parameters);
  parameters.appearancePick = false;
  Object.assign(output,validated.output);
  return validated;
}
// Named scene presets are styles, while source snapshots retain the full view/output contract.
export const presetParameterKeys = Object.freeze([
  'appearanceLayersVersion','materialEnabled','elevationEnabled','textureEnabled','appearanceMode',
  'materialColor','materialOpacity','materialRoughness','materialSpecular','materialSpecularColor',
  'textureMapping','textureFit','textureScale','textureOffsetX','textureOffsetY','textureRotation','textureBlend',
  'textureOpacity','textureExposure','textureContrast','textureShadows','textureHighlights','textureSaturation',
  'colorRamp','rampInterpolation','rampColors','rampStops','rampOffset','rampReverse','rampWrap','rampRangeMode','rampMin','rampMax','elevationBlend','elevationOpacity',
  'stretchType','stretchDeviations','stretchLow','stretchHigh',
  'heightScaleMode','heightScale','heightOffset','heightStep','lod','surface',
  'sunAltitude','sunAzimuth','sunSize','sunIntensity','sunColor','skyModel','skyFollowLight','skyIntensity','skyColor','maxBounces',
  'fogEnabled','fogColor','fogDensity','fogStart','fogEnd','fogHeightEnabled','fogHeightBase','fogHeightFalloff',
  'cloudEnabled','cloudCoverage','cloudDensity','cloudHeight','cloudThickness','cloudRange','cloudScale','cloudOffsetX','cloudOffsetY','cloudSeed','cloudColor',
  'cloudShadowEnabled','cloudShadowStrength','waterEnabled','waterLevel','waterOpacity','waterColor','waterRoughness',
  'sceneExposure','sceneGamma','sceneBrightness','sceneContrast','sceneSaturation','sceneTemperature','sceneShadows','sceneHighlights','bloom',
  'backgroundMode','backgroundColor','backgroundColor2','backgroundFollowLight','backgroundAngle','groundEnabled','groundColor','groundShadow','grid',
]);
const pickPresetParameters = parameters => Object.fromEntries(presetParameterKeys.map(key=>[key,cloneScene(parameters[key])]));
export function defaultPresetSettings() {
  return {parameters:pickPresetParameters(createSceneState().parameters)};
}
export function capturePresetSettings(parameters) {
  return validatePresetSettings({parameters});
}
export function validatePresetSettings(value) {
  if (!value || !value.parameters || typeof value.parameters !== 'object' || Array.isArray(value.parameters)) throw new Error('预设无效');
  // Validate only the explicit style fields. Old view/output/asset fields are ignored, never imported.
  const original=value.parameters;
  let supplied=validateObject(Object.fromEntries(Object.entries(original).filter(([key])=>presetParameterKeys.includes(key) && key!=='heightScaleMode')),presetParameterKeys);
  if (original.heightScaleMode === 'accelerated') {
    supplied.heightScaleMode='linear';
    supplied.heightScale=normalizeParameter('heightScale',Math.min(500,Math.max(.1,(supplied.heightScale??1)**2)));
  } else if (Object.hasOwn(original,'heightScaleMode')) supplied.heightScaleMode=normalizeParameter('heightScaleMode',original.heightScaleMode);
  else { supplied.heightScale=normalizeParameter('heightScale',(original.heightScale??1)*5);supplied.heightScaleMode='linear'; }
  const parameters={...defaultPresetSettings().parameters,...supplied};
  for (const [key,mode] of [['materialEnabled','material'],['elevationEnabled','elevation'],['textureEnabled','texture']]) {
    if (!Object.hasOwn(original,key)) parameters[key]=key==='materialEnabled'||parameters.appearanceMode===mode;
  }
  if (!Object.hasOwn(original,'groundEnabled')) parameters.groundEnabled=parameters.grid!==false;
  if (!Object.hasOwn(original,'rampStops')) parameters.rampStops=rampStops({...parameters,rampStops:null});
  if (original.appearanceLayersVersion!==1 && parameters.appearanceMode!=='material') parameters.materialSpecular=0;
  parameters.appearanceLayersVersion=1;
  if (!Object.hasOwn(original,'skyModel')) parameters.skyModel='constant';
  if (!Object.hasOwn(original,'skyIntensity')) parameters.skyIntensity=parameters.skyModel==='atmosphere'?45:100;
  if (parameters.fogEnd<=parameters.fogStart) throw new Error('雾结束距离必须大于起始距离');
  if (parameters.rampRangeMode==='custom' && parameters.rampMax<=parameters.rampMin) throw new Error('高程范围无效');
  return cloneScene({parameters});
}
export function applyPresetSettings(parameters, output, settings, textureAvailable=false) {
  const validated=validatePresetSettings(settings);
  validated.parameters.maxBounces=Math.min(validated.parameters.maxBounces,parameters.experimentalBounces?100:15);
  if (!textureAvailable) {
    validated.parameters.textureEnabled=false;
    if (validated.parameters.appearanceMode==='texture') validated.parameters.appearanceMode='material';
  }
  delete parameters.transitionCloudSeeds;
  Object.assign(parameters,validated.parameters);
  parameters.appearancePick=false;
  return validated;
}
export function createPresetStore(storage, clock=Date.now, builtins=[]) {
  const defaults=builtins.map(entry=>({...entry,settings:validatePresetSettings(entry.settings)}));
  let entries=[];
  try {
    const saved=JSON.parse(storage.getItem(presetStorageKey));
    if(saved?.version===version && Array.isArray(saved.entries)) entries=saved.entries.slice(0,100).flatMap(entry=>{
      try {
        if(typeof entry.name!=='string' || !entry.name.trim() || entry.name.length>40 || !Number.isFinite(entry.usedAt)) return [];
        return [{name:entry.name.trim(), usedAt:entry.usedAt, settings:validatePresetSettings(entry.settings)}];
      } catch {return [];}
    });
  } catch {}
  function persist(next) {storage.setItem(presetStorageKey,JSON.stringify({version,entries:next}));entries=next;}
  return {
    list(){return cloneScene([...entries,...defaults.filter(base=>!entries.some(entry=>entry.name===base.name))].sort((a,b)=>b.usedAt-a.usedAt));},
    isBuiltin(name){return defaults.some(entry=>entry.name===name) && !entries.some(entry=>entry.name===name);},
    save(name,settings){
      name=typeof name==='string'?name.trim():'';
      if(!name || name.length>40) throw new Error('请输入预设名称');
      const entry={name,usedAt:clock(),settings:validatePresetSettings(settings)};
      persist([entry,...entries.filter(item=>item.name!==name)].slice(0,100));
      return cloneScene(entry);
    },
    use(name){const entry=entries.find(item=>item.name===name) ?? defaults.find(item=>item.name===name);if(!entry) return null;
      if(entries.includes(entry)) persist([{...entry,usedAt:clock()},...entries.filter(item=>item!==entry)]);return cloneScene(entry.settings);},
    remove(name){persist(entries.filter(item=>item.name!==name));},
  };
}
