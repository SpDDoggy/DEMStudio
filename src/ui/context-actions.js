import {cameraPresets} from './camera-axis.js';
import {createContextMenu} from './context-menu.js';

export function installContextActions({scene, fields, getStatus, locked, invoke, patch, reset, rampEditor, selectStandardView, onChange, onError}) {
  const menu=createContextMenu({onChange,onError});
  const sep={separator:true};
  const click=selector=>()=>{const el=document.querySelector(selector);if(el&&!el.disabled)el.click();};
  const item=(label,action,disabled=locked())=>({label,action,disabled});
  const toggle=(label,key)=>({...item(label,()=>patch({[key]:!scene.parameters[key]})),checked:!!scene.parameters[key]});
  const resetKeys=keys=>()=>reset(keys);
  const hasTexture=()=>!!getStatus()?.textureName;
  const modes=()=>Object.entries({perspective:'透视',orthographic:'正交',isometric:'等轴'}).map(([value,label])=>({...item(label,()=>patch({projection:value})),checked:scene.parameters.projection===value}));
  const viewItems=()=>[
    item('适合窗口',resetKeys(['cameraDistance','cameraPan'])),item('重置视角',click('#reset-camera')),sep,
    {label:'视图',children:modes()},
    {label:'显示',children:[toggle('网格','grid'),toggle('地面','groundEnabled')]},sep,
    item('复制当前画面',()=>{
      const png=invoke('renderer_capture_png').then(bytes=>new Blob([new Uint8Array(bytes)],{type:'image/png'}));
      return navigator.clipboard.write([new ClipboardItem({'image/png':png})]);
    },locked()||!getStatus()?.ready||!navigator.clipboard?.write||!globalThis.ClipboardItem),
    item('导出…',click('#show-output')),
  ];
  function openViewport(x,y,target=document.querySelector('#viewport')) {
    if(document.querySelector('dialog[open]'))return;
    menu.open(viewItems(),{x,y,target});
  }
  const standardViews=()=>Object.entries(cameraPresets).map(([name,preset])=>item(`${preset.label}视图`,()=>selectStandardView(name)));
  function openStandardViews(x,y,target) { menu.open(standardViews(),{x,y,target}); }
  function textItems(target) {
    const start=target.selectionStart,end=target.selectionEnd;
    const focus=()=>{target.focus();if(start!==null)target.setSelectionRange(start,end);};
    const edit=command=>()=>{focus();document.execCommand(command);};
    const writable=!target.disabled&&!target.readOnly;
    const selected=start!==null&&end>start;
    return [item('撤销',edit('undo'),!writable),item('重做',edit('redo'),!writable),sep,
      item('剪切',edit('cut'),!writable||!selected),item('复制',edit('copy'),!selected),
      item('粘贴',async()=>{const text=await navigator.clipboard.readText();focus();document.execCommand('insertText',false,text);},!writable||!navigator.clipboard?.readText),sep,
      item('全选',()=>{target.focus();target.select();},target.disabled)];
  }
  function fieldItems(control) {
    const key=control.dataset.key;
    const set=value=>{
      if(locked()||control.disabled)return;
      if(control.type==='checkbox')control.checked=value;
      else control.value=String(value);
      control.dispatchEvent(new Event('input',{bubbles:true}));
      control.dispatchEvent(new Event('change',{bubbles:true}));
    };
    return [item('恢复默认值',()=>set(structuredClone(fields[key].value)),locked()||control.disabled),sep,
      item('复制数值',()=>navigator.clipboard.writeText(control.type==='checkbox'?String(control.checked):control.value),!navigator.clipboard?.writeText),
      item('粘贴数值',async()=>{
        const value=(await navigator.clipboard.readText()).trim();
        if(control.type==='checkbox'){if(!['true','false'].includes(value))throw Error('数值无效');set(value==='true');}
        else if(control.type==='number'||control.type==='range') {if(!value||!Number.isFinite(Number(value)))throw Error('数值无效');set(Number(value));}
        else {if(control.options&&![...control.options].some(option=>option.value===value))throw Error('选项无效');set(value);}
      },locked()||control.disabled||!navigator.clipboard?.readText)];
  }
  function route(target,x) {
    // Numeric/color/enum controls operate on their parameter. Free text keeps editing semantics.
    const parameter=target.closest('.field')?.querySelector('[data-key]');
    if(parameter&&fields[parameter.dataset.key])return fieldItems(parameter);
    if(target.matches('textarea,input:not([type]),input[type=text],input[type=search],input[type=url],input[type=email],input[type=password],input[type=tel]'))return textItems(target);
    const node=target.closest('[data-ramp-index]');
    if(node) {
      const index=Number(node.dataset.rampIndex),row=document.querySelectorAll('.ramp-class-row')[index];
      const discrete=scene.parameters.rampInterpolation==='discrete';
      return [item('修改颜色…',()=>row.querySelector('.ramp-swatch').click()),
        item(discrete?'修改边界…':'修改位置…',()=>row.querySelector('.ramp-class-value').click(),locked()||discrete&&index===0),sep,
        item(discrete?'删除分级':'删除色标',()=>row.querySelector('.ramp-delete').click(),locked()||!!row.querySelector('.ramp-delete').disabled)];
    }
    const track=target.closest('.ramp-track');
    if(track) {
      const bounds=track.getBoundingClientRect(),position=Math.max(0,Math.min(1,(x-bounds.left)/bounds.width));
      return [...(scene.parameters.rampInterpolation==='discrete'?[]:[item('在此添加色标',()=>rampEditor.addStop(position),locked()||document.querySelectorAll('.ramp-class-row').length>=16)]),
        toggle('反转色带','rampReverse'),item('均匀分布色标',()=>rampEditor.equalize())];
    }
    if(target.closest('.map-asset,#source-details'))return [item('更换高程图…',click('#import-heightmap')),sep,
      item('打开所在文件夹',()=>invoke('renderer_open_source',{kind:'height'}),!getStatus()?.source?.path),
      item('复制文件路径',()=>navigator.clipboard.writeText(getStatus().source.path),!getStatus()?.source?.path||!navigator.clipboard?.writeText)];
    if(target.closest('#texture-fields,.texture-asset'))return [item('更换贴图…',click('#import-texture')),
      item('定位贴图文件',()=>invoke('renderer_open_source',{kind:'texture'}),!hasTexture()),sep,
      {...toggle('启用贴图','textureEnabled'),disabled:locked()||!hasTexture()},
      item('恢复贴图参数',click('#texture-fields .appearance-reset')),item('移除贴图',click('#remove-texture'),locked()||!hasTexture())];
    if(target.closest('#material-fields'))return [toggle('启用材质','materialEnabled'),sep,item('恢复材质参数',click('#material-fields .appearance-reset'))];
    if(target.closest('#elevation-fields'))return [toggle('启用赋色','elevationEnabled'),toggle('反转色带','rampReverse'),sep,item('恢复赋色参数',click('#elevation-fields .appearance-reset'))];
    if(target.closest('#view-cube,#compact-view'))return standardViews();
    if(target.closest('[data-mode]'))return modes();
    if(target.closest('#toggle-grid,#background-toggle'))return [toggle('网格','grid'),toggle('地面','groundEnabled'),item('背景设置…',click('#background-toggle'))];
    const group=target.closest('details');
    if(group&&group.closest('#light,#effects,#grading,#background,#camera,#map-fields')) {
      const keys=[...new Set([...group.querySelectorAll('[data-key]')].map(el=>el.dataset.key))];
      return [...(group.closest('#effects')?[item('关闭全部效果',()=>patch({fogEnabled:false,cloudEnabled:false,cloudShadowEnabled:false,waterEnabled:false})),sep]:[]),item('恢复当前分组默认值',resetKeys(keys))];
    }
    if(target.closest('#effects'))return [item('关闭全部效果',()=>patch({fogEnabled:false,cloudEnabled:false,cloudShadowEnabled:false,waterEnabled:false})),item('恢复效果默认值',click('#reset-effects'))];
    if(target.closest('#grading'))return [item('恢复调色默认值',click('#reset-grading'))];
    if(target.closest('#light'))return [item('恢复光照默认值',resetKeys(Object.keys(fields).filter(key=>/^(sun|sky|maxBounces)/.test(key))))];
    return [];
  }
  document.addEventListener('contextmenu',event=>{
    event.preventDefault();
    if(event.target.closest('.app-context-menu'))return;
    menu.close(false);
    if(event.target.closest('#viewport'))return; // Viewport opens only on a completed click gesture.
    const rect=event.target.getBoundingClientRect();
    const x=event.clientX||rect.left,y=event.clientY||rect.bottom;
    const items=route(event.target,x);
    if(items.length)menu.open(items,{x,y,target:event.target});
  });
  document.addEventListener('keydown',event=>{
    if(event.key!=='ContextMenu'&&!(event.shiftKey&&event.key==='F10'))return;
    event.preventDefault();const target=document.activeElement,rect=target.getBoundingClientRect();
    if(target===document.body||target.id==='viewport')openViewport(rect.left+20,rect.top+20,target);
    else {const items=route(target,rect.left);if(items.length)menu.open(items,{x:rect.left,y:rect.bottom,target});}
  });
  return {openViewport,openStandardViews,close:()=>menu.close()};
}
