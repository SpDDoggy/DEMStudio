// Cloud seeds identify density fields; blend fields rather than switching their labels.
export function validateCloudSeeds(values){
  if(!Array.isArray(values)||!values.length||values.length>32)throw new Error('云过渡数据无效');
  const merged=new Map();
  for(const value of values){
    if(!value||Object.keys(value).some(key=>!['seed','weight'].includes(key))||!Number.isInteger(value.seed)||value.seed<0||value.seed>9999||!Number.isFinite(value.weight)||value.weight<0||value.weight>1)throw new Error('云过渡数据无效');
    if(value.weight>0)merged.set(value.seed,(merged.get(value.seed)??0)+value.weight);
  }
  const total=[...merged.values()].reduce((a,b)=>a+b,0);
  if(total<=0)throw new Error('云过渡权重无效');
  return [...merged].map(([seed,weight])=>({seed,weight:weight/total}));
}
export function captureTransitionCloud(parameters){
  return parameters?.transitionCloudSeeds==null||(Array.isArray(parameters.transitionCloudSeeds)&&parameters.transitionCloudSeeds.length===0)?{}:{transitionCloudSeeds:validateCloudSeeds(parameters.transitionCloudSeeds)};
}
function visible(parameters){return captureTransitionCloud(parameters).transitionCloudSeeds??[{seed:parameters.cloudSeed??7,weight:1}];}
export function applyCloudTransition(from,to,t,result){
  if(from.cloudSeed===to.cloudSeed&&!from.transitionCloudSeeds&&!to.transitionCloudSeeds)return result;
  const values=[...visible(from).map(s=>({...s,weight:s.weight*(1-t)})),...visible(to).map(s=>({...s,weight:s.weight*t}))];
  // Endpoints are returned by interpolateSceneParameters before reaching here.
  const merged=new Map();for(const s of values)if(s.weight>0)merged.set(s.seed,(merged.get(s.seed)??0)+s.weight);
  let components=[...merged].map(([seed,weight])=>({seed,weight}));
  if(components.length>32)components=components.sort((a,b)=>b.weight-a.weight).slice(0,32);
  const total=components.reduce((sum,s)=>sum+s.weight,0);
  result.transitionCloudSeeds=validateCloudSeeds(components.map(s=>({...s,weight:s.weight/total})));
  return result;
}
