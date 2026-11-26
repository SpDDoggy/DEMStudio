const clone=value=>JSON.parse(JSON.stringify(value));
const rampKeys=['colorRamp','rampInterpolation','rampColors','rampStops','rampOffset','rampReverse','rampWrap','rampRangeMode','rampMin','rampMax','elevationBlend'];
const mix=(a,b,t)=>a+(b-a)*t;
export function transitionAppearance(parameters){
  if(parameters.appearanceLayersVersion===1)return parameters;
  return {...parameters,appearanceLayersVersion:1,materialEnabled:true,
    elevationEnabled:parameters.appearanceMode==='elevation',textureEnabled:parameters.appearanceMode==='texture',
    materialSpecular:parameters.appearanceMode==='material'?parameters.materialSpecular:0,
    // Legacy elevation always used the complete material color as its base.
    materialOpacity:parameters.appearanceMode==='elevation'?100:parameters.materialOpacity};
}

// Apply after ordinary ramp interpolation. Disabled layers have zero effective
// coverage; their stored opacity is retained only at the exact endpoint.
export function applyLayerTransition(from,to,t,result){
  if(t<=0)return clone(from);
  if(t>=1)return clone(to);
  const a=transitionAppearance(from),b=transitionAppearance(to);
  const enabledA=a.elevationEnabled===true,enabledB=b.elevationEnabled===true;
  if(!enabledA&&!enabledB)return result;
  result.appearanceLayersVersion=1;
  result.elevationEnabled=true;
  const opacityA=enabledA?(a.elevationOpacity??100):0;
  const opacityB=enabledB?(b.elevationOpacity??100):0;
  result.elevationOpacity=mix(opacityA,opacityB,t);
  if(enabledA!==enabledB){
    const visible=enabledA?a:b;
    for(const key of rampKeys)if(visible[key]!==undefined)result[key]=clone(visible[key]);
  }else if(a.elevationBlend!==b.elevationBlend){
    // The two blend operators cannot be interpolated through an enum. Meet at
    // zero coverage, where changing the operator has no visible discontinuity.
    const side=t<.5?a:b;
    result.elevationBlend=side.elevationBlend;
    result.elevationOpacity=t<.5?opacityA*(1-2*t):opacityB*(2*t-1);
  }
  return result;
}
