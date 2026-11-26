// Temporary MCP presentation state; never part of scene snapshots or presets.
export function createMcpHudControl({now=Date.now,schedule=setTimeout,cancel=clearTimeout,onChange=()=>{}}={}){
  let active=false,hidden=false,leaseSeconds=60,expiresAt=null,timer=null,reason='initial';
  const state=()=>({active,hudVisible:!hidden,leaseSeconds,expiresAt,reason});
  function end(nextReason='handoff'){
    if(timer!==null)cancel(timer);
    timer=null;active=false;hidden=false;expiresAt=null;reason=nextReason;onChange(false);
    return state();
  }
  function touch(){
    if(!active)return state();
    // A delayed request must not revive an expired hidden session.
    if(expiresAt!==null&&now()>=expiresAt)return end('lease-expired');
    if(timer!==null)cancel(timer);
    expiresAt=now()+leaseSeconds*1000;
    timer=schedule(()=>end('lease-expired'),leaseSeconds*1000);
    return state();
  }
  return {
    begin({hideHud=true,leaseSeconds:seconds=60}={}){
      if(typeof hideHud!=='boolean')throw new Error('hideHud必须为布尔值');
      if(!Number.isInteger(seconds)||seconds<5||seconds>300)throw new Error('leaseSeconds须为5到300的整数');
      if(timer!==null)cancel(timer);
      active=true;hidden=hideHud;leaseSeconds=seconds;expiresAt=null;reason='mcp';onChange(hidden);
      return touch();
    },
    end,touch,
    read(){if(active&&now()>=expiresAt)end('lease-expired');return state();},
  };
}
