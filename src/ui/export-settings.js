export const stretchKeys = ['stretchType', 'stretchDeviations', 'stretchLow', 'stretchHigh'];

export const exportKeys = new Set(['width', 'height', 'samples', 'exposure', 'gamma',
  'exportOrthographic', 'outputContrast', 'outputSaturation', 'outputTemperature', 'outputShadows', 'outputHighlights', 'exportShadow', 'shadowPadding']);

export function createExportSettings(parameters) {
  return Object.fromEntries([...exportKeys, ...stretchKeys].map(key => [key, key === 'exportOrthographic' ? false : parameters[key]]));
}

export function exportSnapshot(parameters, settings) {
  return cloneScene({ ...parameters, ...settings, grid: false,
    exposure: 1, gamma: 2.2, outputContrast: 0, outputSaturation: 0,
    outputTemperature: 0, outputShadows: 0, outputHighlights: 0 });
}

export function previewScale(value) {
  return Math.max(.25, Math.min(4, value));
}

export function sourceOutputSize(status, multiplier = 1) {
  if (![1, 2, 3].includes(multiplier)) throw new Error('倍率无效');
  const width = status?.source?.width ?? status?.mapWidth;
  const height = status?.source?.height ?? status?.mapHeight;
  if (!Number.isInteger(width) || !Number.isInteger(height) || width < 1 || height < 1) return null;
  return {width: width * multiplier, height: height * multiplier};
}

export function cloneScene(value) {
  if(Array.isArray(value)) return value.map(cloneScene);
  if(value && typeof value==='object') return Object.fromEntries(Object.entries(value).map(([key,item])=>[key,cloneScene(item)]));
  return value;
}

// Preserve the camera frustum after native cover presentation crops the output.
export function freezeCurrentView(parameters, viewport) {
  const snapshot=cloneScene(parameters);
  const {width,height}=viewport;
  if (!(width>0 && height>0 && parameters.width>0 && parameters.height>0)) throw new Error('视口尺寸无效');
  const coverScale=Math.max(width/parameters.width,height/parameters.height);
  const k=Math.min(width,height)/(Math.min(parameters.width,parameters.height)*coverScale);
  snapshot.fieldOfView=2*Math.atan(Math.tan(parameters.fieldOfView*Math.PI/360)*k)*180/Math.PI;
  return snapshot;
}

export function viewOutputSize(status, viewport, multiplier=1) {
  const source=sourceOutputSize(status);
  if (![1,2,3].includes(multiplier)) throw new Error('倍率无效');
  if (!source || !(viewport?.width>0 && viewport?.height>0)) return null;
  const scale=Math.max(source.width,source.height)/Math.max(viewport.width,viewport.height);
  return {width:Math.max(1,Math.round(viewport.width*scale))*multiplier,height:Math.max(1,Math.round(viewport.height*scale))*multiplier};
}

export function proportionalSize(size, key, value) {
  if (!['width','height'].includes(key) || !(size.width>0 && size.height>0 && value>0)) throw new Error('尺寸无效');
  return key==='width'?{width:value,height:Math.max(1,Math.round(value*size.height/size.width))}:{height:value,width:Math.max(1,Math.round(value*size.width/size.height))};
}
