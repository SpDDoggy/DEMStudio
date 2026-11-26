const clone=value=>JSON.parse(JSON.stringify(value));
const keys=['backgroundMode','backgroundColor','backgroundColor2','backgroundAngle','backgroundFollowLight','skyFollowLight'];
function state(p){return {mode:p.backgroundMode??'sky',color:p.backgroundColor??'#30343b',color2:p.backgroundColor2??'#647180',angle:p.backgroundAngle??90,followLight:p.backgroundFollowLight??false,skyFollowLight:p.skyFollowLight??false,weight:1};}
function checked(s){
  if(!s||!['sky','solid','gradient'].includes(s.mode)||![s.color,s.color2].every(c=>/^#[\da-f]{6}$/i.test(c))||!Number.isFinite(s.angle)||typeof s.followLight!=='boolean'||typeof s.skyFollowLight!=='boolean'||!Number.isFinite(s.weight??1)||(s.weight??1)<0||s.components?.length)throw new Error('背景过渡数据无效');
  return {...state({backgroundMode:s.mode,backgroundColor:s.color,backgroundColor2:s.color2,backgroundAngle:s.angle,backgroundFollowLight:s.followLight,skyFollowLight:s.skyFollowLight}),weight:s.weight??1};
}
function merge(values){
  const map=new Map();
  for(const value of values){const s=checked(value);if(s.weight===0)continue;const {weight,...identity}=s,key=JSON.stringify(identity);const old=map.get(key);if(old)old.weight+=weight;else map.set(key,s);}
  if(map.size>32)throw new Error('背景过渡最多支持32种背景');
  const total=[...map.values()].reduce((sum,s)=>sum+s.weight,0);if(total<=0)throw new Error('背景过渡权重无效');
  return [...map.values()].map(s=>({...s,weight:s.weight/total}));
}
export function captureTransitionBackground(p){
  const sky={};for(const key of ['transitionSkyFollow','transitionSkyModel'])if(p?.[key]!=null){if(!Number.isFinite(p[key])||p[key]<0||p[key]>1)throw new Error('天空过渡比例无效');sky[key]=p[key];}
  if(!p?.transitionBackgroundFrom)return sky;
  const t=p.transitionBackgroundMix;if(!Number.isFinite(t)||t<0||t>1)throw new Error('背景过渡比例无效');
  const source=p.transitionBackgroundFrom;
  const components=merge(source.components?.length?source.components:[source]);
  return {...sky,transitionBackgroundFrom:{components},transitionBackgroundMix:t};
}
function visible(p){
  const active=captureTransitionBackground(p);if(!active.transitionBackgroundFrom)return [state(p)];
  const t=active.transitionBackgroundMix;
  return merge([...active.transitionBackgroundFrom.components.map(s=>({...s,weight:s.weight*(1-t)})),{...state(p),weight:t}]);
}
export function applyBackgroundTransition(from,to,t,result){
  for(const [key,field,on] of [['transitionSkyFollow','skyFollowLight',true],['transitionSkyModel','skyModel','atmosphere']]){const a=from[key]??(from[field]===on?1:0),b=to[key]??(to[field]===on?1:0);if(a!==b||from[key]!=null||to[key]!=null)result[key]=a+(b-a)*t;}
  if(!from.transitionBackgroundFrom&&!to.transitionBackgroundFrom&&keys.every(key=>from[key]===to[key]))return result;
  const components=merge([...visible(from).map(s=>({...s,weight:s.weight*(1-t)})),...visible(to).map(s=>({...s,weight:s.weight*t}))]);
  for(const key of keys)if(to[key]!==undefined)result[key]=clone(to[key]);
  result.transitionBackgroundFrom={components};result.transitionBackgroundMix=0;
  return result;
}
