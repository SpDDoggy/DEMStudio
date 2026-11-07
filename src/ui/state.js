export const rampLibrary = {
  continuous: {
    terrain: { label: '森林·砂岩', colors: ['#364824', '#6d753b', '#9a9a5c', '#c39362', '#e1bb89'] },
    antique: { label: '古地图·暖灰', colors: ['#766f69', '#979088', '#b3afa4', '#cdc9be', '#e4e1d8'] },
    spectral: { label: '光谱', colors: ['#3340a6','#26a6c7','#a6d161','#fab838','#bf2933'] },
    grayscale: { label: '灰度', colors: ['#000000','#404040','#808080','#bfbfbf','#ffffff'] },
  },
  discrete: {
    terrain: { label: '森林·砂岩分级', colors: ['#364824', '#6d753b', '#9a9a5c', '#c39362', '#e1bb89'] },
    antique: { label: '古地图·暖灰分级', colors: ['#766f69', '#979088', '#b3afa4', '#cdc9be', '#e4e1d8'] },
    spectral: { label: '光谱分级', colors: ['#3340a6','#26a6c7','#a6d161','#fab838','#bf2933'] },
    grayscale: { label: '灰度分级', colors: ['#000000','#404040','#808080','#bfbfbf','#ffffff'] },
  },
};

export function rampPalette(parameters) {
  if (parameters.colorRamp === 'custom') return [...parameters.rampColors];
  return [...rampLibrary[parameters.rampInterpolation][parameters.colorRamp].colors];
}

// New display presets reuse the existing native custom-ramp protocol.
export function nativeRamp(parameters) {
  const legacy = parameters.rampInterpolation === 'continuous' && ['grayscale','spectral'].includes(parameters.colorRamp);
  return {colorRamp: legacy ? parameters.colorRamp : 'custom', rampColors: rampPalette(parameters)};
}

export const fields = {
  sunAltitude: { label: '高度角', value: 45, min: 0, max: 90, step: 1 },
  sunAzimuth: { label: '方位角', value: 135, min: 0, max: 360, step: 1 },
  sunSize: { label: '面积', value: 1, min: .1, max: 100, step: .1 },
  sunIntensity: { label: '强度', value: 100, min: 0, max: 500, step: 1 },
  sunColor: { label: '颜色', value: '#fff1db', type: 'color' },
  skyIntensity: { label: '强度', value: 100, min: 0, max: 500, step: 1 },
  skyColor: { label: '颜色', value: '#a9c9e8', type: 'color' },
  fogDensity: { label: '密度', value: 0, min: 0, max: 100, step: .1 },
  cameraYaw: { label: '方位', value: 45, min: -180, max: 180, step: 1 },
  cameraPitch: { label: '俯仰', value: 35, min: 0, max: 90, step: 1 },
  cameraDistance: { label: '距离', value: 10, min: .1, max: 1000, step: .1 },
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
  heightScale: { label: '缩放', value: 1, min: .1, max: 100, step: .1 },
  heightOffset: { label: '偏移', value: 0, min: 0, max: 100, step: 1 },
  heightStep: { label: '量化', value: 0, min: 0, max: 100000, step: .1, range: false },
  lod: { label: '细节层级', value: 0, min: 0, max: 14, step: 1 },
  surface: { label: '表面', value: 'bilinear', options: { cube: '方块', polygon: '多边形', bilinear: '双线性', bicubic: '双三次' } },
  appearanceMode: { value: 'material', options: { texture: '贴图', material: '材质', elevation: '赋色' } },
  colorRamp: { label: '色带', value: 'terrain', options: { terrain: '森林·砂岩', antique: '古地图·暖灰', spectral: '光谱', grayscale: '灰度', custom: '自定义' } },
  rampInterpolation: { label: '过渡', value: 'continuous', options: { continuous: '连续', discrete: '离散' } },
  rampColors: { label: '颜色', value: ['#26523b', '#6e9152', '#b3a36e', '#9c856e', '#f0ede3'], type: 'color5' },
  materialOpacity: { label: '不透明度', value: 100, min: 0, max: 100, step: 1 },
  textureOpacity: { label: '不透明度', value: 100, min: 0, max: 100, step: 1 },
  elevationOpacity: { label: '不透明度', value: 100, min: 0, max: 100, step: 1 },
  materialColor: { label: '颜色', value: '#d5d2c9', type: 'color' },
  width: { label: '宽度', value: 1920, min: 1, max: 243600, step: 1, range: false },
  height: { label: '高度', value: 1080, min: 1, max: 243600, step: 1, range: false },
  samples: { label: '采样', value: 256, min: 1, max: 65536, step: 1, range: false },
  projection: { value: 'perspective', options: { perspective: '透视', orthographic: '正交', isometric: '等轴' } },
  grid: { value: true, type: 'boolean' },
  exportShadow: { label: '范围', value: false, type: 'boolean' },
  shadowPadding: { label: '外扩 (%)', value: 20, min: 1, max: 200, step: 1, range: false },
};

export const sceneGradeKeys = ['sceneExposure', 'sceneGamma', 'sceneBrightness', 'sceneContrast', 'sceneTemperature', 'sceneShadows', 'sceneHighlights'];

export function resetSceneGrading(parameters) {
  for (const key of [...sceneGradeKeys, 'sceneSaturation']) parameters[key] = fields[key].value;
}

export function createSceneState() {
  return { parameters: Object.fromEntries(Object.entries(fields).map(([key, field]) => [key, Array.isArray(field.value) ? [...field.value] : field.value])) };
}

export function normalizeParameter(key, value) {
  const field = fields[key];
  if (!field) throw new Error('未知参数');
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
