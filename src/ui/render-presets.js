import { fields, createSceneState, normalizeParameter } from './state.js';
import { createExportSettings, cloneScene } from './export-settings.js';

export const presetStorageKey = 'demstudio.render-presets.v1';
const version = 1;
const excluded = new Set(['experimentalBounces','width','height','samples','exposure','gamma','outputContrast','outputSaturation','outputTemperature','outputShadows','outputHighlights','exportShadow','shadowPadding','outputBitDepth','exportTerrainScale']);
const sceneKeys = Object.keys(fields).filter(key => !excluded.has(key));
function validateObject(value, keys) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('预设无效');
  return Object.fromEntries(keys.filter(key => Object.hasOwn(value,key)).map(key => [key, normalizeParameter(key,value[key])]));
}
export function defaultPresetSettings() {
  const parameters = createSceneState().parameters;
  return {parameters, output:createExportSettings(parameters), format:'png', framing:'view', scale:'1'};
}
export function validatePresetSettings(value) {
  if (!value || !['png','geotiff'].includes(value.format) || !['view','source'].includes(value.framing) || !['1','2','3','custom'].includes(value.scale)) throw new Error('预设无效');
  const defaults = defaultPresetSettings();
  const parameters = {...Object.fromEntries(sceneKeys.map(key=>[key,cloneScene(defaults.parameters[key])])), ...validateObject(value.parameters,sceneKeys)};
  parameters.contourEnabled = false;
  // Old presets stored the native multiplier; migrate once using mode presence.
  if (!Object.hasOwn(value.parameters, 'heightScaleMode')) {
    parameters.heightScale = normalizeParameter('heightScale', (value.parameters.heightScale ?? 1) * 5);
    parameters.heightScaleMode = 'linear';
  }
  // Presets saved before sky models existed retain their original illumination.
  if (!Object.hasOwn(value.parameters, 'skyModel')) parameters.skyModel = 'constant';
  if (!Object.hasOwn(value.parameters, 'skyIntensity')) parameters.skyIntensity = parameters.skyModel === 'atmosphere' ? 45 : 100;
  const output = {...defaults.output, ...validateObject(value.output,Object.keys(defaults.output).filter(key => key!=='exportOrthographic'))};
  if (typeof value.output?.exportOrthographic !== 'boolean') throw new Error('预设无效');
  output.exportOrthographic = value.format==='geotiff' || value.framing==='source';
  return cloneScene({parameters, output, format:value.format, framing:value.framing, scale:value.scale});
}
export function applyPresetSettings(parameters, output, settings, textureAvailable=false) {
  const validated=validatePresetSettings(settings);
  validated.parameters.maxBounces = Math.min(validated.parameters.maxBounces, parameters.experimentalBounces ? 100 : 15);
  if(validated.parameters.appearanceMode==='texture' && !textureAvailable) validated.parameters.appearanceMode='material';
  Object.assign(parameters,validated.parameters);
  Object.assign(output,validated.output);
  return validated;
}
export function createPresetStore(storage, clock=Date.now) {
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
    list(){return cloneScene([...entries].sort((a,b)=>b.usedAt-a.usedAt));},
    save(name,settings){
      name=typeof name==='string'?name.trim():'';
      if(!name || name.length>40) throw new Error('请输入预设名称');
      const entry={name,usedAt:clock(),settings:validatePresetSettings(settings)};
      persist([entry,...entries.filter(item=>item.name!==name)].slice(0,100));
      return cloneScene(entry);
    },
    use(name){const entry=entries.find(item=>item.name===name);if(!entry) return null;
      persist([{...entry,usedAt:clock()},...entries.filter(item=>item!==entry)]);return cloneScene(entry.settings);},
    remove(name){persist(entries.filter(item=>item.name!==name));},
  };
}
