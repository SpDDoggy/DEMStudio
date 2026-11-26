import {nativeHeightScaling} from './height-scaling.js';

const radians = value => value * Math.PI / 180;
const dot = (a,b) => a.reduce((sum,value,i)=>sum+value*b[i],0);
function rectangle(region={x:0,y:0,width:1,height:1}) {
  if (!region || !['x','y','width','height'].every(key=>Number.isFinite(region[key])) ||
      region.x<0 || region.y<0 || region.width<=0 || region.height<=0 ||
      region.x+region.width>1 || region.y+region.height>1) throw new Error('区域必须是图像范围内的归一化矩形');
  return region;
}
function margin(padding) {
  if (!Number.isFinite(padding) || padding<0 || padding>=.5) throw new Error('留白比例必须大于等于 0 且小于 0.5');
  return 1-2*padding;
}
function imageSize(parameters) {
  const {width,height}=parameters;
  if (![width,height].every(value=>Number.isFinite(value)&&value>0)) throw new Error('缺少有效的输出尺寸');
  return [width,height,Math.min(width,height)];
}

// Mirrors gl_renderer camera_target, camera_basis and native conservative height bounds.
// Source rectangles have their origin at the top left; native world +Y points upward.
export function fitTerrainView(parameters,status,{region,padding=.08,projection=parameters.projection}={}) {
  const rect=rectangle(region), inset=margin(padding);
  const width=status?.mapWidth, height=status?.mapHeight;
  if (![width,height].every(value=>Number.isFinite(value)&&value>0)) throw new Error('当前没有有效地形');
  if (!['perspective','orthographic','isometric'].includes(projection)) throw new Error('投影模式无效');
  const [outputWidth,outputHeight,shortest]=imageSize(parameters);
  const dim=Math.max(width,height), native=nativeHeightScaling(parameters);
  const low=(parameters.heightOffset??0)*.01*dim;
  const high=low+.2*dim*native.heightScale+(parameters.heightStep??0)*native.heightScale+.01;
  const center=[(rect.x+rect.width/2)*width,(1-rect.y-rect.height/2)*height,(low+high)/2];
  const yaw=radians(parameters.cameraYaw), pitch=radians(projection==='isometric'?35.26439:Math.max(0,Math.min(90,parameters.cameraPitch)));
  const right=[-Math.sin(yaw),Math.cos(yaw),0];
  const up=[-Math.cos(yaw)*Math.sin(pitch),-Math.sin(yaw)*Math.sin(pitch),Math.cos(pitch)];
  const back=[Math.cos(yaw)*Math.cos(pitch),Math.sin(yaw)*Math.cos(pitch),Math.sin(pitch)];
  const tangent=Math.tan(radians(parameters.fieldOfView)/2);
  const boundX=tangent*outputWidth/shortest*inset, boundY=tangent*outputHeight/shortest*inset;
  let distance=0;
  for(const x of [rect.x*width,(rect.x+rect.width)*width]) for(const y of [(1-rect.y-rect.height)*height,(1-rect.y)*height]) for(const z of [low,high]) {
    const relative=[x-center[0],y-center[1],z-center[2]], depth=dot(relative,back);
    const projected=Math.max(Math.abs(dot(relative,right))/boundX,Math.abs(dot(relative,up))/boundY);
    distance=Math.max(distance,projection==='perspective'?depth+projected:projected,depth+.0001*dim);
  }
  // Match the control field's 0.1 step, rounding outward so normalization cannot crop.
  const cameraDistance=Math.ceil(Math.max(.1,distance/(.14*dim)*(1+1e-7))*10)/10;
  const cameraPan=[(center[0]-width/2)/dim,(center[1]-height/2)/dim,center[2]/dim-.06];
  if (![cameraDistance,...cameraPan].every(Number.isFinite) || cameraDistance>1000) throw new Error('当前高度和视角超出相机可取景范围');
  return {cameraDistance,cameraPan,cameraFrameX:0,cameraFrameY:0,...(projection!==parameters.projection?{projection}:{})};
}

// Exact pixel-region reframing: same projection, output aspect and camera origin.
export function focusViewportRegion(parameters,{region,padding=.08}={}) {
  const rect=rectangle(region), inset=margin(padding);
  const [width,height,shortest]=imageSize(parameters);
  const scale=Math.max(rect.width,rect.height)/inset;
  const oldTangent=Math.tan(radians(parameters.fieldOfView)/2);
  const requestedFov=2*Math.atan(oldTangent*scale)*180/Math.PI;
  if (!Number.isFinite(requestedFov) || requestedFov<10 || requestedFov>120) throw new Error('聚焦区域超出视角范围（10–120°），请调整区域大小或留白');
  // FOV is written at whole degrees and filmshift at 1e-6. Recompute from the
  // final tangent, then check the actual rounded rectangle even with zero padding.
  for(let fieldOfView=Math.ceil(requestedFov);fieldOfView<=120;fieldOfView++) {
    const actualScale=Math.tan(radians(fieldOfView)/2)/oldTangent;
    const cameraFrameX=Number((((parameters.cameraFrameX??0)+(2*(rect.x+rect.width/2)-1)*width/shortest)/actualScale).toFixed(6));
    const cameraFrameY=Number((((parameters.cameraFrameY??0)+(1-2*(rect.y+rect.height/2))*height/shortest)/actualScale).toFixed(6));
    if ([cameraFrameX,cameraFrameY].some(value=>Math.abs(value)>10000)) throw new Error('聚焦区域超出相机移片范围');
    let covered=true;
    for(const x of [rect.x,rect.x+rect.width]) for(const y of [rect.y,rect.y+rect.height]) {
      const sx=((2*x-1)*width/shortest+(parameters.cameraFrameX??0))/actualScale-cameraFrameX;
      const sy=((1-2*y)*height/shortest+(parameters.cameraFrameY??0))/actualScale-cameraFrameY;
      covered&&=Math.abs(sx)<=width/shortest*inset+1e-12 && Math.abs(sy)<=height/shortest*inset+1e-12;
    }
    if(covered)return {fieldOfView,cameraFrameX,cameraFrameY};
  }
  throw new Error('聚焦区域超出视角范围（10–120°），请调整区域大小或留白');
}
