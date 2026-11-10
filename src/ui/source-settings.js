import { validatePresetSettings } from './render-presets.js';

export const sidecarSchema = 'demstudio.source-settings';
export const sidecarVersion = 1;
export function snapshotDocument(settings) {
  return {schema:sidecarSchema, version:sidecarVersion, settings:validatePresetSettings(settings)};
}
export function readSnapshotDocument(value) {
  if (value?.schema !== sidecarSchema || value.version !== sidecarVersion) throw new Error('设置快照版本无效');
  return validatePresetSettings(value.settings);
}

// Every job captures its source; import pauses writing before the native source changes.
export function createSourceSettings({invoke,getSettings,applySettings,onWarning=console.warn,setTimer=setTimeout,clearTimer=clearTimeout,delay=800}) {
  let path=null, timer=null, pending=null, writing=Promise.resolve(), suspended=false, generation=0, failed=false;
  function cancelTimer() {if(timer!==null)clearTimer(timer);timer=null;}
  function warn(error) {if(!failed)onWarning(error);failed=true;}
  function dispatch() {
    cancelTimer();
    const job=pending;pending=null;
    if(!job)return writing;
    writing=writing.then(async()=>{try{await invoke('source_settings_save',job);}catch(error){if(job.path===path)warn(error);}});
    return writing;
  }
  return {
    request() {
      if(!path || suspended || failed)return;
      try{pending={path,document:snapshotDocument(getSettings())};}catch(error){warn(error);return;}
      cancelTimer();timer=setTimer(dispatch,delay);
    },
    async flush() {await dispatch();},
    async suspend() {suspended=true;await dispatch();},
    resume() {suspended=false;},
    async open(nextPath) {
      const token=++generation;
      cancelTimer();pending=null;suspended=true;path=nextPath??null;failed=false;
      if(path) {
        try{const value=await invoke('source_settings_load',{path});if(token!==generation)return;if(value)applySettings(readSnapshotDocument(value));}
        catch(error){if(token===generation)warn(error);}
      }
      if(token!==generation)return;
      suspended=false;
      this.request();
    },
  };
}

export function flushSettingsOnClose(appWindow, flush, onWarning=console.warn) {
  let closing=false, allowClose=false;
  return appWindow.onCloseRequested(async event=>{
    if(allowClose)return;
    event.preventDefault();
    if(closing)return;
    closing=true;
    try{await flush();}catch(error){onWarning(error);}
    allowClose=true;
    try{await appWindow.close();}
    catch(error){allowClose=false;closing=false;onWarning(error);}
  });
}
