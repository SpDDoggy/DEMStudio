export const rampLibrary = {
  continuous: {
    terrain: { label: '森林·砂岩', colors: ['#364824', '#6d753b', '#9a9a5c', '#c39362', '#e1bb89'] },
    antique: { label: '古地图·暖灰', colors: ['#766f69', '#979088', '#b3afa4', '#cdc9be', '#e4e1d8'] },
    spectral: { label: '光谱', colors: ['#3340a6','#26a6c7','#a6d161','#fab838','#bf2933'] },
    elevation: { label: '海拔', colors: ['#edf8e9','#bae4b3','#74c476','#31a354','#006d2c'] },
    diverging: { label: '蓝白红', colors: ['#2166ac','#92c5de','#f7f7f7','#f4a582','#b2182b'] },
    grayscale: { label: '灰度', colors: ['#000000','#404040','#808080','#bfbfbf','#ffffff'] },
  },
  discrete: {
    terrain: { label: '森林·砂岩分级', colors: ['#364824', '#6d753b', '#9a9a5c', '#c39362', '#e1bb89'] },
    antique: { label: '古地图·暖灰分级', colors: ['#766f69', '#979088', '#b3afa4', '#cdc9be', '#e4e1d8'] },
    spectral: { label: '光谱分级', colors: ['#3340a6','#26a6c7','#a6d161','#fab838','#bf2933'] },
    elevation: { label: '海拔分级', colors: ['#edf8e9','#bae4b3','#74c476','#31a354','#006d2c'] },
    diverging: { label: '蓝白红分级', colors: ['#2166ac','#92c5de','#f7f7f7','#f4a582','#b2182b'] },
    grayscale: { label: '灰度分级', colors: ['#000000','#404040','#808080','#bfbfbf','#ffffff'] },
  },
};

export function rampPalette(parameters) {
  if (parameters.colorRamp === 'custom') return [...parameters.rampColors];
  return [...rampLibrary[parameters.rampInterpolation][parameters.colorRamp].colors];
}

export function rampStops(parameters) {
  if (parameters.colorRamp === 'custom' && parameters.rampStops?.length) {
    const stops = parameters.rampStops.map(stop => ({...stop}));
    if (parameters.rampInterpolation === 'discrete') {
      if (stops.at(-1).position === 1) for (const stop of stops) stop.position *= (stops.length-1)/stops.length;
      stops[0].position = 0;
    }
    return stops;
  }
  return rampPalette(parameters).map((color,index,array)=>({position:index/(parameters.rampInterpolation === 'discrete' ? array.length : array.length-1),color}));
}

// New display presets reuse the existing native custom-ramp protocol.
export function nativeRamp(parameters) {
  const legacy = parameters.rampInterpolation === 'continuous' && ['grayscale','spectral'].includes(parameters.colorRamp);
  return {colorRamp: legacy ? parameters.colorRamp : 'custom', rampColors: rampPalette(parameters), rampStops: parameters.colorRamp === 'custom' || parameters.rampInterpolation === 'discrete' ? rampStops(parameters) : []};
}


export function sampleRampColor(stops, t, mode = 'continuous') {
  if (!stops.length) throw new Error('色标为空');
  let index = 0;
  while (index + 1 < stops.length && t >= stops[index + 1].position) index++;
  const a = stops[index], b = stops[Math.min(index + 1, stops.length - 1)];
  if (mode === 'discrete' || t <= a.position || a === b) return a.color;
  const weight = Math.max(0, Math.min(1, (t - a.position) / (b.position - a.position)));
  return '#' + [1,3,5].map(offset => Math.round(parseInt(a.color.slice(offset,offset+2),16)*(1-weight)+parseInt(b.color.slice(offset,offset+2),16)*weight).toString(16).padStart(2,'0')).join('');
}
export function resampleRamp(stops, count, mode = 'continuous') {
  if (!Number.isInteger(count) || count < 2 || count > 16) throw new Error('分级数量无效');
  const first=stops[0]?.position??0,last=stops.at(-1)?.position??1;
  const anchors=stops.map(stop=>({...stop,position:last>first?(stop.position-first)/(last-first):0}));
  return Array.from({length: count}, (_, index) => {
    const position = index / (mode === 'discrete' ? count : count - 1);
    // Resample the underlying color progression, never repeat the old zero-width terminal class.
    return {position, color: sampleRampColor(anchors, index/(count-1), 'continuous')};
  });
}
export function rampGradient(stops, mode, reverse = false) {
  return `linear-gradient(to ${reverse ? 'left' : 'right'},${stops.map((stop,index)=> mode === 'discrete' ? `${stop.color} ${stop.position*100}% ${(stops[index+1]?.position ?? 1)*100}%` : `${stop.color} ${stop.position*100}%`).join(',')})`;
}

export function sampledRampPosition(parameters, sample) {
  const low = parameters.rampRangeMode === 'custom' ? parameters.rampMin : sample.range[0];
  const high = parameters.rampRangeMode === 'custom' ? parameters.rampMax : sample.range[1];
  if (!(high > low) || !Number.isFinite(sample.height)) return null;
  let coordinate = (sample.height - low) / (high - low);
  if (parameters.rampReverse) coordinate = 1 - coordinate;
  coordinate -= parameters.rampOffset;
  return parameters.rampWrap ? ((coordinate % 1) + 1) % 1 : Math.max(0, Math.min(1, coordinate));
}

export const fields = {
  appearanceLayersVersion: { value: 1, min: 1, max: 1, step: 1 },
  appearancePick: { value: false, type: 'boolean' },
  materialEnabled: { label: '启用材质', value: true, type: 'boolean' },
  elevationEnabled: { label: '启用赋色', value: false, type: 'boolean' },
  textureEnabled: { label: '启用贴图', value: false, type: 'boolean' },
  backgroundMode: { label: '背景', value: 'sky', options: { sky: '天空', solid: '纯色', gradient: '渐变' } },
  backgroundColor: { label: '颜色', value: '#30343b', type: 'color' },
  backgroundColor2: { label: '末端颜色', value: '#647180', type: 'color' },
  backgroundFollowLight: { label: '跟随光线方向', value: false, type: 'boolean' },
  backgroundAngle: { label: '方向', value: 90, min: 0, max: 360, step: 1 },
  groundEnabled: { label: '显示地面', value: true, type: 'boolean' },
  groundColor: { label: '颜色', value: '#767676', type: 'color' },
  groundShadow: { label: '承接阴影', value: true, type: 'boolean' },
  exportTransparent: { label: '透明背景', value: false, type: 'boolean' },
  exportNormal: { label: '法线图', value: false, type: 'boolean' },
  exportDepth: { label: '深度图', value: false, type: 'boolean' },
  exportShadowLayer: { label: '阴影图', value: false, type: 'boolean' },
  textureMapping: { label: '附着', value: 'aligned', options: { aligned: '原图对齐', planar: '平面投影', triplanar: '三向投影' } },
  textureFit: { label: '覆盖', value: 'fill', options: { fill: '铺满', contain: '保持比例', repeat: '重复平铺' } },
  textureScale: { label: '缩放', value: 1, min: .01, max: 100, step: .01 },
  textureOffsetX: { label: '水平偏移', value: 0, min: -100, max: 100, step: .01 },
  textureOffsetY: { label: '垂直偏移', value: 0, min: -100, max: 100, step: .01 },
  textureRotation: { label: '旋转', value: 0, min: -360, max: 360, step: 1 },
  textureBlend: { label: '混合', value: 'overlay', options: { overlay: '叠加', normal: '正常', multiply: '正片叠底', softlight: '柔光' } },
  materialRoughness: { label: '粗糙度', value: .5, min: .02, max: 1, step: .01 },
  materialSpecular: { label: '高光强度', value: 0, min: 0, max: 1, step: .01 },
  materialSpecularColor: { label: '高光颜色', value: '#ffffff', type: 'color' },
  rampStops: { value: ['#26523b','#6e9152','#b3a36e','#9c856e','#f0ede3'].map((color,index)=>({position:index/4,color})), type: 'stops' },
  rampOffset: { label: '偏移', value: 0, min: -1, max: 1, step: .01 },
  rampReverse: { label: '反转', value: false, type: 'boolean' },
  rampWrap: { label: '循环', value: false, type: 'boolean' },
  rampRangeMode: { label: '范围', value: 'source', options: { source: '全源高程', custom: '自定义' } },
  rampMin: { label: '最低高程', value: 0, min: -1e12, max: 1e12, step: .01, range: false },
  rampMax: { label: '最高高程', value: 1, min: -1e12, max: 1e12, step: .01, range: false },

  sunAltitude: { label: '高度角', value: 45, min: 0, max: 90, step: 1 },
  sunAzimuth: { label: '方位角', value: 135, min: 0, max: 360, step: 1 },
  sunSize: { label: '面积', value: 1, min: .1, max: 100, step: .1 },
  sunIntensity: { label: '太阳强度', value: 100, min: 0, max: 500, step: 1 },
  sunColor: { label: '颜色', value: '#fff1db', type: 'color' },
  skyModel: { label: '天空模型', value: 'atmosphere', options: { atmosphere: '大气天空', constant: '恒色天空' } },
  skyFollowLight: { label: '跟随光线方向', value: false, type: 'boolean' },
  skyIntensity: { label: '天光强度', value: 45, min: 0, max: 500, step: 1 },
  skyColor: { label: '颜色', value: '#a9c9e8', type: 'color' },
  maxBounces: { label: '最大反弹次数', value: 8, min: 1, max: 100, step: 1 },
  experimentalBounces: { value: false, type: 'boolean' },
  fogEnabled: { label: '启用雾', value: false, type: 'boolean' },
  fogColor: { label: '颜色', value: '#c8d6df', type: 'color' },
  fogDensity: { label: '密度', value: 20, min: 0, max: 100, step: .1 },
  fogStart: { label: '起始距离 (%)', value: 0, min: 0, max: 10000, step: 1 },
  fogEnd: { label: '结束距离 (%)', value: 10000, min: .1, max: 10000, step: 1 },
  fogHeightEnabled: { label: '高度雾', value: false, type: 'boolean' },
  fogHeightBase: { label: '雾层高度 (%)', value: 0, min: -100, max: 1000, step: .1 },
  fogHeightFalloff: { label: '雾层厚度 (%)', value: 20, min: .1, max: 1000, step: .1 },
  waterEnabled: { label: '启用水面', value: false, type: 'boolean' },
  waterLevel: { label: '水位 (%)', value: 20, min: 0, max: 100, step: .1 },
  waterOpacity: { label: '不透明度', value: 80, min: 0, max: 100, step: 1 },
  waterColor: { label: '颜色', value: '#4c9fa6', type: 'color' },
  waterRoughness: { label: '粗糙度', value: .2, min: .02, max: 1, step: .01 },
  cloudEnabled: { label: '启用云', value: false, type: 'boolean' },
  cloudShadowEnabled: { label: '云影', value: false, type: 'boolean' },
  cloudShadowStrength: { label: '阴影强度', value: 50, min: 0, max: 100, step: 1 },
  cloudCoverage: { label: '覆盖', value: 35, min: 0, max: 100, step: .1 },
  cloudDensity: { label: '浓度', value: 60, min: 0, max: 100, step: .1 },
  cloudHeight: { label: '高度 (%)', value: 15, min: -100, max: 1000, step: .1 },
  cloudThickness: { label: '厚度 (%)', value: 12, min: .1, max: 200, step: .1 },
  cloudRange: { label: '范围 (%)', value: 125, min: 10, max: 500, step: 1 },
  cloudScale: { label: '尺度 (%)', value: 25, min: 1, max: 200, step: .1 },
  cloudOffsetX: { label: '横向位置 (%)', value: 0, min: -200, max: 200, step: .1 },
  cloudOffsetY: { label: '纵向位置 (%)', value: 0, min: -200, max: 200, step: .1 },
  cloudSeed: { label: '种子', value: 7, min: 0, max: 9999, step: 1 },
  cloudColor: { label: '颜色', value: '#f3f1e8', type: 'color' },
  cameraYaw: { label: '方位', value: 45, min: -180, max: 180, step: 1 },
  cameraPitch: { label: '俯仰', value: 35, min: 0, max: 90, step: 1 },
  cameraDistance: { label: '距离', value: 10, min: .1, max: 1000, step: .1 },
  cameraFrameX: { value: 0, min: -10000, max: 10000, step: .000001 },
  cameraFrameY: { value: 0, min: -10000, max: 10000, step: .000001 },
  cameraPan: { value: [0, 0, 0], type: 'vector3' },
  fieldOfView: { label: '视角', value: 45, min: 10, max: 120, step: 1 },
  sceneExposure: { label: '曝光', value: 0, min: -4, max: 4, step: .1 },
  sceneGamma: { label: '伽马', value: 2.2, min: .1, max: 4, step: .01 },
  sceneBrightness: { label: '亮度', value: 0, min: -100, max: 100, step: 1 },
  sceneContrast: { label: '对比度', value: 0, min: -100, max: 100, step: 1 },
  sceneSaturation: { label: '饱和度', value: 0, min: -100, max: 100, step: 1 },
  sceneTemperature: { label: '色温', value: 0, min: -100, max: 100, step: 1 },
  sceneShadows: { label: '阴影', value: 0, min: -100, max: 100, step: 1 },
  sceneHighlights: { label: '高光', value: 0, min: -100, max: 100, step: 1 },
  textureExposure: { label: '曝光', value: 0, min: -4, max: 4, step: .1 },
  textureContrast: { label: '对比度', value: 0, min: -100, max: 100, step: 1 },
  textureShadows: { label: '阴影', value: 0, min: -100, max: 100, step: 1 },
  textureHighlights: { label: '高光', value: 0, min: -100, max: 100, step: 1 },
  textureSaturation: { label: '饱和度', value: 0, min: -100, max: 100, step: 1 },
  exposure: { label: '曝光', value: 1, min: 0, max: 5, step: .01 },
  gamma: { label: '伽马', value: 2.2, min: .1, max: 4, step: .01 },
  outputContrast: { label: '对比度', value: 0, min: -100, max: 100, step: 1 },
  outputSaturation: { label: '饱和度', value: 0, min: -100, max: 100, step: 1 },
  outputTemperature: { label: '色温', value: 0, min: -100, max: 100, step: 1 },
  outputShadows: { label: '阴影', value: 0, min: -100, max: 100, step: 1 },
  outputHighlights: { label: '高光', value: 0, min: -100, max: 100, step: 1 },
  bloom: { label: '泛光', value: 0, min: 0, max: 100, step: 1 },
  stretchType: { label: '类型', value: 'none', options: { none: '无', minmax: '最小最大', stddev: '标准差', equalize: '直方图均衡化', percentclip: '百分比截断' } },
  stretchDeviations: { label: '标准差倍数', value: 2, min: .1, max: 10, step: .1 },
  stretchLow: { label: '低端 (%)', value: 2, min: 0, max: 49, step: .1 },
  stretchHigh: { label: '高端 (%)', value: 2, min: 0, max: 49, step: .1 },
  elevationBlend: { label: '混合', value: 'color', options: { normal: '常规', color: '颜色' } },
  heightScaleMode: { label: '缩放模式', value: 'linear', options: { linear: '线性', nonlinear: '非线性' } },
  heightScale: { label: '缩放', value: 1, min: .1, max: 500, step: .1 },
  contourEnabled: { label: '显示', value: false, type: 'boolean' },
  contourInterval: { label: '间距', value: 5, min: .01, max: 1000000, step: .01, range: false },
  contourWidth: { label: '线宽', value: .65, min: .1, max: 30, step: .05 },
  contourSmoothing: { label: '平滑', value: 1, min: 0, max: 5, step: .1 },
  contourColor: { label: '颜色', value: '#6b6258', type: 'color' },
  contourOpacity: { label: '不透明度', value: 45, min: 0, max: 100, step: 1 },
  contourMajorEvery: { label: '主线间隔', value: 5, min: 1, max: 20, step: 1, range: false },
  contourMajorWidth: { label: '主线宽', value: 1.1, min: .1, max: 30, step: .05 },
  heightOffset: { label: '偏移', value: 0, min: 0, max: 100, step: 1 },
  heightStep: { label: '量化', value: 0, min: 0, max: 100000, step: .1, range: false },
  lod: { label: '细节层级', value: 0, min: 0, max: 14, step: 1 },
  surface: { label: '表面', value: 'bilinear', options: { cube: '方块', polygon: '多边形', bilinear: '双线性', bicubic: '双三次' } },
  appearanceMode: { value: 'material', options: { texture: '贴图', material: '材质', elevation: '赋色' } },
  colorRamp: { label: '色带', value: 'terrain', options: { terrain: '森林·砂岩', antique: '古地图·暖灰', spectral: '光谱', grayscale: '灰度', elevation: '海拔', diverging: '蓝白红', custom: '自定义' } },
  rampInterpolation: { label: '过渡', value: 'continuous', options: { continuous: '连续', discrete: '离散' } },
  rampColors: { label: '颜色', value: ['#26523b', '#6e9152', '#b3a36e', '#9c856e', '#f0ede3'], type: 'color5' },
  materialOpacity: { label: '不透明度', value: 100, min: 0, max: 100, step: 1 },
  textureOpacity: { label: '不透明度', value: 100, min: 0, max: 100, step: 1 },
  elevationOpacity: { label: '不透明度', value: 100, min: 0, max: 100, step: 1 },
  materialColor: { label: '颜色', value: '#d5d2c9', type: 'color' },
  width: { label: '宽度', value: 1920, min: 1, max: 243600, step: 1, range: false },
  height: { label: '高度', value: 1080, min: 1, max: 243600, step: 1, range: false },
  exportTerrainScale: { label: '地形精度', value: 'source', options: { source: '原始', half: '1/2', quarter: '1/4', preview: '预览' } },
  outputBitDepth: { label: '位深', value: 8, min: 8, max: 16, step: 8 },
  samples: { label: '采样', value: 256, min: 1, max: 65536, step: 1, range: false },
  projection: { value: 'perspective', options: { perspective: '透视', orthographic: '正交', isometric: '等轴' } },
  grid: { value: true, type: 'boolean' },
  exportGeoreference: { label: '导出地理信息', value: false, type: 'boolean' },
  exportShadow: { label: '范围', value: false, type: 'boolean' },
  shadowPadding: { label: '外扩 (%)', value: 20, min: 1, max: 200, step: 1, range: false },
};

export const sceneGradeKeys = ['sceneExposure', 'sceneGamma', 'sceneBrightness', 'sceneContrast', 'sceneTemperature', 'sceneShadows', 'sceneHighlights'];

export function resetSceneGrading(parameters) {
  for (const key of [...sceneGradeKeys, 'sceneSaturation']) parameters[key] = fields[key].value;
}

export function createSceneState() {
  return { parameters: Object.fromEntries(Object.entries(fields).map(([key, field]) => [key, Array.isArray(field.value) ? field.value.map(value => value && typeof value === 'object' ? {...value} : value) : field.value])) };
}

export function normalizeParameter(key, value) {
  const field = fields[key];
  if (key === 'outputBitDepth' && ![8,16].includes(value)) throw new Error('位深无效');
  if (!field) throw new Error('未知参数');
  if (field.type === 'stops') {
    if (!Array.isArray(value) || value.length < 2 || value.length > 16 || !value.every(stop => Number.isFinite(stop?.position) && stop.position >= 0 && stop.position <= 1 && /^#[0-9a-f]{6}$/i.test(stop.color))) throw new Error('色标无效');
    const sorted = value.map(stop=>({position:stop.position,color:stop.color.toLowerCase()})).sort((a,b)=>a.position-b.position);
    if (sorted.some((stop,index)=>index && stop.position===sorted[index-1].position)) throw new Error('色标位置重复');
    return sorted;
  }
  if (field.type === 'color5') {
    if (!Array.isArray(value) || value.length !== 5 || !Array.from(value).every(color => typeof color === 'string' && /^#[0-9a-f]{6}$/i.test(color))) throw new Error('色带无效');
    return value.map(color => color.toLowerCase());
  }
  if (field.type === 'vector3') {
    if (!Array.isArray(value) || value.length !== 3 || !value.every(Number.isFinite)) throw new Error('向量无效');
    return [...value];
  }
  if (field.options) {
    if (typeof value !== 'string' || !Object.hasOwn(field.options, value)) throw new Error('参数值无效');
    return value;
  }
  if (field.type === 'color') {
    if (typeof value !== 'string' || !/^#[0-9a-f]{6}$/i.test(value)) throw new Error('颜色无效');
    return value.toLowerCase();
  }
  if (field.type === 'boolean') {
    if (typeof value !== 'boolean') throw new Error('开关值无效');
    return value;
  }
  if (typeof value !== 'number' || !Number.isFinite(value) || value < field.min || value > field.max) throw new Error('数值超出范围');
  const precision = String(field.step).split('.')[1]?.length ?? 0;
  return Number((Math.round(value / field.step) * field.step).toFixed(precision));
}

export function resetAppearance(parameters) {
  parameters.materialEnabled = false;
  parameters.elevationEnabled = false;
  parameters.textureEnabled = false;
  parameters.appearancePick = false;
}
