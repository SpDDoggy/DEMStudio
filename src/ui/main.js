import { fields, createSceneState, normalizeParameter, sceneGradeKeys, resetSceneGrading, rampLibrary, rampPalette, nativeRamp } from './state.js';
import { projectViewCube, cameraPresets, applyCameraPreset } from './camera-axis.js';
import { cameraPanDelta, nativePanPixels, resetCameraParameters } from './camera-navigation.js';
import { stretchKeys, exportKeys, createExportSettings, exportSnapshot, previewScale, sourceOutputSize, freezeCurrentView, viewOutputSize, proportionalSize, cloneScene } from './export-settings.js';
import { enhanceSelects } from './combobox.js';
import { createExportProgress } from './export-progress.js';

const estimateExportProgress = createExportProgress();
let exportCommandPending = false;
function updateExportProgress(status) {
  const progress = estimateExportProgress(exportCommandPending && !!status?.exporting, status?.exportProgress, performance.now());
  document.querySelector('#export-job').hidden = !progress.visible;
  document.querySelector('#export-job-bar').value = progress.percent;
  document.querySelector('#export-job-percent').textContent = `${progress.percent}%`;
  document.querySelector('#export-job-remaining').textContent = progress.remaining;
}

export function createLatestValueScheduler(send, {
  interval = 16, now = () => performance.now(),
  setTimer = setTimeout, clearTimer = clearTimeout,
  onResult = () => {}, onError = () => {},
} = {}) {
  let pending = null;
  let inFlight = null;
  let timer = null;
  let lastStarted = -Infinity;

  function schedule() {
    if (timer !== null || inFlight || pending === null) return;
    timer = setTimer(() => { timer = null; dispatch(); }, Math.max(0, interval - (now() - lastStarted)));
  }

  function dispatch() {
    if (inFlight || pending === null) return inFlight;
    if (timer !== null) { clearTimer(timer); timer = null; }
    const value = pending;
    pending = null;
    lastStarted = now();
    inFlight = Promise.resolve().then(() => send(value)).then(result => {
      onResult(result);
    }, error => {
      onError(error);
      throw error;
    }).finally(() => { inFlight = null; schedule(); });
    // Normal input has no caller awaiting it; flush still receives the failure.
    inFlight.catch(() => {});
    return inFlight;
  }

  const snapshot = value => value && typeof value === 'object' ? Object.fromEntries(Object.entries(value).map(([key, item]) => [key, Array.isArray(item) ? [...item] : item])) : value;
  return {
    request(value) { pending = snapshot(value); schedule(); },
    async flush(value) {
      pending = snapshot(value);
      if (timer !== null) { clearTimer(timer); timer = null; }
      while (pending !== null || inFlight) await (inFlight ?? dispatch());
    },
  };
}

const scene = createSceneState();
const exportSettings = createExportSettings(scene.parameters);
let exportSceneSnapshot = null;
let exportViewport = null;
let exportSourceStatus = null;
let comboUI = null;
function syncComboboxes() { comboUI?.sync(); }
const invoke = window.__TAURI__?.core?.invoke;
let rendererReady = false;
let renderStatus = null;
let lastError = '';
let nativeDialogOpen = false;
const sourceDetails = document.createElement('div');
sourceDetails.id = 'source-details';
sourceDetails.className = 'source-details';
sourceDetails.hidden = true;
document.querySelector('.map-asset').after(sourceDetails);
const parameterUpdates = createLatestValueScheduler(
  params => invoke('renderer_update', { params }),
  { onResult: applyStatus, onError: error => { void showError(error); } },
);

function rendererParams() {
  return { ...scene.parameters, cameraPan: [...scene.parameters.cameraPan], ...nativeRamp(scene.parameters), surface: scene.parameters.surface === 'polygon' ? 'poly' : scene.parameters.surface };
}

window.__DEMSTUDIO_OVERLAYS__ = () => {
  const scale = window.devicePixelRatio || 1;
  return [...document.querySelectorAll('[data-native-overlay]')].filter(element => {
    const style = getComputedStyle(element);
    return !element.closest('[hidden]') && style.display !== 'none' && style.visibility !== 'hidden' && element.getClientRects().length;
  }).map(element => {
    const r = element.getBoundingClientRect();
    return { x: Math.round(r.x * scale), y: Math.round(r.y * scale), width: Math.ceil(r.width * scale), height: Math.ceil(r.height * scale) };
  }).filter(rect => rect.width > 0 && rect.height > 0);
};
let overlayFrame = null;
function overlaysChanged() {
  if (overlayFrame !== null) return;
  overlayFrame = requestAnimationFrame(() => {
    overlayFrame = null;
    document.dispatchEvent(new Event('demstudio-overlays-changed'));
  });
}
document.addEventListener('demstudio-overlays-changed', () => { void resizeViewport().catch(showError); });
const overlayObserver = new ResizeObserver(overlaysChanged);
document.querySelectorAll('[data-native-overlay]').forEach(element => overlayObserver.observe(element));
document.addEventListener('toggle', overlaysChanged, true);

function viewportRect() {
  const rect = document.querySelector('#viewport').getBoundingClientRect();
  const scale = window.devicePixelRatio || 1;
  return {
    x: Math.round(rect.x * scale), y: Math.round(rect.y * scale),
    width: Math.max(1, Math.round(rect.width * scale)),
    height: Math.max(1, Math.round(rect.height * scale)),
    overlays: window.__DEMSTUDIO_OVERLAYS__(),
    // WebView occlusion may set document.hidden while the native surface is still needed.
    visible: rect.width > 0 && rect.height > 0,
  };
}

async function resizeViewport() {
  if (rendererReady) await invoke('renderer_resize', { rect: viewportRect() });
  if (typeof appWindow !== 'undefined' && appWindow) {
    const [maximized, fullscreen] = await Promise.all([appWindow.isMaximized(), appWindow.isFullscreen()]);
    document.documentElement.classList.toggle('window-square', maximized || fullscreen);
  }
}

async function showError(error) {
  const message = String(error?.message ?? error);
  console.error(error);
  if (lastError === message) return;
  lastError = message;
  document.querySelector('#error-text').textContent = message;
  // Both native and HTML dialogs overlay the surface; keep its last frame visible.
  const dialog = document.querySelector('#error-dialog');
  if (!dialog.open) dialog.showModal();
}

function applyStatus(status) {
  if (!status) return;
  renderStatus = status;
  updateExportProgress(status);
  document.querySelector('#render-progress').textContent = status.exporting ? `导出 ${status.exportProgress}%` : `${status.samples} / ${status.targetSamples}`;
  const toggle = document.querySelector('#render-toggle');
  toggle.disabled = !status.ready || status.exporting;
  toggle.textContent = status.paused ? '渲染' : '暂停';
  document.querySelector('#map-name').textContent = status.mapName;
  document.querySelector('#map-name').title = status.mapName;
  document.querySelector('#import-heightmap').disabled = !status.ready || status.exporting;
  document.querySelector('#import-texture').disabled = !status.ready || status.exporting;
  document.querySelector('#texture-name').textContent = status.textureName || '未导入';
  document.querySelector('#texture-name').title = status.textureName || '';
  document.querySelector('#texture-details').textContent = status.textureName ? `${status.textureWidth} × ${status.textureHeight}` : '';
  document.querySelector('#remove-texture').disabled = !status.ready || !status.textureName || !!status.exporting || removingTexture;
  document.querySelector('[data-appearance=texture]').disabled = !status.textureName || !!status.exporting;
  document.querySelector('#export-image').disabled = !status.ready || status.exporting;
  document.querySelector('#export-geotiff').disabled = !status.ready || status.exporting || !status.source?.geo?.keys?.length;
  document.querySelector('#cancel-export').hidden = !status.exporting;
  document.querySelectorAll('[data-key],[role=tab],[data-mode],button[data-view],[data-appearance=material],[data-appearance=elevation],#reset-camera,#reset-grading,#toggle-grid').forEach(control => { control.disabled = !!status.exporting; });
  const source = status.source;
  sourceDetails.hidden = !source;
  if (source) {
    const crs = sourceCrsLabel(source.geo);
    sourceDetails.textContent = `${source.width} × ${source.height} · ${status.mapWidth} × ${status.mapHeight}${crs ? `\n${crs}` : ''}`;
    sourceDetails.title = sourceDetails.textContent;
  }
  if (typeof updateExportPreviewStatus === 'function') updateExportPreviewStatus(status);
  if (typeof syncExportSource === 'function') syncExportSource(status);
  if (status.error) void showError(status.error);
  if (typeof syncComboboxes === 'function') syncComboboxes();
}

function sourceCrsLabel(geo) {
  if (!geo) return '';
  const keys = geo.keys ?? [];
  for (const id of [3072, 2048]) {
    for (let i = 4; i < 4 + (keys[3] ?? 0) * 4; i += 4) {
      if (keys[i] === id && keys[i + 1] === 0 && keys[i + 2] === 1 && keys[i + 3] > 0 && keys[i + 3] !== 32767) return `EPSG:${keys[i + 3]}`;
    }
  }
  return geo.wkt ?? (geo.ascii ?? '').replace(/[|\0]+$/g, '');
}

function queueParameterUpdate() {
  renderCameraAxis();
  if (!rendererReady || renderStatus?.exporting || removingTexture) return;
  parameterUpdates.request(rendererParams());
  if (document.querySelector('#export-dialog').open) scheduleExportPreview();
}

async function nativeFileCommand(command) {
  if (!rendererReady || nativeDialogOpen || removingTexture) return;
  nativeDialogOpen = true;
  try {
    await flushParameterUpdate();
    const exporting = command === 'renderer_export' || command === 'renderer_export_geotiff';
    exportCommandPending = exporting;
    if (exporting && typeof resetExportResult === 'function') resetExportResult();
    const result = await invoke(command, exporting ? { params: {...exportSnapshot(exportSceneSnapshot ?? rendererParams(), exportSettings), exportShadow: command === 'renderer_export_geotiff' && exportSettings.exportShadow} } : undefined);
    if (exporting && result) await showExportResult();
    if ((command === 'renderer_import' || command === 'renderer_import_texture') && result) applyStatus(result);
    if (command === 'renderer_import_texture' && result) {
      scene.parameters.appearanceMode = 'texture';
      render(); queueParameterUpdate();
    }
  } catch (error) { if (!/^已取消 (TIFF|PNG) 导出$/.test(error?.message ?? String(error))) await showError(error); }
  finally {
    nativeDialogOpen = false;
    exportCommandPending = false;
    updateExportProgress(null);
  }
}

async function flushParameterUpdate() {
  if (rendererReady) await parameterUpdates.flush(rendererParams());
}
const groups = [
  ['#map-fields', '地形', ['surface', 'heightScale', 'heightOffset', 'heightStep']],
  ['#map-fields', '拉伸', stretchKeys],
  ['#postprocess', '拉伸', stretchKeys],
  ['#texture-fields', '贴图', ['textureOpacity']],
  ['#texture-fields', '处理', ['textureExposure', 'textureContrast', 'textureShadows', 'textureHighlights', 'textureSaturation']],
  ['#material-fields', '材质', ['materialColor', 'materialOpacity']],
  ['#elevation-fields', '赋色', ['rampInterpolation', 'colorRamp', 'elevationBlend', 'elevationOpacity']],
  ['#light', '太阳', ['sunAltitude', 'sunAzimuth', 'sunSize', 'sunIntensity', 'sunColor']],
  ['#light', '天空', ['skyIntensity', 'skyColor']],
  ['#grading', '调色', sceneGradeKeys],
  ['#camera', '相机', ['cameraYaw', 'cameraPitch', 'cameraDistance', 'fieldOfView']],
  ['#output', '图像', ['width', 'height']],
  ['#output', '质量', ['samples']],
  ['#shadow-options', '阴影', ['shadowPadding']],
];

function makeControl(key, type) {
  const field = fields[key];
  const input = document.createElement(field.options ? 'select' : 'input');
  input.dataset.key = key;
  input.setAttribute('aria-label', field.label + (type === 'range' ? '滑块' : ''));
  if (field.options) {
    for (const [value, text] of Object.entries(field.options)) input.add(new Option(text, value));
  } else {
    input.type = type ?? field.type ?? 'number';
    if (field.min !== undefined) Object.assign(input, { min: field.min, max: field.max, step: field.step });
  }
  return input;
}
for (const [target, title, keys] of groups) {
  const group = document.createElement('details'); group.open = true;
  const summary = document.createElement('summary'); summary.textContent = title; group.append(summary);
  const container = title === '相机' ? document.createElement('div') : group;
  if (title === '相机') { container.className = 'camera-fields'; group.append(container); }
  for (const key of keys) {
    const field = fields[key];
    const label = document.createElement('label'); label.className = 'field';
    const row = document.createElement('span'); row.className = 'field-line';
    const caption = document.createElement('span'); caption.textContent = field.label;
    const control = makeControl(key);
    if (target === '#postprocess') control.dataset.scope = 'export';
    row.append(caption, control); label.append(row);
    if (field.min !== undefined && field.range !== false) {
      const slider = makeControl(key, 'range');
      if (target === '#postprocess') slider.dataset.scope = 'export';
      label.append(slider);
    }
    container.append(label);
  }
  document.querySelector(target).append(group);
}
const rampEditor = document.createElement('div');
rampEditor.id = 'ramp-colors'; rampEditor.className = 'ramp-colors';
for (let index = 0; index < 5; index++) {
  const input = document.createElement('input');
  input.type = 'color'; input.dataset.key = 'rampColors'; input.dataset.colorIndex = String(index);
  input.setAttribute('aria-label', `高程 ${index * 25}% 颜色`);
  rampEditor.append(input);
}
document.querySelector('#elevation-fields details').append(rampEditor);
const rampPreview = document.createElement('div');
rampPreview.id = 'ramp-preview'; rampPreview.setAttribute('aria-label', '色带预览');
rampEditor.before(rampPreview);

const exportButton = document.createElement('button');
exportButton.id = 'export-image'; exportButton.className = 'primary export-action';
exportButton.textContent = '导出 PNG'; exportButton.disabled = true;
exportButton.addEventListener('click', () => void nativeFileCommand('renderer_export'));
document.querySelector('#export-actions').append(exportButton);
const geoExportButton = document.createElement('button');
geoExportButton.id = 'export-geotiff'; geoExportButton.className = 'primary export-action';
geoExportButton.textContent = '导出正射 TIF'; geoExportButton.disabled = true;
geoExportButton.addEventListener('click', () => void nativeFileCommand('renderer_export_geotiff'));
const cancelExportButton = document.createElement('button');
cancelExportButton.id = 'cancel-export'; cancelExportButton.className = 'export-action';
cancelExportButton.textContent = '取消导出'; cancelExportButton.hidden = true;
cancelExportButton.addEventListener('click', () => { void invoke('renderer_cancel_export').catch(showError); });
document.querySelector('#export-actions').append(geoExportButton, cancelExportButton);
// Export preview reads the real progressive GPU frame only on demand.
let exportSourceKey = null;
function updateExportDimensions() {
  let size = exportSettings;
  if (document.querySelector('#export-format').value === 'geotiff') {
    const source = sourceOutputSize(exportSourceStatus ?? renderStatus);
    if (!source) return;
    const margin = exportSettings.exportShadow ? Math.ceil(Math.max(source.width,source.height)*exportSettings.shadowPadding/100) : 0;
    size = {width:source.width+2*margin,height:source.height+2*margin};
  }
  for (const key of ['width','height']) {
    document.querySelector('#output').querySelectorAll(`[data-key=${key}]`).forEach(input => {input.value=size[key];});
  }
}
function currentExportSize(multiplier=1) {
  const status=exportSourceStatus ?? renderStatus;
  return exportSettings.exportOrthographic ? sourceOutputSize(status,multiplier) : viewOutputSize(status,exportViewport ?? document.querySelector('#viewport').getBoundingClientRect(),multiplier);
}
function setExportScale(multiplier) {
  const size = currentExportSize(multiplier);
  if (!size) return;
  Object.assign(exportSettings, size);
  document.querySelector('#export-scale').value = String(multiplier);
  updateExportDimensions();
  scheduleExportPreview();
  if (typeof syncComboboxes === 'function') syncComboboxes();
}
function syncExportSource(status) {
  if (document.querySelector('#export-dialog').open && exportSourceStatus) return;
  const size=sourceOutputSize(status);
  if (!size) return;
  const key=`${status.source?.path ?? status.mapName}:${size.width}:${size.height}`;
  if (key===exportSourceKey) return;
  exportSourceKey=key;
  setExportScale(1);
}
document.querySelector('#export-scale').addEventListener('change', event => {
  if (renderStatus?.exporting || event.target.value === 'custom') return;
  setExportScale(Number(event.target.value));
});
let previewUrl = null;
let previewTimer = null;
let previewBusy = false;
let previewRevision = 0;
let previewZoom = 1;
let exportedPreview = false;
function setPreviewZoom(value) {
  previewZoom = previewScale(value);
  document.querySelector('#export-preview-image').style.transform = `scale(${previewZoom})`;
  document.querySelector('#preview-zoom').textContent = `${Math.round(previewZoom * 100)}%`;
}
document.querySelector('#preview-zoom-in').addEventListener('click', () => setPreviewZoom(previewZoom * 1.25));
document.querySelector('#preview-zoom-out').addEventListener('click', () => setPreviewZoom(previewZoom / 1.25));
document.querySelector('#preview-fit').addEventListener('click', () => setPreviewZoom(1));
document.querySelector('.preview-image').addEventListener('wheel', event => {
  event.preventDefault(); event.stopPropagation();
  setPreviewZoom(previewZoom * Math.exp(-Math.sign(event.deltaY) * .15));
}, { passive: false });
document.querySelector('#export-preview-image').addEventListener('dragstart', event => event.preventDefault());
function scheduleExportPreview() {
  previewRevision++;
  exportedPreview = false;
  clearTimeout(previewTimer);
  previewTimer = setTimeout(() => { void refreshExportPreview(); }, 450);
}
function updateExportPreviewStatus(status) {
  const dialog = document.querySelector('#export-dialog');
  if (!dialog.open) return;
  document.querySelector('#preview-progress').textContent = status.exporting ? `${status.exportProgress}%` : (previewBusy ? '渲染中' : exportedPreview ? '已导出' : '4 采样');
  document.querySelector('#refresh-preview').disabled = !status.ready || !!status.exporting || previewBusy;
  document.querySelector('#close-export').disabled = !!status.exporting;
  document.querySelector('#export-format').disabled = !!status.exporting;
  document.querySelector('#export-framing').disabled = !!status.exporting || document.querySelector('#export-format').value === 'geotiff';
  document.querySelector('#export-scale').disabled = !!status.exporting || document.querySelector('#export-format').value === 'geotiff';
  const orthographic = document.querySelector('#export-format').value === 'geotiff';
  document.querySelector('#export-extent').disabled = !!status.exporting;
  document.querySelector('#shadow-options').querySelectorAll('input').forEach(input => { input.disabled = !!status.exporting; });
  document.querySelector('#output').querySelectorAll('[data-key=width],[data-key=height]').forEach(input => { input.disabled = orthographic || !!status.exporting; });
  updateExportDimensions();
}
async function refreshExportPreview() {
  const dialog = document.querySelector('#export-dialog');
  if (!dialog.open || !rendererReady || renderStatus?.exporting || previewBusy) return;
  resetExportResult();
  previewBusy = true;
  const revision = previewRevision;
  document.querySelector('#refresh-preview').disabled = true;
  document.querySelector('#preview-progress').textContent = '渲染中';
  try {
    await flushParameterUpdate();
    const orthographic = exportSettings.exportOrthographic;
    const bytes = await invoke('renderer_preview', {orthographic, params: {...exportSnapshot(exportSceneSnapshot ?? rendererParams(), exportSettings), exportShadow: document.querySelector('#export-format').value === 'geotiff' && exportSettings.exportShadow}});
    if (!dialog.open || revision !== previewRevision) return;
    const next = URL.createObjectURL(new Blob([new Uint8Array(bytes)], {type:'image/png'}));
    document.querySelector('#export-preview-image').src = next;
    if (previewUrl) URL.revokeObjectURL(previewUrl);
    previewUrl = next;
    setPreviewZoom(previewZoom);
    document.querySelector('#preview-progress').textContent = '4 采样';
  } catch (error) { if (dialog.open && revision === previewRevision) await showError(error); }
  finally {
    previewBusy = false;
    document.querySelector('#refresh-preview').disabled = !rendererReady || !!renderStatus?.exporting;
    if (dialog.open && revision !== previewRevision && !exportedPreview) scheduleExportPreview();
  }
}
function resetExportResult() {
  exportedPreview=false;
  document.querySelector('#preview-progress').textContent='预览';
  document.querySelector('#open-export-file').hidden=true;
  document.querySelector('#open-export-directory').hidden=true;
}
for (const [id,directory] of [['open-export-file',false],['open-export-directory',true]]) {
  document.querySelector(`#${id}`).addEventListener('click',()=> {
    if(exportedPreview) void invoke('renderer_open_export',{directory}).catch(showError);
  });
}
async function showExportResult() {
  const dialog=document.querySelector('#export-dialog');
  const revision=++previewRevision;
  clearTimeout(previewTimer);
  const bytes=await invoke('renderer_export_preview_result');
  if (!dialog.open || revision!==previewRevision) return;
  document.querySelector('#open-export-file').hidden=false;
  document.querySelector('#open-export-directory').hidden=false;
  const next=URL.createObjectURL(new Blob([new Uint8Array(bytes)],{type:'image/png'}));
  document.querySelector('#export-preview-image').src=next;
  if(previewUrl) URL.revokeObjectURL(previewUrl);
  previewUrl=next; exportedPreview=true; clearTimeout(previewTimer);
  setPreviewZoom(previewZoom);
  document.querySelector('#preview-progress').textContent='已导出';
}
function setExportFormat() {
  const geotiff=document.querySelector('#export-format').value==='geotiff';
  const framing=document.querySelector('#export-framing');
  if(geotiff) framing.value='source';
  framing.disabled=geotiff || !!renderStatus?.exporting;
  exportSettings.exportOrthographic=geotiff || framing.value==='source';
  document.querySelector('#geo-options').hidden=!geotiff;
  document.querySelector('#export-scale').disabled=geotiff || !!renderStatus?.exporting;
  document.querySelector('#preview-kind').textContent=exportSettings.exportOrthographic?'原图正射':'当前视图';
  document.querySelector('#export-image').hidden=geotiff;
  document.querySelector('#export-geotiff').hidden=!geotiff;
  setExportScale(1);
  updateExportDimensions();
  scheduleExportPreview();
  if(typeof syncComboboxes==='function') syncComboboxes();
}
document.querySelector('#export-framing').addEventListener('change',setExportFormat);
document.querySelector('#export-format').addEventListener('change', setExportFormat);
document.querySelector('#export-extent').addEventListener('change', event => {
  if (renderStatus?.exporting) return;
  exportSettings.exportShadow = event.target.value === 'shadow';
  document.querySelector('#shadow-options').hidden = !exportSettings.exportShadow;
  updateExportDimensions();
  scheduleExportPreview();
});
document.querySelector('#export-geotiff').hidden = true;
document.querySelector('#refresh-preview').addEventListener('click', () => void refreshExportPreview());
document.querySelector('#close-export').addEventListener('click', () => {
  if (!renderStatus?.exporting) document.querySelector('#export-dialog').close();
});
document.querySelector('#export-dialog').addEventListener('cancel', event => {
  if (renderStatus?.exporting) event.preventDefault();
});
document.querySelector('#export-dialog').addEventListener('close', () => {
  previewRevision++; clearTimeout(previewTimer);
  setPreviewZoom(1);
  document.querySelector('#export-preview-image').removeAttribute('src');
  if (previewUrl) { URL.revokeObjectURL(previewUrl); previewUrl = null; }
  exportSceneSnapshot=null; exportViewport=null; exportSourceStatus=null; resetExportResult();
  overlaysChanged();
});

document.querySelector('#import-heightmap').disabled = true;
document.querySelector('#import-heightmap').addEventListener('click', () => void nativeFileCommand('renderer_import'));
document.querySelector('#import-texture').addEventListener('click', () => void nativeFileCommand('renderer_import_texture'));
const removeTextureButton = document.createElement('button');
removeTextureButton.id = 'remove-texture'; removeTextureButton.className = 'wide';
removeTextureButton.textContent = '移除贴图'; removeTextureButton.disabled = true;
document.querySelector('[data-appearance-panel=texture] .texture-asset').after(removeTextureButton);
let removingTexture = false;
async function removeTexture() {
  if (!rendererReady || renderStatus?.exporting || !renderStatus?.textureName || removingTexture) return;
  removingTexture = true; removeTextureButton.disabled = true;
  try {
    await flushParameterUpdate();
    const status = await invoke('renderer_remove_texture');
    scene.parameters.appearanceMode = 'material';
    applyStatus(status); render(); queueParameterUpdate(); overlaysChanged();
  } catch (error) { await showError(error); }
  finally {
    removingTexture = false;
    removeTextureButton.disabled = !renderStatus?.textureName || !!renderStatus?.exporting;
  }
}
removeTextureButton.addEventListener('click', () => void removeTexture());
document.querySelectorAll('[data-appearance]').forEach(button => button.addEventListener('click', () => {
  if (renderStatus?.exporting || removingTexture || button.disabled) return;
  scene.parameters.appearanceMode = normalizeParameter('appearanceMode', button.dataset.appearance);
  render(); queueParameterUpdate(); overlaysChanged();
}));

function renderCameraAxis() {
  const faces = document.querySelector('#view-cube-faces');
  faces.replaceChildren();
  for (const face of projectViewCube(scene.parameters)) {
    const group = document.createElementNS('http://www.w3.org/2000/svg', 'g');
    group.dataset.view = face.name;
    group.setAttribute('role', 'button'); group.setAttribute('tabindex', '0');
    group.setAttribute('aria-label', `${face.label}视图`);
    group.setAttribute('aria-disabled', String(!!renderStatus?.exporting));
    const polygon = document.createElementNS('http://www.w3.org/2000/svg', 'polygon');
    polygon.setAttribute('points', face.points.map(([x,y]) => `${(50+x*19).toFixed(3)},${(46+y*19).toFixed(3)}`).join(' '));
    const label = document.createElementNS('http://www.w3.org/2000/svg', 'text');
    label.setAttribute('x', (50+face.center[0]*19).toFixed(3));
    label.setAttribute('y', (46+face.center[1]*19).toFixed(3));
    label.textContent = face.label;
    group.append(polygon, label); faces.append(group);
  }
}

const viewPresets = document.createElement('div');
viewPresets.className = 'view-presets';
viewPresets.setAttribute('role', 'group'); viewPresets.setAttribute('aria-label', '标准视图');
for (const [name, preset] of Object.entries(cameraPresets)) {
  const button = document.createElement('button');
  button.dataset.view = name; button.textContent = preset.label;
  button.setAttribute('aria-label', `${preset.label}视图`); viewPresets.append(button);
}
document.querySelector('#camera').append(viewPresets);
function selectStandardView(name) {
  if (renderStatus?.exporting || !applyCameraPreset(scene.parameters, name)) return;
  render(); queueParameterUpdate();
}
for (const control of [document.querySelector('#view-cube'), viewPresets]) {
  control.addEventListener('click', event => {
    const view = event.target.closest('[data-view]');
    if (view) selectStandardView(view.dataset.view);
  });
}
document.querySelector('#view-cube').addEventListener('keydown', event => {
  if (event.key !== 'Enter' && event.key !== ' ') return;
  const view = event.target.closest('g[data-view]');
  if (view) { event.preventDefault(); selectStandardView(view.dataset.view); }
});

function render() {
  renderCameraAxis();
  document.querySelectorAll('[data-appearance]').forEach(button => {
    button.setAttribute('aria-pressed', String(button.dataset.appearance === scene.parameters.appearanceMode));
  });
  document.querySelectorAll('[data-appearance-panel]').forEach(panel => {
    panel.hidden = panel.dataset.appearancePanel !== scene.parameters.appearanceMode;
  });
  document.querySelector('#ramp-colors').hidden = scene.parameters.colorRamp !== 'custom';
  const library = rampLibrary[scene.parameters.rampInterpolation];
  document.querySelectorAll('[data-key="colorRamp"] option').forEach(option => {
    option.textContent = option.value === 'custom' ? '自定义' : library[option.value].label;
  });
  const colors = rampPalette(scene.parameters);
  const stops = colors.map((color, index) => scene.parameters.rampInterpolation === 'discrete' ? `${color} ${index*20}% ${(index+1)*20}%` : `${color} ${index*25}%`);
  document.querySelector('#ramp-preview').style.background = `linear-gradient(to right,${stops.join(',')})`;
  document.querySelectorAll('[data-key]').forEach(input => {
    input.value = input.dataset.colorIndex !== undefined ? scene.parameters.rampColors[Number(input.dataset.colorIndex)] : (exportKeys.has(input.dataset.key) || input.dataset.scope === 'export' ? exportSettings : scene.parameters)[input.dataset.key];
  });
  document.querySelectorAll('[data-key=stretchDeviations],[data-key=stretchLow],[data-key=stretchHigh]').forEach(input => {
    const settings = input.dataset.scope === 'export' ? exportSettings : scene.parameters;
    input.closest('.field').hidden = input.dataset.key === 'stretchDeviations' ? settings.stretchType !== 'stddev' : settings.stretchType !== 'percentclip';
  });
  document.querySelectorAll('[data-mode]').forEach(button => {
    const selected = button.dataset.mode === scene.parameters.projection;
    button.classList.toggle('active', selected);
    button.setAttribute('aria-pressed', String(selected));
  });
  document.querySelector('#toggle-grid').classList.toggle('active', scene.parameters.grid);
  document.querySelector('#toggle-grid').setAttribute('aria-pressed', String(scene.parameters.grid));
  document.querySelector('#viewport').classList.toggle('grid-off', !scene.parameters.grid);
  if (typeof syncComboboxes === 'function') syncComboboxes();
}

const cameraControlKeys = new Set();
let cameraControlFrame = null;
function refreshCameraControls(...keys) {
  keys.forEach(key => cameraControlKeys.add(key));
  if (cameraControlFrame !== null) return;
  cameraControlFrame = requestAnimationFrame(() => {
    cameraControlFrame = null;
    for (const key of cameraControlKeys) {
      document.querySelectorAll(`[data-key="${key}"]`).forEach(input => { input.value = scene.parameters[key]; });
    }
    cameraControlKeys.clear();
  });
}

document.addEventListener('input', event => {
  const input = event.target;
  if (!input.dataset.key || renderStatus?.exporting || removingTexture || input.disabled) return;
  try {
    const key = input.dataset.key;
    const field = fields[key];
    let value = field.min !== undefined ? input.valueAsNumber : input.value;
    if (exportKeys.has(key) || input.dataset.scope === 'export') {
      const normalized = normalizeParameter(key, value);
      if (key === 'width' || key === 'height') {
        const size=proportionalSize(currentExportSize(),key,normalized);
        size.width=normalizeParameter('width',size.width);
        size.height=normalizeParameter('height',size.height);
        Object.assign(exportSettings,size);
        document.querySelector('#export-scale').value = 'custom';
        if (typeof syncComboboxes === 'function') syncComboboxes();
      } else exportSettings[key]=normalized;
      input.setCustomValidity('');
      document.querySelectorAll(`[data-key="${key}"]`).forEach(peer => {
        if (peer !== input && (peer.dataset.scope ?? '') === (input.dataset.scope ?? '')) peer.value = exportSettings[key];
      });
      if (key === 'stretchType') { render(); overlaysChanged(); }
      scheduleExportPreview();
      if (typeof updateExportDimensions === 'function') updateExportDimensions();
      return;
    }
    if (field.type === 'color5') {
      const index = Number(input.dataset.colorIndex);
      if (!Number.isInteger(index) || index < 0 || index > 4) throw new Error('色带无效');
      value = [...scene.parameters.rampColors];
      value[index] = input.value;
    }
    scene.parameters[key] = normalizeParameter(key, value);
    if (key === 'rampColors') scene.parameters.colorRamp = 'custom';
    if ((key === 'colorRamp' || key === 'rampInterpolation') && scene.parameters.colorRamp !== 'custom') scene.parameters.rampColors = rampPalette(scene.parameters);
    input.setCustomValidity('');
    document.querySelectorAll(`[data-key="${key}"]`).forEach(peer => {
      if (peer !== input && peer.dataset.scope !== 'export') peer.value = field.type === 'color5' ? scene.parameters[key][Number(peer.dataset.colorIndex)] : scene.parameters[key];
    });
    if (key === 'colorRamp' || key === 'rampColors' || key === 'rampInterpolation') { render(); overlaysChanged(); }
    if (key === 'stretchType') { render(); overlaysChanged(); }
    queueParameterUpdate();
  } catch { input.setCustomValidity('数值无效'); }
});

document.addEventListener('change', event => {
  if (event.target.dataset.key) render();
});
function selectTab(button) {
  if (renderStatus?.exporting || button.disabled) return;
  const tablist = button.closest('[role="tablist"]');
  tablist.querySelectorAll('[role="tab"]').forEach(item => {
    item.setAttribute('aria-selected', String(item === button));
    item.tabIndex = item === button ? 0 : -1;
    document.getElementById(item.getAttribute('aria-controls')).hidden = item !== button;
  });
  overlaysChanged();
}
document.querySelectorAll('[role="tab"]').forEach(button => {
  button.addEventListener('click', () => selectTab(button));
  button.addEventListener('keydown', event => {
    if (renderStatus?.exporting || button.disabled) return;
    if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
    const tabs = [...button.closest('[role="tablist"]').querySelectorAll('[role="tab"]')];
    const index = tabs.indexOf(button);
    const next = event.key === 'Home' ? 0 : event.key === 'End' ? tabs.length - 1 : (index + (event.key === 'ArrowRight' ? 1 : -1) + tabs.length) % tabs.length;
    event.preventDefault(); selectTab(tabs[next]); tabs[next].focus();
  });
});
function panelVisible(id, visible) {
  document.querySelector(`#${id}`).hidden = !visible;
  document.querySelector(`#${id === 'map-panel' ? 'show-map' : 'show-inspector'}`).hidden = visible;
  if (id === 'map-panel') document.querySelector('#map-panel').classList.toggle('mobile-open', visible);
  document.querySelector('#panels').setAttribute('aria-pressed', String(!document.querySelector('#inspector').hidden));
  overlaysChanged();
}
document.querySelectorAll('[data-hide]').forEach(button => button.addEventListener('click', () => panelVisible(button.dataset.hide, false)));
document.querySelector('#show-map').addEventListener('click', () => panelVisible('map-panel', true));
document.querySelector('#show-inspector').addEventListener('click', () => panelVisible('inspector', true));
function togglePanels() {
  const visible = document.querySelector('#inspector').hidden;
  panelVisible('map-panel', visible); panelVisible('inspector', visible);
}
document.querySelector('#panels').addEventListener('click', togglePanels);
function cameraOpen(visible) {
  document.querySelector('#camera').hidden = !visible;
  document.querySelector('#camera-toggle').setAttribute('aria-expanded', String(visible)); overlaysChanged();
}
document.querySelector('#camera-toggle').addEventListener('click', () => cameraOpen(document.querySelector('#camera').hidden));
document.addEventListener('click', event => {
  if (!event.target.closest('#camera,#camera-toggle')) cameraOpen(false);
});
document.addEventListener('keydown', event => {
  if (event.key === 'Escape') cameraOpen(false);
  if (event.key === 'Tab' && event.target === document.body && !document.querySelector('dialog[open]')) { event.preventDefault(); togglePanels(); }
});
document.querySelector('#show-output').addEventListener('click', () => {
  const dialog = document.querySelector('#export-dialog');
  if (!dialog.open) {
    const rect=document.querySelector('#viewport').getBoundingClientRect();
    exportViewport={width:rect.width,height:rect.height};
    exportSceneSnapshot=freezeCurrentView(rendererParams(),exportViewport);
    exportSourceStatus=cloneScene(renderStatus ?? {});
    for (const key of stretchKeys) exportSettings[key] = scene.parameters[key];
    resetExportResult(); setExportFormat();
    render(); dialog.showModal(); overlaysChanged();
  }
  void refreshExportPreview();
});
if (matchMedia('(max-width:800px)').matches) panelVisible('map-panel', false);
const appWindow = window.__TAURI__?.window?.getCurrentWindow();
for (const [id, action] of [['window-minimize', 'minimize'], ['window-maximize', 'toggleMaximize'], ['window-close', 'close']]) {
  document.querySelector(`#${id}`).addEventListener('click', () => {
    if (appWindow) void appWindow[action]().catch(showError);
  });
}
document.querySelector('.title-drag').addEventListener('dblclick', () => { if (appWindow) void appWindow.toggleMaximize().catch(showError); });
document.querySelectorAll('[data-mode]').forEach(button => button.addEventListener('click', () => {
  scene.parameters.projection = button.dataset.mode;
  render();
  queueParameterUpdate();
}));
document.querySelector('#toggle-grid').addEventListener('click', () => {
  scene.parameters.grid = !scene.parameters.grid;
  render();
  queueParameterUpdate();
});
document.querySelector('#reset-grading').addEventListener('click', () => {
  if (renderStatus?.exporting) return;
  resetSceneGrading(scene.parameters);
  render();
  queueParameterUpdate();
});
document.querySelector('#reset-camera').addEventListener('click', () => {
  resetCameraParameters(scene.parameters, fields);
  render();
  queueParameterUpdate();
});
document.querySelector('#close-error').addEventListener('click', () => document.querySelector('#error-dialog').close());
document.querySelector('#error-dialog').addEventListener('close', () => {
  lastError = '';
  void resizeViewport().catch(showError);
});
document.querySelector('#render-toggle').addEventListener('click', async () => {
  if (!rendererReady || !renderStatus) return;
  try {
    await flushParameterUpdate();
    applyStatus(await invoke('renderer_pause', { paused: !renderStatus.paused }));
  } catch (error) { await showError(error); }
});
function orbitCamera(dx, dy) {
  if (renderStatus?.exporting) return;
  scene.parameters.cameraYaw = ((scene.parameters.cameraYaw + dx * .3 + 180) % 360 + 360) % 360 - 180;
  scene.parameters.cameraPitch = Math.max(0, Math.min(90, scene.parameters.cameraPitch + dy * .3));
  refreshCameraControls('cameraYaw', 'cameraPitch'); queueParameterUpdate();
}
function zoomCamera(delta) {
  if (renderStatus?.exporting) return;
  scene.parameters.cameraDistance = Math.max(.1, Math.min(1000, scene.parameters.cameraDistance * Math.exp(-delta * .1)));
  refreshCameraControls('cameraDistance'); queueParameterUpdate();
}
function panCamera(dx, dy) {
  if (renderStatus?.exporting) return;
  const delta = cameraPanDelta(scene.parameters, dx, dy, viewport.getBoundingClientRect());
  scene.parameters.cameraPan = scene.parameters.cameraPan.map((value, index) => value + delta[index]);
  queueParameterUpdate();
}
const viewport = document.querySelector('#viewport');
let orbitPointer = null;
viewport.addEventListener('contextmenu', event => event.preventDefault());
viewport.addEventListener('pointerdown', event => {
  if (event.button !== 2) return;
  orbitPointer = { id: event.pointerId, x: event.clientX, y: event.clientY };
  viewport.setPointerCapture(event.pointerId); event.preventDefault();
});
viewport.addEventListener('pointermove', event => {
  if (orbitPointer?.id !== event.pointerId) return;
  if (!(event.buttons & 2)) { finishOrbit(event); return; }
  const moveCamera = event.shiftKey ? panCamera : orbitCamera;
  moveCamera(event.clientX - orbitPointer.x, event.clientY - orbitPointer.y);
  orbitPointer.x = event.clientX; orbitPointer.y = event.clientY;
});
function finishOrbit(event) {
  if (orbitPointer?.id !== event.pointerId) return;
  orbitPointer = null;
  if (viewport.hasPointerCapture(event.pointerId)) viewport.releasePointerCapture(event.pointerId);
}
for (const name of ['pointerup', 'pointercancel', 'lostpointercapture']) viewport.addEventListener(name, finishOrbit);
viewport.addEventListener('wheel', event => {
  event.preventDefault();
  const pixels = event.deltaY * (event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? viewport.clientHeight : 1);
  zoomCamera(-pixels / 100);
}, { passive: false });
render();
comboUI = enhanceSelects(document, overlaysChanged);
if (invoke) {
  (async () => {
    await invoke('app_info');
    let status = await invoke('renderer_init', { rect: viewportRect(), params: rendererParams() });
    const deadline = performance.now() + 30000;
    while (!status?.ready || !status.mapWidth || !status.width) {
      if (status?.error) throw new Error(status.error);
      if (performance.now() >= deadline) throw new Error('渲染初始化超时');
      await new Promise(resolve => setTimeout(resolve, 50));
      status = await invoke('renderer_status');
    }
    if (status.error) throw new Error(status.error);
    rendererReady = true;
    applyStatus(status);
    await flushParameterUpdate();
    new ResizeObserver(() => void resizeViewport().catch(showError)).observe(document.querySelector('#viewport'));
    window.addEventListener('resize', () => void resizeViewport().catch(showError));
    document.addEventListener('visibilitychange', () => void resizeViewport().catch(showError));
    const listen = window.__TAURI__.event.listen;
    await listen('viewport-orbit', event => { if (!document.querySelector('#export-dialog').open) orbitCamera(event.payload.dx, event.payload.dy); });
    await listen('viewport-pan', event => { if (!document.querySelector('#export-dialog').open) panCamera(...nativePanPixels(event.payload, window.devicePixelRatio)); });
    await listen('viewport-zoom', event => { if (!document.querySelector('#export-dialog').open) zoomCamera(event.payload.delta); });
    let polling = false;
    setInterval(async () => {
      if (polling || document.hidden) return;
      polling = true;
      try { applyStatus(await invoke('renderer_status')); }
      catch (error) { await showError(error); }
      finally { polling = false; }
    }, 250);
    await invoke('ui_ready', { controlCount: document.querySelectorAll('button,input,select').length });
    window.__DEMSTUDIO_UI_READY__ = true;
    document.body.dataset.nativeReady = 'true';
  })().catch(error => { document.body.dataset.nativeReady = 'false'; void showError(error); });
}
