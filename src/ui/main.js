import {fitTerrainView,focusViewportRegion} from './mcp-camera-fit.js';
import {createMcpHudControl} from './mcp-hud.js';
import {designControlViews,sequenceControlViews} from './mcp-view-design.js';
import {installLayoutEditor,layoutRamp} from './layout-editor.js';
import {attachRuntimeControl,createControlJobs,validateControlPatch,controlTransitions,controlTransitionKeys,mergeControlParameters} from './mcp-control.js';
import { createSceneTransition, captureSceneSettings, sceneTransitionDuration, mergeSceneView } from './scene-transition.js';
import { createVideoWorkflow } from './video-workflow.js';
import { createVideoRendering, timelineFrames } from './video-rendering.js';
import { createNamedViews, createNamedViewsStore, validateNamedView, viewParameterKeys } from './named-views.js';
import { createExportFrame, fitFrame } from './export-frame.js';
import { builtinPresets } from './builtin-presets.js';
import { installContextActions } from './context-actions.js';
import { applyUiContrast } from './ui-contrast.js';
import { createPalettePanel } from './palette-panel.js';
import { createColorPicker } from './color-picker.js';
import { createRampEditor } from './ramp-editor.js';
import { createAppearanceHistory } from './appearance-history.js';
import { createRampWorkbench } from './ramp-workbench.js';
import { fields, createSceneState, normalizeParameter, sceneGradeKeys, resetSceneGrading, rampLibrary, rampPalette, rampStops, sampledRampPosition, nativeRamp, resampleRamp, resetAppearance } from './state.js';
import { projectNavigationCube, roundedFacePath, cameraPresets, applyCameraPreset } from './camera-axis.js';
import { cameraPanDelta, nativePanPixels, resetCameraParameters } from './camera-navigation.js';
import { stretchKeys, exportKeys, createExportSettings, exportRequest, previewScale, sourceOutputSize, freezeCurrentView, viewOutputSize, sourceHasCrs, cloneScene } from './export-settings.js';
import { enhanceSelects } from './combobox.js';
import { createExportProgress } from './export-progress.js';
import { createPresetStore, defaultPresetSettings, applyPresetSettings, capturePresetSettings, applySnapshotSettings } from './render-presets.js';
import { createSourceSettings, flushSettingsOnClose } from './source-settings.js';
import { nativeHeightScaling } from './height-scaling.js';
import { bounceLimit, switchSkyModel, toggleExperimentalBounces, acceptsExperimentalShortcut } from './lighting-controls.js';

const estimateExportProgress = createExportProgress();
let exportCommandPending = false;
let videoWorkflow=null,videoRendering=null,videoBusy=false;
function updateExportProgress(status) {
  if(videoBusy){document.querySelector('#export-job').hidden=true;return;}
  const progress = estimateExportProgress(exportCommandPending && !!status?.exporting, status?.exportProgress, performance.now(), status?.exportWorkProgress);
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
let presetStretchScene = null;
let exportSceneSnapshot = null;
let exportViewport = null;
let exportSourceStatus = null;
let exportFrameState = null, exportFrameUI = null, namedViews = null, framingBackup = null, viewTransition = null;
let comboUI = null;
let palettePanel = null, colorPicker = null;
function syncComboboxes() { comboUI?.sync(); palettePanel?.sync(); colorPicker?.sync(); }
const invoke = window.__TAURI__?.core?.invoke;
let mcpHud=null;
let rendererReady = false;
let renderStatus = null;
let lastRampStatusKey = null;
let lastError = '';
let nativeDialogOpen = false;
let appearanceHistory = null;
let appearanceHistoryBusy = false;
let textureAsset = null;
let mcpImportedSourceKey = null;
let appearanceDistribution = null;
let distributionRevision = 0;
let distributionSource = null;
const appearanceHistoryKeys = Object.keys(scene.parameters).filter(key => /^(material|texture|elevation|ramp)/.test(key) || key === 'colorRamp' || key === 'appearanceLayersVersion');
function externalAppearanceLocked() { return !!renderStatus?.exporting || nativeDialogOpen || removingTexture; }
function appearanceLocked() { return externalAppearanceLocked() || appearanceHistoryBusy; }
function captureAppearance() { return { parameters: Object.fromEntries(appearanceHistoryKeys.map(key => [key, structuredClone(scene.parameters[key])])), textureAsset }; }
async function refreshAppearanceDistribution() {
  if (!rendererReady || !invoke) return;
  const revision = ++distributionRevision;
  try { const result = await invoke('renderer_appearance_distribution'); if (revision === distributionRevision) { appearanceDistribution = result; drawRampEditor(); } }
  catch (error) { if (revision === distributionRevision) console.warn('高程分布', error); }
}
const sourceSettings = createSourceSettings({invoke:(...args)=>invoke(...args),getSettings:currentSourceSettings,applySettings:installSourceSnapshot,
  confirmRestore:()=>new Promise(resolve=>{
    document.querySelector('#import-dialog').close();
    const dialog=document.querySelector('#restore-settings-dialog');
    dialog.returnValue='skip';
    dialog.addEventListener('close',()=>{overlaysChanged();resolve(dialog.returnValue==='restore');},{once:true});
    dialog.showModal();overlaysChanged();
  }),
  onWarning:error=>console.warn('设置快照',error)});
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
  return { ...scene.parameters, contourEnabled: false, ...nativeHeightScaling(scene.parameters), cameraPan: [...scene.parameters.cameraPan], ...nativeRamp(scene.parameters), surface: scene.parameters.surface === 'polygon' ? 'poly' : scene.parameters.surface };
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
  mcpHud?.end('application-error');
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
  if(videoBusy)status={...status,exporting:true};
  renderStatus = status;
  const sourceIdentity = JSON.stringify([status.source?.path,status.mapName,status.mapWidth,status.mapHeight]);
  if (sourceIdentity !== distributionSource) { if(typeof viewTransition!=='undefined')viewTransition?.cancel(); exportFrameState=null; namedViews?.refresh(); if(typeof videoWorkflow!=='undefined')videoWorkflow?.refresh({reset:true}); distributionSource = sourceIdentity; appearanceDistribution = null; void refreshAppearanceDistribution(); }
  updateExportProgress(status);
  document.querySelector('#render-progress').textContent = status.exporting ? `导出 ${status.exportProgress}%` : `${status.samples} / ${status.targetSamples}`;
  const toggle = document.querySelector('#render-toggle');
  toggle.disabled = !status.ready || status.exporting;
  toggle.textContent = status.paused ? '渲染' : '暂停';
  document.querySelector('#map-name').textContent = status.mapName;
  document.querySelector('#map-name').title = status.mapName;
  document.querySelector('#import-heightmap').disabled = !status.ready || status.exporting;
  document.querySelector('#import-texture').disabled = !status.ready || status.exporting;
  document.querySelectorAll('.appearance-reset,#background-toggle').forEach(control=>control.disabled=!!status.exporting);
  const rampStatusKey=JSON.stringify([status.exporting,status.ready,status.textureName,status.appearanceRange]);
  if (lastRampStatusKey !== rampStatusKey) {lastRampStatusKey=rampStatusKey;drawRampEditor();}
  document.querySelector('#texture-name').textContent = status.textureName || '未导入';
  document.querySelector('#texture-name').title = status.textureName || '';
  document.querySelector('#texture-details').textContent = status.textureName ? `${status.textureWidth} × ${status.textureHeight}` : '';
  document.querySelector('#remove-texture').disabled = !status.ready || !status.textureName || !!status.exporting || removingTexture;
  document.querySelector('[data-appearance=texture]').disabled = !status.textureName || !!status.exporting;
  document.querySelector('#export-image').disabled = !status.ready || status.exporting;
  document.querySelector('#export-geotiff').disabled = !status.ready || !!status.exporting;
  document.querySelector('#cancel-export').hidden = !status.exporting;
  document.querySelectorAll('[data-key],[role=tab],[data-mode],button[data-view],[data-appearance=material],[data-appearance=elevation],#views-toggle,#reset-camera,#reset-grading,#reset-effects,#toggle-grid,#render-preset,#preset-name,#save-preset,#confirm-preset,#cancel-preset-save,#apply-preset,#remove-preset,#reset-render,#reset-appearance').forEach(control => { control.disabled = !!status.exporting; });
  document.querySelectorAll('[data-key=textureEnabled]').forEach(control => { control.disabled = !!status.exporting || !status.textureName; });
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
  appearanceHistory?.refresh();
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
  reconcilePresetSelection();
  renderCameraAxis();
  appearanceHistory?.record();
  if (!rendererReady || renderStatus?.exporting || removingTexture) return;
  parameterUpdates.request(rendererParams());
  sourceSettings.request();
  if (document.querySelector('#export-dialog').open) scheduleExportPreview();
}

async function nativeFileCommand(command, extra = {}) {
  if(typeof viewTransition!=='undefined')viewTransition?.cancel();
  if (!rendererReady || nativeDialogOpen || appearanceLocked()) return;
  appearanceHistory?.end();
  nativeDialogOpen = true;
  appearanceHistory?.refresh();
  const meshExport = command === 'renderer_export_mesh';
  const exporting = meshExport || command === 'renderer_export' || command === 'renderer_export_geotiff';
  let result = null;
  try {
    const format = meshExport ? document.querySelector('#export-mesh-format').value : exporting ? document.querySelector('#export-format').value : null;
    const request = meshExport ? {params:{...cloneScene(exportSceneSnapshot ?? rendererParams()),exportTerrainScale:exportSettings.exportTerrainScale}} : exporting ? exportRequest(exportSceneSnapshot ?? rendererParams(), exportSettings, format) : null;
    exportCommandPending = exporting;
    if (exporting) {
      previewRevision++; clearTimeout(previewTimer);
      resetExportResult();
    }
    await flushParameterUpdate();
    if (command === 'renderer_import') await sourceSettings.suspend();
    result = await invoke(command, exporting ? { ...extra, params: request.params, ...((command === 'renderer_export' || meshExport) ? {format} : {}) } : undefined);
    if (exporting && result) await showExportResult(meshExport);
    if ((command === 'renderer_import' || command === 'renderer_import_texture') && result) applyStatus(result);
    if(command==='renderer_import'&&result)mcpImportedSourceKey=null;
    if (command === 'renderer_import' && Number.isFinite(result?.initialHeightScale)) {
      scene.parameters.heightScale = normalizeParameter('heightScale', result.initialHeightScale * 5);
      scene.parameters.heightScaleMode = 'linear';
      render(); queueParameterUpdate();
    }
    if (command === 'renderer_import' && result) {
      await sourceSettings.open(result.source?.path);
      rampWorkbench.manualEdit();
      appearanceDistribution=null;void refreshAppearanceDistribution();
    }
    if (command === 'renderer_import_texture' && result) {
      textureAsset = result.textureAsset ?? null;
      scene.parameters.appearanceMode = 'texture';
      scene.parameters.textureEnabled = true;
      render(); queueParameterUpdate();
    }
  } catch (error) { if (command === 'renderer_import') document.querySelector('#import-dialog').close(); if (!/^已取消 (TIFF|PNG|JPEG|BMP|TGA|PLY|OBJ) 导出$/.test(error?.message ?? String(error))) await showError(error); }
  finally {
    if (command === 'renderer_import') { document.querySelector('#import-dialog').close(); overlaysChanged(); }
    nativeDialogOpen = false;
    if (command === 'renderer_import' && result) appearanceHistory?.clear();
    else appearanceHistory?.record();
    appearanceHistory?.refresh();
    restoreAppearanceControls();
    if (command === 'renderer_import') sourceSettings.resume();
    exportCommandPending = false;
    updateExportProgress(null);
    if (exporting && document.querySelector('#export-dialog').open && !exportedPreview) scheduleExportPreview();
  }
}

async function flushParameterUpdate() {
  if (rendererReady) await parameterUpdates.flush(rendererParams());
}
const groups = [
  ['#map-fields', '地形', ['surface', 'heightScaleMode', 'heightScale', 'heightOffset', 'heightStep']],
  ['#texture-fields', '贴图', ['textureEnabled', 'textureOpacity', 'textureBlend']] ,
  ['#texture-fields', '映射', ['textureMapping', 'textureFit', 'textureScale', 'textureOffsetX', 'textureOffsetY', 'textureRotation']],
  ['#texture-fields', '处理', ['textureExposure', 'textureContrast', 'textureShadows', 'textureHighlights', 'textureSaturation']],
  ['#material-fields', '材质', ['materialEnabled', 'materialColor', 'materialOpacity', 'materialRoughness', 'materialSpecular']],
  ['#material-fields', '细节', ['materialSpecularColor']],
  ['#elevation-fields', '赋色', ['elevationEnabled', 'rampInterpolation', 'colorRamp', 'rampReverse']],
  ['#elevation-fields', '范围', ['rampRangeMode', 'rampMin', 'rampMax']],
  ['#elevation-fields', '偏移', ['rampOffset', 'rampWrap']],
  ['#background', '背景', ['backgroundMode', 'skyFollowLight', 'backgroundColor', 'backgroundColor2', 'backgroundFollowLight', 'backgroundAngle']],
  ['#background', '地面', ['groundEnabled', 'groundColor', 'groundShadow']],
  ['#elevation-fields', '表现', ['elevationBlend', 'elevationOpacity']],
  ['#light', '太阳', ['sunAltitude', 'sunAzimuth', 'sunSize', 'sunIntensity', 'sunColor']],
  ['#light', '天空', ['skyModel', 'skyIntensity', 'skyColor']],
  ['#light', '路径追踪', ['maxBounces']],
  ['#effects', '雾', ['fogEnabled', 'fogDensity', 'fogColor', 'fogStart', 'fogEnd', 'fogHeightEnabled', 'fogHeightBase', 'fogHeightFalloff']],
  ['#effects', '云', ['cloudEnabled', 'cloudShadowEnabled', 'cloudShadowStrength', 'cloudCoverage', 'cloudDensity', 'cloudColor', 'cloudHeight', 'cloudThickness', 'cloudRange', 'cloudScale', 'cloudOffsetX', 'cloudOffsetY', 'cloudSeed']],
  ['#effects', '水面', ['waterEnabled', 'waterLevel', 'waterOpacity', 'waterColor', 'waterRoughness']],
  ['#grading', '调色', sceneGradeKeys],
  ['#camera', '相机', ['cameraYaw', 'cameraPitch', 'cameraDistance', 'fieldOfView']],
  ['#output', '图像', ['width', 'height', 'exportTransparent']],
  ['#output', '质量', ['samples']],
  ['#output', '独立图层', ['exportNormal', 'exportDepth', 'exportShadowLayer']],
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
    input.type = field.type === 'boolean' ? 'checkbox' : type ?? field.type ?? 'number';
    if (field.type === 'boolean') input.className = 'toggle-input';
    if (field.min !== undefined) Object.assign(input, { min: field.min, max: key === 'maxBounces' ? bounceLimit(scene.parameters) : field.max, step: field.step });
  }
  return input;
}
for (const [target, title, keys] of groups) {
  const group = document.createElement('details'); group.open = !['映射','处理','细节','偏移','范围','表现'].includes(title);
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
const rampEditorContainer = document.createElement('div');
rampEditorContainer.id = 'ramp-colors';
const rampGroup = [...document.querySelectorAll('#elevation-fields details')].find(group=>group.querySelector('summary').textContent==='赋色');
rampGroup.append(rampEditorContainer);
const rampWorkbenchContainer = document.createElement('div');
rampWorkbenchContainer.id = 'ramp-workbench';
rampGroup.append(rampWorkbenchContainer);
function appearanceRange() { return scene.parameters.rampRangeMode === 'custom' ? [scene.parameters.rampMin,scene.parameters.rampMax] : renderStatus?.appearanceRange; }
function commitRampPatch(patch) {
  for (const [key,value] of Object.entries(patch)) scene.parameters[key] = normalizeParameter(key,value);
  render(); queueParameterUpdate(); sourceSettings.request();
}
const rampWorkbench = createRampWorkbench({
  container:rampWorkbenchContainer,getParameters:()=>scene.parameters,getRange:appearanceRange,
  getDistribution:()=>appearanceDistribution,onChange:commitRampPatch,isLocked:appearanceLocked,onError:error=>void showError(error),
});
function commitStops(stops,{kind}={}) {
  if (kind === 'position' || kind === 'delete') rampWorkbench.manualEdit();
  const patch = kind === 'count' || kind === 'resize' ? rampWorkbench.classificationPatch(stops) : {colorRamp:'custom',rampStops:stops};
  commitRampPatch(patch);
}
const rampEditor = createRampEditor({
  container: rampEditorContainer,
  getParameters: () => scene.parameters,
  getRange: appearanceRange,
  resizeStops: (...args) => rampWorkbench.resize(...args),
  onError: error => void showError(error),
  onChange: commitStops,
  canPickHeight: () => rendererReady && !!renderStatus?.ready,
  canPickTexture: () => !!renderStatus?.textureName,
  isLocked: appearanceLocked,
  onPickHeight: () => { if(!rendererReady)return;scene.parameters.appearancePick=!scene.parameters.appearancePick;drawRampEditor();void flushParameterUpdate().catch(showError); },
  onPickTexture: callback => {if(renderStatus?.textureName)void openTexturePicker(callback);},
});
function drawRampEditor() { rampEditor.render(); rampWorkbench.render(); }
for(const [target,keys] of [['#background',['backgroundMode','skyFollowLight','backgroundColor','backgroundColor2','backgroundFollowLight','backgroundAngle','groundEnabled','groundColor','groundShadow']],['#texture-fields',['textureMapping','textureFit','textureScale','textureOffsetX','textureOffsetY','textureRotation','textureBlend','textureExposure','textureContrast','textureShadows','textureHighlights','textureSaturation','textureOpacity']],['#material-fields',['materialColor','materialOpacity','materialRoughness','materialSpecular','materialSpecularColor']],['#elevation-fields',['colorRamp','rampInterpolation','rampStops','rampColors','rampOffset','rampReverse','rampWrap','rampRangeMode','rampMin','rampMax','elevationBlend','elevationOpacity']]]) {
  const reset=document.createElement('button');reset.textContent='恢复默认';reset.className='appearance-reset';reset.addEventListener('click',()=>{if(appearanceLocked())return;const defaults=createSceneState().parameters;keys.forEach(key=>scene.parameters[key]=structuredClone(defaults[key]));render();queueParameterUpdate();sourceSettings.request();});document.querySelector(target).append(reset);
}

async function sampleAppearance(event) {
      if(!scene.parameters.appearancePick||appearanceLocked())return;
      try { await flushParameterUpdate();const sample=await invoke('renderer_sample_appearance',{x:event.payload.x,y:event.payload.y});
        if (sample) {
          const position = sampledRampPosition(scene.parameters, sample);
          if (position !== null) {
            rampEditor.setSamplePosition(position);
          }
        }
      } catch(error){await showError(error);}finally{scene.parameters.appearancePick=false;render();queueParameterUpdate();}

}
let texturePickerCallback=null;
const texturePicker=document.createElement('dialog');texturePicker.className='texture-picker';texturePicker.innerHTML='<h2>贴图取色</h2><canvas aria-label="贴图取色"></canvas><div class="dialog-actions"><button>关闭</button></div>';document.body.append(texturePicker);
texturePicker.setAttribute('data-native-overlay','');
overlayObserver.observe(texturePicker);
texturePicker.addEventListener('close',()=>{texturePickerCallback=null;overlaysChanged();});
texturePicker.querySelector('button').addEventListener('click',()=>texturePicker.close());
async function openTexturePicker(callback) {
  try { await flushParameterUpdate();const bytes=await invoke('renderer_texture_preview');const bitmap=await createImageBitmap(new Blob([new Uint8Array(bytes)],{type:'image/png'}));const canvas=texturePicker.querySelector('canvas');canvas.width=bitmap.width;canvas.height=bitmap.height;canvas.getContext('2d').drawImage(bitmap,0,0);bitmap.close();texturePickerCallback=callback;texturePicker.showModal();overlaysChanged(); }catch(error){await showError(error);}
}
texturePicker.querySelector('canvas').addEventListener('click',event=>{const canvas=event.currentTarget,bounds=canvas.getBoundingClientRect();const x=Math.max(0,Math.min(canvas.width-1,Math.floor((event.clientX-bounds.left)/bounds.width*canvas.width)));const y=Math.max(0,Math.min(canvas.height-1,Math.floor((event.clientY-bounds.top)/bounds.height*canvas.height)));const pixel=canvas.getContext('2d').getImageData(x,y,1,1).data;if(!pixel[3])return;texturePickerCallback?.('#'+Array.from(pixel.slice(0,3),value=>value.toString(16).padStart(2,'0')).join(''));texturePicker.close();});

const exportButton = document.createElement('button');
exportButton.id = 'export-image'; exportButton.className = 'primary export-action';
exportButton.textContent = '导出 PNG'; exportButton.disabled = true;
exportButton.addEventListener('click', () => void nativeFileCommand(document.querySelector('#export-type').value==='mesh'?'renderer_export_mesh':'renderer_export'));
document.querySelector('#export-actions').append(exportButton);
const geoExportButton = document.createElement('button');
geoExportButton.id = 'export-geotiff'; geoExportButton.className = 'primary export-action';
geoExportButton.textContent = '导出 TIFF'; geoExportButton.disabled = true;
geoExportButton.addEventListener('click', () => void nativeFileCommand('renderer_export_geotiff'));
const cancelExportButton = document.createElement('button');
cancelExportButton.id = 'cancel-export'; cancelExportButton.className = 'export-action';
cancelExportButton.textContent = '取消导出'; cancelExportButton.hidden = true;
cancelExportButton.addEventListener('click', () => { void invoke('renderer_cancel_export').catch(showError); });
document.querySelector('#export-actions').append(geoExportButton, cancelExportButton);
// Export preview reads the real progressive GPU frame only on demand.
let exportSourceKey = null;
function updateExportDimensions() {
  const size = exportSettings;
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
function isMeshExport() { return document.querySelector('#export-type').value==='mesh'; }
function scheduleExportPreview() {
  previewRevision++;
  exportedPreview = false;
  clearTimeout(previewTimer);
  if (isMeshExport() || document.querySelector('#export-type').value==='video') return;
  previewTimer = setTimeout(() => { void refreshExportPreview(); }, 450);
}
function updateExportPreviewStatus(status) {
  const dialog = document.querySelector('#export-dialog');
  if (!dialog.open) return;
  document.querySelector('#preview-progress').textContent = status.exporting ? `${status.exportProgress}%` : (previewBusy ? '渲染中' : exportedPreview ? '已导出' : '4 采样');
  document.querySelector('#refresh-preview').disabled = !status.ready || !!status.exporting || previewBusy;
  document.querySelector('#select-export-frame').disabled = !!status.exporting || previewBusy || exportSettings.exportOrthographic || document.querySelector('#export-type').value==='mesh';
  document.querySelector('#close-export').disabled = !!status.exporting;
  document.querySelector('#export-format').disabled = !!status.exporting;
  document.querySelector('#export-type').disabled = !!status.exporting;
  document.querySelector('#export-mesh-format').disabled = !!status.exporting;
  syncExportFormatControls();
  document.querySelector('#export-terrain-scale').disabled = !!status.exporting;
  document.querySelector('#export-framing').disabled = !!status.exporting;
  document.querySelector('#export-scale').disabled = !!status.exporting;
  document.querySelector('#export-extent').disabled = !!status.exporting;
  document.querySelector('#shadow-options').querySelectorAll('input').forEach(input => { input.disabled = !!status.exporting; });
  document.querySelector('#output').querySelectorAll('[data-key=width],[data-key=height]').forEach(input => { input.disabled = !!status.exporting; });
  syncExportGeoreference();
  updateExportDimensions();
}
async function refreshExportPreview() {
  const dialog = document.querySelector('#export-dialog');
  if (!dialog.open || isMeshExport() || document.querySelector('#export-type').value==='video' || !rendererReady || renderStatus?.exporting || exportCommandPending || previewBusy) return;
  const request = exportRequest(exportSceneSnapshot ?? rendererParams(), exportSettings, document.querySelector('#export-format').value);
  resetExportResult();
  previewBusy = true;
  const revision = previewRevision;
  document.querySelector('#refresh-preview').disabled = true;
  document.querySelector('#preview-progress').textContent = '渲染中';
  try {
    await flushParameterUpdate();
    if (!dialog.open || revision !== previewRevision || exportCommandPending) return;
    const bytes = await invoke('renderer_preview', request);
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
    if (dialog.open && revision !== previewRevision && !exportedPreview && !exportCommandPending) scheduleExportPreview();
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
async function showExportResult(mesh = false) {
  const dialog=document.querySelector('#export-dialog');
  const revision=++previewRevision;
  clearTimeout(previewTimer);
  if (mesh) {
    exportedPreview=true;
    document.querySelector('#open-export-file').hidden=false;
    document.querySelector('#open-export-directory').hidden=false;
    return;
  }
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
function syncExportGeoreference() {
  const available=sourceHasCrs(exportSourceStatus ?? renderStatus);
  const checkbox=document.querySelector('#export-georeference');
  if (!available) exportSettings.exportGeoreference=false;
  checkbox.checked=!!exportSettings.exportGeoreference;
  checkbox.disabled=!available || !!renderStatus?.exporting;
}
function syncExportFormatControls() {
  const format=document.querySelector('#export-format').value;
  const limited=['jpeg','bmp','tga'].includes(format);
  const opaque=['jpeg','bmp'].includes(format);
  const depth=document.querySelector('#export-depth');
  depth.value=String(limited?8:exportSettings.outputBitDepth);
  depth.disabled=limited || !!renderStatus?.exporting;
  document.querySelector('#output').querySelectorAll('[data-key=exportTransparent]').forEach(input=>{
    input.checked=opaque?false:!!exportSettings.exportTransparent;
    input.disabled=opaque || !!renderStatus?.exporting;
  });
}
function syncExportTypeControls() {
  const mesh=isMeshExport(),video=document.querySelector('#export-type').value==='video';
  document.querySelector('#export-dialog').classList.toggle('video-active',video);
  if(typeof videoWorkflow!=='undefined')videoWorkflow?.setActive(video);
  document.querySelector('.export-preview').hidden=mesh||video;
  document.querySelector('#export-model-options').hidden=!mesh;
  document.querySelector('.export-settings').querySelectorAll('[data-export-image]').forEach(element=>{element.hidden=mesh||video;});
  document.querySelector('#export-terrain-scale').closest('.field').hidden=video;
  if (!mesh&&!video) document.querySelector('#geo-options').hidden=!exportSettings.exportOrthographic;
  document.querySelector('#export-image').hidden=video || !mesh && document.querySelector('#export-format').value==='geotiff';
  document.querySelector('#export-geotiff').hidden=video || mesh || document.querySelector('#export-format').value!=='geotiff';
  document.querySelector('#export-image').textContent=`导出 ${(mesh?document.querySelector('#export-mesh-format').value:document.querySelector('#export-format').value).toUpperCase()}`;
}
function setExportFormat(preserveSize = false) {
  const format=document.querySelector('#export-format').value;
  const geotiff=format==='geotiff';
  syncExportFormatControls();
  document.querySelector('#export-image').textContent=`导出 ${format.toUpperCase()}`;
  const framing=document.querySelector('#export-framing');
  framing.disabled=!!renderStatus?.exporting;
  exportSettings.exportOrthographic=framing.value==='source';
  document.querySelector('#geo-options').hidden=!exportSettings.exportOrthographic;
  document.querySelector('#export-scale').disabled=!!renderStatus?.exporting;
  syncExportGeoreference();
  document.querySelector('#preview-kind').textContent=exportSettings.exportOrthographic?'原图正射':'当前视图';
  document.querySelector('#export-image').hidden=geotiff;
  document.querySelector('#export-geotiff').hidden=!geotiff;
  if (preserveSize === false) setExportScale(1);
  else if (preserveSize === true && !exportSettings.exportOrthographic) {
    const multiplier=Number(document.querySelector('#export-scale').value);
    const preset=[1,2,3].includes(multiplier);
    const size=currentExportSize(preset?multiplier:1);
    if (size) {
      if (!preset) {
        const scale=Math.max(exportSettings.width,exportSettings.height)/Math.max(size.width,size.height);
        size.width=Math.max(1,Math.round(size.width*scale));
        size.height=Math.max(1,Math.round(size.height*scale));
      }
      Object.assign(exportSettings,size);
    }
  }
  syncExportTypeControls();
  updateExportDimensions();
  scheduleExportPreview();
  if(typeof syncComboboxes==='function') syncComboboxes();
}
document.querySelector('#export-type').addEventListener('change',()=>{resetExportResult();syncExportTypeControls();scheduleExportPreview();});
document.querySelector('#export-mesh-format').addEventListener('change',()=>{resetExportResult();syncExportTypeControls();});
document.querySelector('#export-framing').addEventListener('change',() => setExportFormat());
document.querySelector('#export-format').addEventListener('change', () => {
  setExportFormat('format');
});
document.querySelector('#export-georeference').addEventListener('change', event => {
  if (renderStatus?.exporting) return;
  exportSettings.exportGeoreference=sourceHasCrs(exportSourceStatus ?? renderStatus) && event.target.checked;
  syncExportGeoreference();
  scheduleExportPreview();
});
document.querySelector('#export-extent').addEventListener('change', event => {
  if (renderStatus?.exporting) return;
  exportSettings.exportShadow = event.target.value === 'shadow';
  document.querySelector('#shadow-options').hidden = !exportSettings.exportShadow;
  updateExportDimensions();
  scheduleExportPreview();
});
document.querySelector('#export-terrain-scale').addEventListener('change', event => {
  if (renderStatus?.exporting) return;
  exportSettings.exportTerrainScale = normalizeParameter('exportTerrainScale', event.target.value);
  resetExportResult(); scheduleExportPreview();
});

document.querySelector('#export-depth').addEventListener('change', event => {
  if (renderStatus?.exporting) return;
  exportSettings.outputBitDepth = Number(event.target.value);
  resetExportResult(); scheduleExportPreview();
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
  videoWorkflow?.stopPreview();videoRendering?.stopPreview();
  previewRevision++; clearTimeout(previewTimer);
  setPreviewZoom(1);
  document.querySelector('#export-preview-image').removeAttribute('src');
  if (previewUrl) { URL.revokeObjectURL(previewUrl); previewUrl = null; }
  exportSceneSnapshot=null; exportViewport=null; exportSourceStatus=null; resetExportResult();
  overlaysChanged();
});

document.querySelector('#import-heightmap').disabled = true;
for(const id of ['import-dialog','startup-dialog'])document.getElementById(id).addEventListener('cancel',event=>event.preventDefault());
document.querySelector('#import-heightmap').addEventListener('click', () => void nativeFileCommand('renderer_import'));
document.querySelector('#import-texture').addEventListener('click', () => void nativeFileCommand('renderer_import_texture'));
const removeTextureButton = document.createElement('button');
removeTextureButton.id = 'remove-texture'; removeTextureButton.className = 'wide';
removeTextureButton.textContent = '移除贴图'; removeTextureButton.disabled = true;
document.querySelector('[data-appearance-panel=texture] .texture-asset').after(removeTextureButton);
let removingTexture = false;
const historyActions = document.createElement('span');
historyActions.className = 'appearance-history-actions';
const undoAppearance = document.createElement('button');
const redoAppearance = document.createElement('button');
for (const [button,label,symbol] of [[undoAppearance,'撤销','↶'],[redoAppearance,'重做','↷']]) {
  button.type='button';button.textContent=symbol;button.title=label;button.setAttribute('aria-label',label);button.disabled=true;historyActions.append(button);
}
document.querySelector('#reset-appearance').before(historyActions);
function refreshAppearanceHistoryControls(state) {
  appearanceHistoryBusy=state.busy;
  undoAppearance.disabled=!state.canUndo;redoAppearance.disabled=!state.canRedo;
  const locked=state.busy||!!renderStatus?.exporting||nativeDialogOpen||removingTexture;
  document.querySelectorAll('#appearance input,#appearance select,#appearance button').forEach(control=>{
    if(control===undoAppearance||control===redoAppearance)return;
    if(locked)control.disabled=true;
  });
  pruneAppearanceAssets();
}
let lastPrunedAssets = '';
let pruneTail = Promise.resolve();
appearanceHistory = createAppearanceHistory({
  capture:captureAppearance,isLocked:externalAppearanceLocked,onChange:refreshAppearanceHistoryControls,
  restore:async(snapshot,previous)=>{
    await flushParameterUpdate();
    await pruneTail;
    let textureChanged=false;
    try {
      if(snapshot.textureAsset!==previous.textureAsset&&rendererReady) {
        applyStatus(await invoke('renderer_restore_texture',{asset:snapshot.textureAsset}));textureChanged=true;
      }
      Object.assign(scene.parameters,structuredClone(snapshot.parameters));
      scene.parameters.appearancePick=false;
      if(rendererReady)await parameterUpdates.flush(rendererParams());
      textureAsset=snapshot.textureAsset;
      rampWorkbench.manualEdit();
      render();sourceSettings.request();overlaysChanged();
    } catch(error) {
      textureAsset=previous.textureAsset;
      Object.assign(scene.parameters,structuredClone(previous.parameters));
      if(textureChanged&&rendererReady)applyStatus(await invoke('renderer_restore_texture',{asset:previous.textureAsset}));
      if(rendererReady)await parameterUpdates.flush(rendererParams());
      render();throw error;
    }
  },
});
function pruneAppearanceAssets() {
  if(!rendererReady||!invoke)return;
  pruneTail=pruneTail.then(async()=>{
    const keep=[...new Set(appearanceHistory.snapshots().map(snapshot=>snapshot.textureAsset).filter(Boolean))];
    const key=JSON.stringify(keep);if(key===lastPrunedAssets)return;
    await invoke('renderer_prune_texture_assets',{keep});lastPrunedAssets=key;
  }).catch(error=>console.warn('贴图历史清理',error));
  return pruneTail;
}
async function moveAppearanceHistory(direction) {
  try { const changed=await appearanceHistory[direction]();if(changed)pruneAppearanceAssets(); }
  catch(error){await showError(error);}
  finally {restoreAppearanceControls();appearanceHistory.refresh();}
}
function restoreAppearanceControls() {
  document.querySelectorAll('#appearance input,#appearance select,#appearance button').forEach(control=>control.disabled=false);
  render();
  if(renderStatus)applyStatus(renderStatus);
  else {
    document.querySelector('#import-texture').disabled=!rendererReady;
    document.querySelector('#remove-texture').disabled=true;
    document.querySelector('[data-appearance=texture]').disabled=true;
    document.querySelectorAll('[data-key=textureEnabled]').forEach(control=>control.disabled=true);
  }
}
undoAppearance.addEventListener('click',()=>void moveAppearanceHistory('undo'));
redoAppearance.addEventListener('click',()=>void moveAppearanceHistory('redo'));
appearanceHistory.refresh();
document.addEventListener('keydown',event=>{
  if(!(event.ctrlKey||event.metaKey)||event.altKey||document.querySelector('dialog[open]'))return;
  const editing=event.target?.closest?.('textarea,[contenteditable=true],input:not([type=range]):not([type=color]):not([type=checkbox])');
  if(editing)return;
  const key=event.key.toLowerCase();
  const direction=key==='z'?(event.shiftKey?'redo':'undo'):key==='y'?'redo':null;
  if(!direction||appearanceLocked())return;
  event.preventDefault();void moveAppearanceHistory(direction);
});
// Pointer sliders and color dialogs are a single edit, irrespective of input count.
document.addEventListener('pointerdown',event=>{
  if(appearanceLocked())return;
  const target=event.target;
  if(target.closest?.('#appearance')&&(target.type==='range'||target.type==='color'||target.closest?.('.ramp-track')))appearanceHistory.begin();
},true);
document.addEventListener('pointerup',()=>{appearanceHistory.end();pruneAppearanceAssets();},true);
document.addEventListener('pointercancel',()=>{appearanceHistory.end();pruneAppearanceAssets();},true);
document.addEventListener('focusin',event=>{
  if(event.target.closest?.('#appearance')&&['number','text','color','range'].includes(event.target.type))appearanceHistory.begin();
},true);
document.addEventListener('focusout',()=>{appearanceHistory.end();pruneAppearanceAssets();},true);
async function removeTexture() {
  if (!rendererReady || appearanceLocked() || !renderStatus?.textureName) return;
  removingTexture = true; removeTextureButton.disabled = true;
  try {
    await flushParameterUpdate();
    const status = await invoke('renderer_remove_texture');
    textureAsset = null;
    scene.parameters.textureEnabled = false;
    applyStatus(status); render(); queueParameterUpdate(); overlaysChanged();
  } catch (error) { await showError(error); }
  finally {
    removingTexture = false;
    appearanceHistory.record();appearanceHistory.refresh();pruneAppearanceAssets();
    restoreAppearanceControls();
    removeTextureButton.disabled = !renderStatus?.textureName || !!renderStatus?.exporting;
  }
}
removeTextureButton.addEventListener('click', () => void removeTexture());
document.querySelector('#reset-appearance').addEventListener('click', () => {
  if (appearanceLocked()) return;
  resetAppearance(scene.parameters);
  render(); queueParameterUpdate(); overlaysChanged();
});
document.querySelectorAll('[data-appearance]').forEach(button => button.addEventListener('click', () => {
  if (appearanceLocked() || button.disabled) return;
  scene.parameters.appearanceMode = normalizeParameter('appearanceMode', button.dataset.appearance);
  render(); queueParameterUpdate(); overlaysChanged();
}));

let lastCubeState='';
function renderCameraAxis() {
  const key=JSON.stringify([scene.parameters.cameraYaw,scene.parameters.cameraPitch,scene.parameters.projection,!!renderStatus?.exporting]);
  if(key===lastCubeState)return;
  lastCubeState=key;
  const faces = document.querySelector('#view-cube-faces');
  faces.replaceChildren();
  for (const face of projectNavigationCube(scene.parameters)) {
    const group = document.createElementNS('http://www.w3.org/2000/svg', 'g');
    if(face.enabled)group.dataset.view = face.name;
    group.setAttribute('class', `cube-${face.kind}`);
    if(face.enabled){group.setAttribute('role', 'button');group.setAttribute('tabindex', '0');}
    group.setAttribute('aria-label', `${face.label}视图`);
    group.setAttribute('aria-disabled', String(!face.enabled||!!renderStatus?.exporting));
    const polygon = document.createElementNS('http://www.w3.org/2000/svg', 'path');
    polygon.setAttribute('d', roundedFacePath(face.points.map(([x,y])=>[50+x*19,46+y*19])));
    const label = document.createElementNS('http://www.w3.org/2000/svg', 'text');
    label.setAttribute('x', (50+face.center[0]*19).toFixed(3));
    label.setAttribute('y', (46+face.center[1]*19).toFixed(3));
    label.textContent = face.kind==='face'?({top:'T',front:'F',back:'B',left:'L',right:'R'}[face.name]||''):'';
    const title = document.createElementNS('http://www.w3.org/2000/svg', 'title');
    title.textContent = `${face.label}视图`;
    group.append(title, polygon, label); faces.append(group);
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
  if(rendererReady)void parameterUpdates.flush(rendererParams()).catch(showError);
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

function renderAppearanceButtons() {
  document.querySelectorAll('[data-appearance]').forEach(button => {
    const mode = button.dataset.appearance;
    const enabled = scene.parameters[`${mode}Enabled`];
    button.setAttribute('aria-pressed', String(mode === scene.parameters.appearanceMode));
    button.setAttribute('aria-label', `${fields.appearanceMode.options[mode]}，${enabled ? '已启用' : '已关闭'}`);
    button.classList.toggle('effect-enabled', enabled);
  });
}

function render() {
  applyUiContrast(scene.parameters);
  renderCameraAxis();
  document.querySelector('#reset-appearance').disabled = !!renderStatus?.exporting;
  renderAppearanceButtons();
  document.querySelectorAll('[data-appearance-panel]').forEach(panel => {
    panel.hidden = panel.dataset.appearancePanel !== scene.parameters.appearanceMode;
  });
  const library = rampLibrary[scene.parameters.rampInterpolation];
  document.querySelectorAll('[data-key="colorRamp"] option').forEach(option => { option.textContent = option.value === 'custom' ? '自定义' : library[option.value].label; });
  drawRampEditor();
  document.querySelectorAll('[data-key]').forEach(input => {
    if (fields[input.dataset.key]?.type === 'boolean') { input.checked = (exportKeys.has(input.dataset.key) ? exportSettings : scene.parameters)[input.dataset.key]; return; }
    if (input.dataset.key.startsWith('contour')) input.closest('.field').hidden = !scene.parameters.contourEnabled;
    if (input.dataset.key === 'maxBounces') { input.max = bounceLimit(scene.parameters); input.setCustomValidity(''); }
    input.value = input.dataset.colorIndex !== undefined ? scene.parameters.rampColors[Number(input.dataset.colorIndex)] : (exportKeys.has(input.dataset.key) || input.dataset.scope === 'export' ? exportSettings : scene.parameters)[input.dataset.key];
  });
  document.querySelectorAll('[data-key=stretchDeviations],[data-key=stretchLow],[data-key=stretchHigh]').forEach(input => {
    const settings = input.dataset.scope === 'export' ? exportSettings : scene.parameters;
    input.closest('.field').hidden = input.dataset.key === 'stretchDeviations' ? settings.stretchType !== 'stddev' : settings.stretchType !== 'percentclip';
  });
  for (const [key,visible] of [['skyFollowLight',scene.parameters.backgroundMode==='sky'],['backgroundColor',scene.parameters.backgroundMode!=='sky'],['backgroundColor2',scene.parameters.backgroundMode==='gradient'],['backgroundFollowLight',scene.parameters.backgroundMode==='gradient'],['backgroundAngle',scene.parameters.backgroundMode==='gradient'&&!scene.parameters.backgroundFollowLight],['groundColor',scene.parameters.groundEnabled&&scene.parameters.backgroundMode==='sky'&&!scene.parameters.skyFollowLight],['groundShadow',scene.parameters.groundEnabled],['rampMin',scene.parameters.rampRangeMode==='custom'],['rampMax',scene.parameters.rampRangeMode==='custom']]) document.querySelectorAll(`[data-key=${key}]`).forEach(input=>input.closest('.field').hidden=!visible);
  document.querySelectorAll('[data-mode]').forEach(button => {
    const selected = button.dataset.mode === scene.parameters.projection;
    button.classList.toggle('active', selected);
    button.setAttribute('aria-pressed', String(selected));
  });
  document.querySelector('#toggle-grid').classList.toggle('active', scene.parameters.grid);
  document.querySelector('#toggle-grid').setAttribute('aria-pressed', String(scene.parameters.grid));
  document.querySelector('#viewport').classList.toggle('grid-off', !scene.parameters.grid);
  if (typeof syncExportFormatControls === 'function') syncExportFormatControls();
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
  if (!input.dataset.key || appearanceLocked() || input.disabled) return;
  try {
    const key = input.dataset.key;
    const field = fields[key];
    let value = field.type === 'boolean' ? input.checked : field.min !== undefined ? input.valueAsNumber : input.value;
    if (exportKeys.has(key) || input.dataset.scope === 'export') {
      const normalized = normalizeParameter(key, value);
      if (key === 'width' || key === 'height') {
        exportSettings[key]=normalized;
        document.querySelector('#export-scale').value = 'custom';
        if (typeof syncComboboxes === 'function') syncComboboxes();
      } else exportSettings[key]=normalized;
      if(['heightScale','heightScaleMode'].includes(key))delete scene.parameters._transitionHeight;
    if(key.startsWith('background')||key.startsWith('sky')){for(const transient of ['transitionBackgroundFrom','transitionBackgroundMix','transitionSkyFollow','transitionSkyModel'])delete scene.parameters[transient];}
    input.setCustomValidity('');
      document.querySelectorAll(`[data-key="${key}"]`).forEach(peer => {
        if (peer !== input && (peer.dataset.scope ?? '') === (input.dataset.scope ?? '')) peer.value = exportSettings[key];
      });
      if (key === 'stretchType') { render(); overlaysChanged(); }
      sourceSettings.request();
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
    if (key === 'textureEnabled' && value && !renderStatus?.textureName) { input.checked = false; return; }
    const normalized = normalizeParameter(key, value);
    if (key === 'maxBounces' && normalized > bounceLimit(scene.parameters)) throw new Error('数值超出范围');
    if (key === 'skyModel') {
      if (switchSkyModel(scene.parameters, normalized)) document.querySelectorAll('[data-key="skyIntensity"]').forEach(peer => { peer.value = scene.parameters.skyIntensity; });
    } else if (key === 'rampInterpolation') {
      rampWorkbench.manualEdit();
      const previous = rampStops(scene.parameters);
      scene.parameters[key] = normalized;
      if (scene.parameters.colorRamp === 'custom') scene.parameters.rampStops = resampleRamp(previous, previous.length, normalized);
    } else scene.parameters[key] = normalized;
    if (key === 'cloudSeed') delete scene.parameters.transitionCloudSeeds;
    if (key === 'rampColors') scene.parameters.colorRamp = 'custom';
    if ((key === 'colorRamp' || key === 'rampInterpolation') && scene.parameters.colorRamp !== 'custom') { scene.parameters.rampColors = rampPalette(scene.parameters); scene.parameters.rampStops = rampStops(scene.parameters); }
    input.setCustomValidity('');
    document.querySelectorAll(`[data-key="${key}"]`).forEach(peer => {
      if (peer !== input && peer.dataset.scope !== 'export') {
        if (field.type === 'boolean') peer.checked = scene.parameters[key];
        else peer.value = field.type === 'color5' ? scene.parameters[key][Number(peer.dataset.colorIndex)] : scene.parameters[key];
      }
    });
    if (key === 'colorRamp' || key === 'rampColors' || key === 'rampInterpolation') { render(); overlaysChanged(); }
    if (key === 'stretchType') { render(); overlaysChanged(); }
    queueParameterUpdate();
  } catch { input.setCustomValidity('数值无效'); }
});

document.addEventListener('change', event => {
  if (event.target.dataset.key) render();
  if (event.target.dataset.key || event.target.id?.startsWith('export-')) sourceSettings.request();
});
function selectTab(button) {
  if (renderStatus?.exporting || button.disabled) return;
  const tablist = button.closest('[role="tablist"]');
  tablist.querySelectorAll('[role="tab"]').forEach(item => {
    item.setAttribute('aria-selected', String(item === button));
    item.tabIndex = item === button ? 0 : -1;
    document.getElementById(item.getAttribute('aria-controls')).hidden = item !== button;
  });
  if(button.id==='tab-views')namedViews?.refresh();
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
function backgroundOpen(visible) { document.querySelector('#background').hidden=!visible;document.querySelector('#background-toggle').setAttribute('aria-expanded',String(visible));overlaysChanged(); }
document.querySelector('#background-toggle').addEventListener('click',()=>{cameraOpen(false);backgroundOpen(document.querySelector('#background').hidden);});
document.addEventListener('pointerdown',event=>{if(!event.target.closest('#background,#background-toggle') && !comboUI?.containsOwnedPopup(event.target,document.querySelector('#background')) && !colorPicker?.containsOwnedPopup(event.target,document.querySelector('#background')))backgroundOpen(false);});
document.addEventListener('keydown',event=>{if(event.key==='Escape')backgroundOpen(false);});
document.querySelector('#camera-toggle').addEventListener('click', () => cameraOpen(document.querySelector('#camera').hidden));
document.addEventListener('click', event => {
  if (!event.target.closest('#camera,#camera-toggle')) cameraOpen(false);
});
document.addEventListener('keydown', event => {
  if (acceptsExperimentalShortcut(event) && toggleBounceTestMode()) event.preventDefault();
}, { capture: true });

document.addEventListener('keydown', event => {
  if (event.key === 'Escape') { cameraOpen(false); if(scene.parameters.appearancePick){scene.parameters.appearancePick=false;drawRampEditor();queueParameterUpdate();} }
  if (event.key === 'Tab' && event.target === document.body && !document.querySelector('dialog[open]')) { event.preventDefault(); togglePanels(); }
});

let bounceNoticeTimer = null;
function toggleBounceTestMode() {
  if (nativeDialogOpen || removingTexture || renderStatus?.exporting || document.querySelector('dialog[open]')) return false;
  const active = toggleExperimentalBounces(scene.parameters);
  const notice = document.querySelector('#bounce-test-notice');
  clearTimeout(bounceNoticeTimer);
  notice.textContent = active ? '反弹测试模式 · 上限 100' : '反弹测试模式已关闭 · 上限 15';
  notice.hidden = false;
  if (!active) bounceNoticeTimer = setTimeout(() => { notice.hidden = true; overlaysChanged(); }, 4000);
  render(); queueParameterUpdate(); overlaysChanged();
  return true;
}
document.querySelector('#show-output').addEventListener('click', () => {
  const dialog = document.querySelector('#export-dialog');
  if (!dialog.open) {
    const rect=document.querySelector('#viewport').getBoundingClientRect();
    if(exportFrameState?.ratio)exportFrameState=fitFrame(exportFrameState,rect.width,rect.height);
    const crop=exportFrameState?.crop;
    const fullViewport={width:rect.width,height:rect.height};
    exportViewport={width:rect.width*(crop?.width??1),height:rect.height*(crop?.height??1)};
    exportSceneSnapshot=freezeCurrentView(rendererParams(),fullViewport,crop);
    exportSourceStatus=cloneScene(renderStatus ?? {});
    if (!presetStretchScene || stretchKeys.some(key => scene.parameters[key] !== presetStretchScene[key])) {
      for (const key of stretchKeys) exportSettings[key] = scene.parameters[key];
      presetStretchScene = null;
    }
    resetExportResult(); setExportFormat(true);
    render(); dialog.showModal(); overlaysChanged();
  }
  void refreshExportPreview();
});
if (matchMedia('(max-width:800px)').matches) panelVisible('map-panel', false);
const appWindow = window.__TAURI__?.window?.getCurrentWindow();
if (appWindow) void flushSettingsOnClose(appWindow,()=>sourceSettings.flush(),error=>console.warn('设置快照',error)).catch(error=>console.warn('设置快照',error));
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
const resetEffects=document.createElement('button');resetEffects.id='reset-effects';resetEffects.className='wide';resetEffects.textContent='恢复默认';
resetEffects.addEventListener('click',()=>{if(renderStatus?.exporting)return;for(const key of groups.filter(([target])=>target==='#effects').flatMap(([, , keys])=>keys))scene.parameters[key]=fields[key].value;delete scene.parameters.transitionCloudSeeds;render();queueParameterUpdate();});
document.querySelector('#effects').append(resetEffects);
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
// Presets contain validated render settings only; assets remain owned by the renderer.
var presetSelectionSignature = null;
function reconcilePresetSelection() {
  const select=document.querySelector('#render-preset');
  if (select.value && presetSelectionSignature !== null && presetSelectionSignature !== JSON.stringify(currentPresetSettings())) {
    select.value=''; presetSelectionSignature=null; syncComboboxes(); overlaysChanged();
  }
}
const presetStore = createPresetStore({getItem:key=>window.localStorage.getItem(key),setItem:(key,value)=>window.localStorage.setItem(key,value)},Date.now,builtinPresets);
function refreshRenderPresets(selected) {
  const select=document.querySelector('#render-preset');
  const items=presetStore.list();
  select.replaceChildren();
  const empty=document.createElement('option'); empty.value=''; empty.textContent=''; select.append(empty);
  for(const entry of items) {const option=document.createElement('option');option.value=entry.name;option.textContent=entry.name;select.append(option);}
  select.value=selected ?? '';
  presetSelectionSignature=select.value ? JSON.stringify(currentPresetSettings()) : null;
  document.querySelector('#remove-preset').hidden=!!presetStore.isBuiltin(select.value);
  syncComboboxes(); overlaysChanged();
}
function currentPresetSettings() {
  return capturePresetSettings(scene.parameters);
}
function currentSourceSettings() {
  return {parameters:scene.parameters,output:exportSettings,format:document.querySelector('#export-format').value,
    framing:document.querySelector('#export-framing').value,scale:document.querySelector('#export-scale').value};
}
function installRenderPreset(settings) {
  for(const key of ['transitionCloudSeeds','_transitionHeight','transitionBackgroundFrom','transitionBackgroundMix','transitionSkyFollow','transitionSkyModel'])delete scene.parameters[key];
  applyPresetSettings(scene.parameters,exportSettings,settings,!!renderStatus?.textureName);
  presetStretchScene = null;
  exportSceneSnapshot=null; exportViewport=null; exportSourceStatus=null;
  render(); resetExportResult(); queueParameterUpdate(); overlaysChanged();
}
function installSourceSnapshot(settings) {
  for(const key of ['transitionCloudSeeds','_transitionHeight','transitionBackgroundFrom','transitionBackgroundMix','transitionSkyFollow','transitionSkyModel'])delete scene.parameters[key];
  const applied=applySnapshotSettings(scene.parameters,exportSettings,settings,!!renderStatus?.textureName);
  presetStretchScene = Object.fromEntries(stretchKeys.map(key => [key, scene.parameters[key]]));
  document.querySelector('#export-format').value=applied.format;
  document.querySelector('#export-framing').value=applied.framing;
  document.querySelector('#export-scale').value=applied.scale;
  document.querySelector('#export-depth').value=String(exportSettings.outputBitDepth);
  document.querySelector('#export-terrain-scale').value=exportSettings.exportTerrainScale;
  document.querySelector('#export-extent').value=exportSettings.exportShadow?'shadow':'source';
  document.querySelector('#shadow-options').hidden=!exportSettings.exportShadow;
  exportSceneSnapshot=null; exportViewport=null; exportSourceStatus=null;
  render(); resetExportResult(); setExportFormat(true); queueParameterUpdate(); overlaysChanged();
}

function closePresetEditor(restoreFocus = false) {
  document.querySelector('#preset-editor').hidden=true;
  document.querySelector('#save-preset').setAttribute('aria-expanded','false');
  document.querySelector('#preset-name').setCustomValidity('');
  if (restoreFocus) document.querySelector('#save-preset').focus();
  overlaysChanged();
}
function saveNamedPreset() {
  if(renderStatus?.exporting || removingTexture) return;
  const input=document.querySelector('#preset-name');
  try {const entry=presetStore.save(input.value,currentPresetSettings());input.setCustomValidity('');refreshRenderPresets(entry.name);closePresetEditor(true);}
  catch(error) {input.setCustomValidity(String(error.message));input.reportValidity();}
}
document.querySelector('#save-preset').addEventListener('click',()=>{
  if(renderStatus?.exporting || removingTexture) return;
  const editor=document.querySelector('#preset-editor');
  if(!editor.hidden){closePresetEditor(true);return;}
  const input=document.querySelector('#preset-name');
  input.value=document.querySelector('#render-preset').value;
  input.setCustomValidity('');editor.hidden=false;
  document.querySelector('#save-preset').setAttribute('aria-expanded','true');
  input.focus();input.select();overlaysChanged();
});
document.querySelector('#confirm-preset').addEventListener('click',saveNamedPreset);
document.querySelector('#cancel-preset-save').addEventListener('click',()=>closePresetEditor(true));
document.querySelector('#preset-name').addEventListener('input',event=>event.target.setCustomValidity(''));
document.querySelector('#preset-name').addEventListener('keydown',event=>{
  if(event.key==='Enter'){event.preventDefault();saveNamedPreset();}
  else if(event.key==='Escape'){event.preventDefault();event.stopPropagation();closePresetEditor(true);}
});
document.querySelector('#render-preset').addEventListener('change',()=>{document.querySelector('#remove-preset').hidden=!!presetStore.isBuiltin(document.querySelector('#render-preset').value);presetSelectionSignature=document.querySelector('#render-preset').value ? JSON.stringify(currentPresetSettings()) : null;closePresetEditor();});
document.querySelector('#apply-preset').addEventListener('click',()=>{
  if(renderStatus?.exporting || removingTexture) return;
  try {const name=document.querySelector('#render-preset').value;const settings=presetStore.use(name);if(settings){installRenderPreset(settings);refreshRenderPresets(name);}}
  catch(error) {void showError(error);}
});
document.querySelector('#remove-preset').addEventListener('click',()=>{
  if(renderStatus?.exporting || removingTexture) return;
  try {presetStore.remove(document.querySelector('#render-preset').value);refreshRenderPresets();}
  catch(error) {void showError(error);}
});
document.querySelector('#reset-render').addEventListener('click',()=>{
  if(renderStatus?.exporting || removingTexture) return;
  refreshRenderPresets('');
  installRenderPreset(defaultPresetSettings());
});
refreshRenderPresets();

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
function clearManualTransition(keys){
  if(keys.includes('cloudSeed'))delete scene.parameters.transitionCloudSeeds;
  if(keys.some(key=>['heightScale','heightScaleMode'].includes(key)))delete scene.parameters._transitionHeight;
  if(keys.some(key=>key.startsWith('background')||key.startsWith('sky')))for(const key of ['transitionBackgroundFrom','transitionBackgroundMix','transitionSkyFollow','transitionSkyModel'])delete scene.parameters[key];
}
const contextActions=installContextActions({
  scene,fields,getStatus:()=>renderStatus,locked:appearanceLocked,invoke,
  rampEditor,selectStandardView,onChange:overlaysChanged,onError:error=>void showError(error),
  patch(values) {
    if(appearanceLocked())return;
    appearanceHistory?.end();
    clearManualTransition(Object.keys(values));
    for(const [key,value] of Object.entries(values))scene.parameters[key]=normalizeParameter(key,structuredClone(value));
    render();queueParameterUpdate();appearanceHistory?.end();
  },
  reset(keys) {
    if(appearanceLocked())return;
    appearanceHistory?.end();
    clearManualTransition(keys);
    for(const key of keys)if(fields[key])scene.parameters[key]=structuredClone(fields[key].value);
    render();queueParameterUpdate();appearanceHistory?.end();
  },
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
  contextActions.close();
  orbitPointer = { id: event.pointerId, x: event.clientX, y: event.clientY, startX:event.clientX, startY:event.clientY, dragged:false, shifted:event.shiftKey };
  viewport.setPointerCapture(event.pointerId); event.preventDefault();
});
viewport.addEventListener('pointermove', event => {
  if (orbitPointer?.id !== event.pointerId) return;
  if (!(event.buttons & 2)) { finishOrbit(event); return; }
  orbitPointer.shifted ||= event.shiftKey;
  if(!orbitPointer.dragged && Math.hypot(event.clientX-orbitPointer.startX,event.clientY-orbitPointer.startY)<4)return;
  orbitPointer.dragged=true;
  const moveCamera = event.shiftKey ? panCamera : orbitCamera;
  moveCamera(event.clientX - orbitPointer.x, event.clientY - orbitPointer.y);
  orbitPointer.x = event.clientX; orbitPointer.y = event.clientY;
});
function finishOrbit(event) {
  if (orbitPointer?.id !== event.pointerId) return;
  const clicked=event.type==='pointerup'&&!orbitPointer.dragged&&!orbitPointer.shifted&&!event.shiftKey;
  const point={x:event.clientX,y:event.clientY};
  orbitPointer = null;
  if(clicked)contextActions.openViewport(point.x,point.y);
  if (viewport.hasPointerCapture(event.pointerId)) viewport.releasePointerCapture(event.pointerId);
}
for (const name of ['pointerup', 'pointercancel', 'lostpointercapture']) viewport.addEventListener(name, finishOrbit);
viewport.addEventListener('wheel', event => {
  event.preventDefault();
  const pixels = event.deltaY * (event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? viewport.clientHeight : 1);
  zoomCamera(-pixels / 100);
}, { passive: false });

palettePanel = createPalettePanel({
  select:document.querySelector('[data-key="colorRamp"]'),getParameters:()=>scene.parameters,
  invoke:(command,args)=>invoke ? invoke(command,args) : Promise.resolve(args?.defaults),
  isLocked:appearanceLocked,onOverlayChange:overlaysChanged,onError:error=>void showError(error),
  onApply:stops=>{
    rampWorkbench.manualEdit();
    commitRampPatch({colorRamp:'custom',rampStops:stops,rampColors:resampleRamp(stops,5).map(stop=>stop.color)});
  },
});
colorPicker = createColorPicker({onOverlayChange:overlaysChanged,isLocked:appearanceLocked});
render();
function currentViewRecord() {
  return {parameters:Object.fromEntries(viewParameterKeys.map(key=>[key,cloneScene(scene.parameters[key])])),frame:cloneScene(exportFrameUI?.isOpen()?exportFrameUI.getFrame():exportFrameState),settings:captureSceneSettings(scene.parameters)};
}
let transitionView=null;
viewTransition=createSceneTransition({
  duration:sceneTransitionDuration,maxFrameDelta:50,speed:()=>namedViews?.getSpeed()??1,
  read:()=>cloneScene(scene.parameters),
  onCancel:()=>{render();sourceSettings.request();},
  write:parameters=>{
    if(renderStatus?.exporting||nativeDialogOpen){viewTransition.cancel();return;}
    for(const key of ['transitionCloudSeeds','_transitionHeight','transitionBackgroundFrom','transitionBackgroundMix','transitionSkyFollow','transitionSkyModel'])delete scene.parameters[key];
    Object.assign(scene.parameters,parameters);
    renderCameraAxis();
    if(rendererReady)parameterUpdates.request(rendererParams());
  },
  finish:()=>{
    exportFrameState=cloneScene(transitionView.frame);
    if(exportFrameUI?.isOpen())exportFrameUI.open(exportFrameState);
    render();queueParameterUpdate();overlaysChanged();
  }
});
function applyViewRecord(view) {
  transitionView=cloneScene(view);viewTransition.start(mergeSceneView(scene.parameters,view));
}
// Capture phase cancels before direct input changes camera or starts another action.
for(const eventName of ['pointerdown','wheel','keydown','input','change'])document.addEventListener(eventName,event=>{if(!event.target.closest?.('[data-view-speed]'))viewTransition?.cancel();},{capture:true});

function reopenFramedExport() {
  document.body.classList.remove('is-export-framing');
  document.querySelector('#show-output').click();
  document.querySelector('#export-framing').value='view';setExportFormat(true);scheduleExportPreview();
}
exportFrameUI=createExportFrame({document,isLocked:()=>!!renderStatus?.exporting,
  onOverlayChange:overlaysChanged,
  onConfirm(frame){exportFrameState=cloneScene(frame);framingBackup=null;reopenFramedExport();},
  onCancel(){if(framingBackup){Object.assign(scene.parameters,cloneScene(framingBackup.parameters));exportFrameState=cloneScene(framingBackup.frame);framingBackup=null;render();queueParameterUpdate();}reopenFramedExport();}
});
namedViews=createNamedViews({document,storage:window.localStorage,
  getSourceKey:()=>renderStatus?.ready?(mcpImportedSourceKey??renderStatus.source?.path??(renderStatus.mapName==='内置地形'?'builtin':null)):null,
  getView:currentViewRecord,applyView:applyViewRecord,
  onOverlayChange:overlaysChanged,
  captureThumbnail:async(view)=>{
    await flushParameterUpdate();
    const bytes=await invoke('renderer_capture_png');
    const bitmap=await createImageBitmap(new Blob([new Uint8Array(bytes)],{type:'image/png'}));
    try{
      const crop=view.frame?.crop??{x:0,y:0,width:1,height:1};
      const w=bitmap.width*crop.width,h=bitmap.height*crop.height;
      const canvas=document.createElement('canvas');canvas.width=Math.max(1,Math.round(240*Math.min(1,w/h)));canvas.height=Math.max(1,Math.round(240*Math.min(1,h/w)));
      canvas.getContext('2d').drawImage(bitmap,crop.x*bitmap.width,crop.y*bitmap.height,w,h,0,0,canvas.width,canvas.height);
      return canvas.toDataURL('image/jpeg',.75);
    }finally{bitmap.close();}
  },
  isLocked:()=>!!renderStatus?.exporting||nativeDialogOpen,onError:error=>void showError(error)
});

document.querySelector('#select-export-frame').addEventListener('click',()=>{
  if(renderStatus?.exporting||previewBusy)return;
  framingBackup=currentViewRecord();namedViews.close();cameraOpen(false);backgroundOpen(false);
  document.querySelector('#export-dialog').close();document.body.classList.add('is-export-framing');exportFrameUI.open(exportFrameState);overlaysChanged();
});

videoRendering=createVideoRendering({
  invoke:(command,args)=>invoke(command,args),
  getBase:()=>cloneScene(scene.parameters),
  getViewport:()=>{const r=viewport.getBoundingClientRect();return {width:r.width,height:r.height};},
  getTerrainScale:()=>exportSettings.exportTerrainScale,
  onPreview:url=>videoWorkflow.setPreview(url),
  onProgress:value=>videoWorkflow.setProgress(value),
  onBusy:value=>{videoBusy=value;nativeDialogOpen=value;exportCommandPending=value;if(value)applyStatus(renderStatus);},
  onComplete:async(path)=>{if(path){exportedPreview=true;document.querySelector('#open-export-file').hidden=false;document.querySelector('#open-export-directory').hidden=false;}},
  onFinish:async()=>{videoBusy=false;nativeDialogOpen=false;exportCommandPending=false;videoWorkflow.finish();applyStatus(await invoke('renderer_status'));restoreAppearanceControls();overlaysChanged();},
  flush:flushParameterUpdate,onError:error=>{videoWorkflow?.stopPreview();void showError(error);}
});
videoWorkflow=createVideoWorkflow({document,getViews:()=>namedViews.list(),getCurrentView:currentViewRecord,
  onPreview:request=>videoRendering.preview(request),onExport:request=>videoRendering.export(request),
  onCancel:()=>void invoke('renderer_cancel_export').catch(showError),onOverlayChange:overlaysChanged,onError:error=>void showError(error)
});

const layoutEditor=installLayoutEditor({document,
  getContext:()=>({sourceKey:mcpImportedSourceKey??renderStatus?.source?.path??'builtin',title:renderStatus?.mapName??'地形',ramp:layoutRamp(scene.parameters,appearanceRange()),legendLabels:(appearanceRange()??[]).map(v=>Number(v).toLocaleString('zh-CN',{maximumFractionDigits:2})),legendDiscrete:scene.parameters.rampInterpolation==='discrete'}),
  isLocked:()=>!!renderStatus?.exporting||nativeDialogOpen||!renderStatus?.ready,
  onError:error=>void showError(error),onOverlayChange:overlaysChanged,
  captureImage:async({background=true}={})=>{
    await flushParameterUpdate();
    if(!background){
      const r=viewport.getBoundingClientRect(),frame=exportFrameState?fitFrame(exportFrameState,r.width,r.height):null,crop=frame?.crop;
      const p=freezeCurrentView(rendererParams(),{width:r.width,height:r.height},crop);
      const width=r.width*(crop?.width??1),height=r.height*(crop?.height??1),scale=Math.min(window.devicePixelRatio||1,1920/Math.max(width,height));
      Object.assign(p,{width:Math.max(2,Math.round(width*scale)),height:Math.max(2,Math.round(height*scale)),samples:Math.max(64,Math.min(256,renderStatus?.targetSamples??256)),exportTerrainScale:'source'});
      const bytes=await invoke('renderer_layout_capture',{params:p});
      const blob=new Blob([new Uint8Array(bytes)],{type:'image/png'});
      return await new Promise((resolve,reject)=>{const reader=new FileReader();reader.onload=()=>resolve(reader.result);reader.onerror=()=>reject(new Error('地形图像读取失败'));reader.readAsDataURL(blob);});
    }
    const bytes=await invoke('renderer_capture_png');
    const bitmap=await createImageBitmap(new Blob([new Uint8Array(bytes)],{type:'image/png'}));
    try{const crop=exportFrameState?fitFrame(exportFrameState,bitmap.width,bitmap.height).crop:{x:0,y:0,width:1,height:1};
      const canvas=document.createElement('canvas');canvas.width=Math.max(1,Math.round(bitmap.width*crop.width));canvas.height=Math.max(1,Math.round(bitmap.height*crop.height));
      canvas.getContext('2d').drawImage(bitmap,crop.x*bitmap.width,crop.y*bitmap.height,bitmap.width*crop.width,bitmap.height*crop.height,0,0,canvas.width,canvas.height);
      return canvas.toDataURL('image/png');
    }finally{bitmap.close();}
  },
  exportImage:async(data,filename)=>{nativeDialogOpen=true;try{const bytes=Array.from(Uint8Array.from(atob(data.split(',')[1]),c=>c.charCodeAt(0)));await invoke('renderer_export_layout',{bytes,filename});}finally{nativeDialogOpen=false;}}
});
overlayObserver.observe(document.querySelector('#layout-dialog'));
document.querySelector('#show-layout').addEventListener('click',()=>{viewTransition?.cancel();cameraOpen(false);backgroundOpen(false);void layoutEditor.open().catch(showError);});

comboUI = enhanceSelects(document, overlaysChanged);

// Runtime control uses the same scene and output objects as the visible controls.
const mcpViewStore=createNamedViewsStore(window.localStorage);
mcpHud=createMcpHudControl({onChange:hidden=>{
  document.body.classList.toggle('mcp-clean-hud',hidden);
  overlaysChanged();
  void resizeViewport().catch(error=>console.error(error));
}});
document.addEventListener('keydown',event=>{if(event.key==='Escape')mcpHud.end('user');},{capture:true});
window.addEventListener('pagehide',()=>mcpHud.end('pagehide'));
let mcpRevision=0,mcpProgressTimer=null;
const mcpSource=()=>mcpImportedSourceKey??renderStatus?.source?.path??'builtin';
function mcpEditable(){if(!rendererReady||nativeDialogOpen||renderStatus?.exporting||mcpJobs.active)throw new Error('应用正在处理任务，请查询任务状态');}
const mcpJobs=createControlJobs({onChange:(busy,kind,job)=>{
  if(!busy&&['failed','cancelled'].includes(job?.state))mcpHud.end('job-error');
  nativeDialogOpen=busy;exportCommandPending=busy&&kind.startsWith('export');
  if(mcpProgressTimer!==null){clearInterval(mcpProgressTimer);mcpProgressTimer=null;}
  if(busy&&kind==='export_video'){videoBusy=true;videoWorkflow.setProgress(0);mcpProgressTimer=setInterval(async()=>{try{const p=await invoke('renderer_video_progress');if(mcpJobs.active&&p?.total)videoWorkflow.setProgress(p.completed/p.total*100);}catch{}},300);}
  if(!busy&&kind==='export_video'){videoBusy=false;videoWorkflow.finish();}
  if(!busy){document.querySelector('#import-dialog').close();restoreAppearanceControls();void invoke('renderer_status').then(applyStatus).catch(()=>{});}
  updateExportProgress(renderStatus);overlaysChanged();
}});
const mcpFields=()=>Object.fromEntries(Object.entries(fields).map(([key,value])=>[key,cloneScene(value)]));
const mcpScene=()=>({sourceKey:mcpSource(),revision:mcpRevision,parameters:cloneScene(Object.fromEntries(Object.keys(fields).map(key=>[key,scene.parameters[key]]))),nativeParameters:cloneScene(rendererParams()),frame:cloneScene(exportFrameState),fields:mcpFields(),status:cloneScene(renderStatus)});
const mcpOutput=()=>({parameters:cloneScene(exportSettings),format:document.querySelector('#export-format').value,type:document.querySelector('#export-type').value});
function mcpParams(parameters){return {...parameters,contourEnabled:false,...nativeHeightScaling(parameters),cameraPan:[...parameters.cameraPan],...nativeRamp(parameters),surface:parameters.surface==='polygon'?'poly':parameters.surface};}
async function mcpPatchScene(parameters,transition){
  mcpEditable();const previous=cloneScene(scene.parameters),patch=validateControlPatch(parameters),next=mergeControlParameters(scene.parameters,patch,transition);
  if((patch.rampColors||patch.rampStops)&&patch.colorRamp===undefined)next.colorRamp='custom';
  if((patch.colorRamp||patch.rampInterpolation)&&next.colorRamp!=='custom'){next.rampColors=rampPalette(next);next.rampStops=rampStops(next);}
  else if(patch.rampInterpolation&&next.colorRamp==='custom'&&!patch.rampStops)next.rampStops=resampleRamp(rampStops(scene.parameters),rampStops(scene.parameters).length,next.rampInterpolation);
  await invoke('mcp_validate_params',{params:mcpParams(next),output:false});
  viewTransition.cancel();for(const key of controlTransitionKeys)delete scene.parameters[key];Object.assign(scene.parameters,next);
  render();queueParameterUpdate();await flushParameterUpdate();applyStatus(await invoke('renderer_status'));mcpRevision++;return {sourceKey:mcpSource(),revision:mcpRevision,parameters:cloneScene(Object.fromEntries(Object.keys(fields).filter(key=>JSON.stringify(next[key])!==JSON.stringify(previous[key])).map(key=>[key,scene.parameters[key]])))};
}
async function mcpPatchOutput(parameters){
  mcpEditable();const patch=validateControlPatch(parameters,Object.keys(exportSettings)),next={...exportSettings,...patch};
  await invoke('mcp_validate_params',{params:exportRequest(rendererParams(),next).params,output:true});
  Object.assign(exportSettings,next);document.querySelector('#export-scale').value='custom';document.querySelector('#export-depth').value=String(next.outputBitDepth);document.querySelector('#export-terrain-scale').value=next.exportTerrainScale;
  document.querySelector('#export-framing').value=next.exportOrthographic?'source':'view';document.querySelector('#export-extent').value=next.exportShadow?'shadow':'source';document.querySelector('#shadow-options').hidden=!next.exportShadow;
  exportSceneSnapshot=null;render();updateExportDimensions();sourceSettings.request();mcpRevision++;return mcpOutput();
}
function mcpSnapshot(){return {sourceKey:mcpSource(),parameters:mcpScene().parameters,output:cloneScene(exportSettings),frame:cloneScene(exportFrameState),transition:controlTransitions(Object.fromEntries(controlTransitionKeys.filter(key=>scene.parameters[key]!==undefined).map(key=>[key,scene.parameters[key]])))};}
function mcpStart(kind,run,cancel){mcpEditable();viewTransition.cancel();videoWorkflow.stopPreview();videoRendering.stopPreview();return mcpJobs.start(kind,run,cancel);}
async function mcpImport(payload,kind){
  if(typeof payload.path!=='string'||!payload.path)throw new Error('缺少导入路径');
  return mcpStart(`import_${kind}`,async()=>{
    await flushParameterUpdate();if(kind==='dem')await sourceSettings.suspend();
    try{
      const result=await invoke('mcp_import_path',{path:payload.path,kind});applyStatus(result);
      if(kind==='dem'){
        mcpImportedSourceKey=result.source?.path??payload.path;
        if(Number.isFinite(result.initialHeightScale)){scene.parameters.heightScale=normalizeParameter('heightScale',result.initialHeightScale*5);scene.parameters.heightScaleMode='linear';}
        await sourceSettings.open(mcpImportedSourceKey,{restore:payload.restoreSettings===true});
        appearanceDistribution=null;rampWorkbench.manualEdit();await refreshAppearanceDistribution();appearanceHistory?.clear();namedViews.refresh();
      }else{textureAsset=result.textureAsset??null;scene.parameters.appearanceMode='texture';scene.parameters.textureEnabled=true;}
      render();await flushParameterUpdate();sourceSettings.request();mcpRevision++;return {sourceKey:mcpSource(),status:await invoke('renderer_status')};
    }finally{if(kind==='dem')sourceSettings.resume();}
  });
}
async function mcpExport(payload){
  const kind=payload.kind??'image';if(!['image','mesh','geotiff','video'].includes(kind))throw new Error('导出类型无效');
  if(typeof payload.path!=='string'||!payload.path)throw new Error('缺少导出路径');
  const overrides={};for(const [from,to] of Object.entries({width:'width',height:'height',samples:'samples',transparentBackground:'exportTransparent',precision:'exportTerrainScale',bitDepth:'outputBitDepth'}))if(payload[from]!==undefined)overrides[to]=payload[from];
  if(Object.keys(overrides).length)await mcpPatchOutput(overrides);
  const format=payload.format??(kind==='mesh'?document.querySelector('#export-mesh-format').value:kind==='video'?'mp4':kind==='geotiff'?'tiff':document.querySelector('#export-format').value);
  const formats={image:['png','jpeg','jpg','bmp','tga','tiff','tif','geotiff'],mesh:['ply','obj'],geotiff:['tiff','tif','geotiff'],video:['mp4']};if(!formats[kind].includes(format))throw new Error('导出格式无效');
  document.querySelector('#export-type').value=kind==='geotiff'?'image':kind;
  if(kind==='mesh')document.querySelector('#export-mesh-format').value=format;
  else if(kind!=='video')document.querySelector('#export-format').value=['tiff','tif'].includes(format)?'geotiff':format==='jpg'?'jpeg':format;
  syncExportTypeControls();syncExportFormatControls();syncComboboxes();
  const rect=viewport.getBoundingClientRect(),frame=exportFrameState?fitFrame(exportFrameState,rect.width,rect.height).crop:null;
  const base=kind==='mesh'||exportSettings.exportOrthographic?rendererParams():freezeCurrentView(rendererParams(),rect,frame);
  const params=kind==='mesh'?{...base,exportTerrainScale:exportSettings.exportTerrainScale}:exportRequest(base,exportSettings,format).params;
  let frames=null,fps=null;
  if(kind==='video'){const settings=videoWorkflow.getSettings();frames=timelineFrames(videoWorkflow.getTimeline(),settings,cloneScene(scene.parameters),rect,exportSettings.exportTerrainScale);fps=settings.fps;}
  return mcpStart(`export_${kind}`,async()=>{await flushParameterUpdate();const path=await invoke('mcp_export_path',{path:payload.path,kind,format,params,frames,fps,bytes:null});return {path};},()=>invoke('renderer_cancel_export'));
}
const mcpHandlers={
  begin_control:async options=>{
    mcpEditable();
    if(document.querySelector('dialog[open]'))throw new Error('请先关闭对话框');
    const result=mcpHud.begin(options);await resizeViewport();return result;
  },
  end_control:async()=>{const result=mcpHud.end();await resizeViewport();return result;},
  get_control_state:()=>mcpHud.read(),
  fit_view:async(options)=>{
    mcpEditable();const status=await invoke('renderer_status');applyStatus(status);
    const parameters=fitTerrainView(scene.parameters,status,options);
    const result=await mcpPatchScene(parameters);return {...result,fit:{space:'terrain',region:options.region??{x:0,y:0,width:1,height:1},padding:options.padding??.08}};
  },
  focus_region:async(options)=>{
    mcpEditable();const parameters=focusViewportRegion(scene.parameters,options);
    const result=await mcpPatchScene(parameters);return {...result,focus:{space:'viewport',region:options.region,padding:options.padding??.08}};
  },
  inspect_scene:({keys,metadata=false,native=false})=>{
    if(!Array.isArray(keys)||!keys.length||keys.length>Object.keys(fields).length||keys.some(key=>!Object.hasOwn(fields,key)))throw new Error('查询字段无效');
    const parameters=cloneScene(Object.fromEntries(keys.map(key=>[key,scene.parameters[key]])));
    return {sourceKey:mcpSource(),revision:mcpRevision,parameters,
      ...(metadata?{fields:Object.fromEntries(keys.map(key=>[key,{...cloneScene(fields[key]),value:parameters[key]}]))}:{}),
      ...(native?{nativeParameters:cloneScene(Object.fromEntries(keys.map(key=>[key,rendererParams()[key]])))}:{})};
  },
  set_view_frame:({frame})=>{
    mcpEditable();const checked=validateNamedView({...currentViewRecord(),frame});
    exportFrameState=cloneScene(checked.frame);if(exportFrameUI.isOpen()){if(checked.frame)exportFrameUI.open(checked.frame);else exportFrameUI.close();}render();updateExportDimensions();sourceSettings.request();mcpRevision++;
    return {sourceKey:mcpSource(),revision:mcpRevision,frame:cloneScene(exportFrameState)};
  },
  design_views:({views})=>{
    mcpEditable();const entries=designControlViews(currentViewRecord(),views);
    const saved=mcpViewStore.saveMany(mcpSource(),entries);namedViews.refresh();mcpRevision++;
    return {sourceKey:mcpSource(),revision:mcpRevision,views:saved.map(row=>({name:row.name,view:row.view}))};
  },
  set_view_sequence:({frames,settings={}})=>{
    mcpEditable();const document=sequenceControlViews(namedViews.list(),frames,{...videoWorkflow.getSettings(),...settings});
    const result=videoWorkflow.setDocument(document);mcpRevision++;return {revision:mcpRevision,...result};
  },
  get_scene:()=>mcpScene(),patch_scene:p=>mcpPatchScene(p.parameters),
  get_export_settings:()=>mcpOutput(),patch_export_settings:p=>mcpPatchOutput(p.parameters),
  capture_snapshot:()=>mcpSnapshot(),
  restore_snapshot:async({snapshot})=>{
    mcpEditable();if(snapshot?.sourceKey!==mcpSource())throw new Error('快照属于其他DEM');
    const parameters=validateControlPatch(snapshot.parameters),output=validateControlPatch(snapshot.output,Object.keys(exportSettings)),transition=controlTransitions(snapshot.transition??{});
    const checked=validateNamedView({parameters:{...scene.parameters,...parameters},frame:snapshot.frame});
    await invoke('mcp_validate_params',{params:exportRequest(mcpParams({...scene.parameters,...parameters}),{...exportSettings,...output}).params,output:true});
    await mcpPatchScene(parameters,transition);await mcpPatchOutput(output);exportFrameState=checked.frame;return mcpSnapshot();
  },
  pause:async({paused})=>{mcpEditable();if(typeof paused!=='boolean')throw new Error('paused必须为布尔值');await invoke('renderer_pause',{paused});applyStatus(await invoke('renderer_status'));return renderStatus;},
  import_dem:p=>mcpImport(p,'dem'),import_texture:p=>mcpImport(p,'texture'),export:p=>mcpExport(p),
  job_status:async({jobId})=>{const job=mcpJobs.read(jobId);if(job.state==='running'){const status=await invoke('renderer_status');applyStatus(status);if(job.kind==='export_video'){const progress=await invoke('renderer_video_progress');if(progress?.total)mcpJobs.progress(jobId,progress.completed/progress.total*100);}else mcpJobs.progress(jobId,status.exportProgress);return {...mcpJobs.read(jobId),renderStatus:status};}return job;},
  cancel_job:({jobId})=>mcpJobs.cancel(jobId),
  list_views:()=>namedViews.list(),
  save_view:({name})=>{mcpEditable();const result=mcpViewStore.save(mcpSource(),name,currentViewRecord());namedViews.refresh();return result;},
  delete_view:({name})=>{mcpEditable();mcpViewStore.remove(mcpSource(),name);namedViews.refresh();return namedViews.list();},
  apply_view:async({name,animate=false})=>{mcpEditable();const row=namedViews.list().find(r=>r.name===name);if(!row)throw new Error('视图不存在');if(animate){applyViewRecord(row.view);return {transitioning:true,view:row};}const merged=mergeSceneView(scene.parameters,row.view);await mcpPatchScene(Object.fromEntries(Object.keys(fields).map(key=>[key,merged[key]])),controlTransitions(Object.fromEntries(controlTransitionKeys.filter(key=>merged[key]!==undefined).map(key=>[key,merged[key]]))));exportFrameState=cloneScene(row.view.frame);return mcpScene();},
  list_presets:()=>presetStore.list(),
  save_preset:({name})=>{mcpEditable();const result=presetStore.save(name,currentPresetSettings());refreshRenderPresets(name);return result;},
  apply_preset:async({name})=>{mcpEditable();if(name===''){refreshRenderPresets('');return mcpScene();}const settings=presetStore.use(name);if(!settings)throw new Error('预设不存在');installRenderPreset(settings);refreshRenderPresets(name);await flushParameterUpdate();return mcpScene();},
  get_layout:()=>layoutEditor.getLayout(),set_layout:({layout})=>{mcpEditable();return layoutEditor.setLayout(layout);},
  export_layout:({path})=>{if(typeof path!=='string'||!path)throw new Error('缺少导出路径');return mcpStart('export_layout',async()=>{const data=await layoutEditor.compose();const bytes=Array.from(Uint8Array.from(atob(data.split(',')[1]),c=>c.charCodeAt(0)));return {path:await invoke('mcp_export_path',{path,kind:'layout',format:'png',bytes,params:null,frames:null,fps:null})};});},
  get_video:()=>videoWorkflow.getDocument(),set_video:({video})=>{mcpEditable();return videoWorkflow.setDocument(video);},
};
if (invoke) {
  (async () => {
    document.querySelector('#startup-dialog').showModal();
    await new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)));
    let status = await invoke('renderer_init', { rect: viewportRect(), params: rendererParams() });
    const deadline = performance.now() + 30000;
    while (!status?.ready || !status.mapWidth || !status.width || !status.framePresented) {
      if (status?.error) throw new Error(status.error);
      if (performance.now() >= deadline) throw new Error('渲染初始化超时');
      await new Promise(resolve => setTimeout(resolve, 50));
      status = await invoke('renderer_status');
    }
    if (status.error) throw new Error(status.error);
    rendererReady = true;
    applyStatus(status);
    document.querySelector('#startup-dialog').close();
    await flushParameterUpdate();
    new ResizeObserver(() => void resizeViewport().catch(showError)).observe(document.querySelector('#viewport'));
    window.addEventListener('resize', () => void resizeViewport().catch(showError));
    document.addEventListener('visibilitychange', () => void resizeViewport().catch(showError));
    const listen = window.__TAURI__.event.listen;
    await attachRuntimeControl({listen,invoke,handlers:mcpHandlers,control:mcpHud});
    await listen('mcp-control-activity',()=>mcpHud.touch());
    await listen('viewport-restore-hud',()=>mcpHud.end('user'));
    await invoke('mcp_control_ready');
    await listen('renderer-import-started', () => {
      const dialog = document.querySelector('#import-dialog');
      if (nativeDialogOpen && !dialog.open) { dialog.showModal(); overlaysChanged(); }
    });
    await listen('viewport-pick', sampleAppearance);
    await listen('viewport-test-bounces', () => toggleBounceTestMode());
    await listen('viewport-context-menu',event=>{const scale=window.devicePixelRatio||1;contextActions.openViewport(event.payload.x/scale,event.payload.y/scale);});
    await listen('viewport-pick',()=>contextActions.close());
    await listen('viewport-orbit', event => { if (!document.querySelector('dialog[open]')) orbitCamera(event.payload.dx, event.payload.dy); });
    await listen('viewport-pan', event => { if (!document.querySelector('dialog[open]')) panCamera(...nativePanPixels(event.payload, window.devicePixelRatio)); });
    await listen('viewport-zoom', event => { if (!document.querySelector('dialog[open]')) zoomCamera(event.payload.delta); });
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
  })().catch(error => { document.querySelector('#startup-dialog').close();document.body.dataset.nativeReady = 'false'; void showError(error); });
}
