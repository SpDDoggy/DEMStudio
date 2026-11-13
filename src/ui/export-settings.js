export const stretchKeys = ['stretchType', 'stretchDeviations', 'stretchLow', 'stretchHigh'];

export const exportKeys = new Set(['width', 'height', 'samples', 'outputBitDepth', 'exportTerrainScale', 'exposure', 'gamma',
  'exportOrthographic', 'exportGeoreference', 'outputContrast', 'outputSaturation', 'outputTemperature', 'outputShadows', 'outputHighlights', 'exportShadow', 'shadowPadding']);

export function createExportSettings(parameters) {
  return Object.fromEntries([...exportKeys, ...stretchKeys].map(key => [key, key === 'exportOrthographic' ? false : parameters[key]]));
}

export function exportSnapshot(parameters, settings) {
  return cloneScene({ ...parameters, ...settings, grid: false, contourEnabled: false,
    exposure: 1, gamma: 2.2, outputContrast: 0, outputSaturation: 0,
    outputTemperature: 0, outputShadows: 0, outputHighlights: 0 });
}

// Source framing and geographic metadata are independent of file encoding.
export function exportRequest(parameters, settings) {
  const params = exportSnapshot(parameters, settings);
  params.exportShadow = params.exportOrthographic && params.exportShadow;
  params.exportGeoreference = params.exportOrthographic && params.exportGeoreference;
  return { orthographic: params.exportOrthographic, params };
}

// Match native georef::valid_wkt without resolving or changing the source CRS.
function validSourceWkt(value) {
  if (typeof value !== 'string') return false;
  value=value.trim();
  const open=value.indexOf('[');
  if (open<0 || !['PROJCS','GEOGCS','GEOCCS','LOCAL_CS','COMPD_CS','VERT_CS','PROJCRS','GEOGCRS','GEODCRS','VERTCRS','ENGCRS','COMPOUNDCRS','BOUNDCRS','DERIVEDPROJCRS'].includes(value.slice(0,open).trim().toUpperCase())) return false;
  const root=value.slice(0,open).trim().toUpperCase();
  const body=value.slice(open+1).trimStart();
  if (root==='BOUNDCRS' ? !/^SOURCECRS\s*\[/iu.test(body) : !body.startsWith('"')) return false;
  let depth=0, quoted=false;
  for (let i=open;i<value.length;i++) {
    const c=value[i], code=c.charCodeAt(0);
    if ((code<32 || code>=127 && code<=159) && !/\s/u.test(c)) return false;
    if (c==='"') {
      if (quoted && value[i+1]==='"') i++;
      else quoted=!quoted;
    } else if (!quoted) {
      if (c==='[') depth++;
      if (c===']' && --depth===0) return !value.slice(i+1).trim();
      if (depth<0) return false;
    }
  }
  return false;
}

export function sourceHasCrs(status) {
  const geo = status?.source?.geo;
  if (!geo) return false;
  if (validSourceWkt(geo.wkt)) return true;
  const keys = geo.keys;
  if (!Array.isArray(keys) || keys.length < 4 || keys[0] !== 1) return false;
  const count = keys[3];
  if (!Number.isInteger(count) || keys.length < 4 + count * 4) return false;
  const end = 4 + count * 4;
  for (let i = 4; i < end; i += 4) {
    const location=keys[i+1], length=keys[i+2], offset=keys[i+3];
    if (!(length > 0) || !(location===0 ? length===1 : location===34735 ? offset>=end && offset+length<=keys.length : location===34736 ? offset+length<=(geo.doubles?.length ?? 0) && geo.doubles.slice(offset,offset+length).every(Number.isFinite) : location===34737 ? offset+length<=(geo.ascii?.length ?? 0)+1 : false)) return false;
  }
  for (let i = 4; i < 4 + count * 4; i += 4) {
    if ([2048,3072].includes(keys[i]) && keys[i+1] === 0 && keys[i+2] === 1 && keys[i+3] > 0 && keys[i+3] !== 32767) return true;
  }
  return false;
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
