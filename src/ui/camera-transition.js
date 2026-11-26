const keys=['cameraYaw','cameraPitch','cameraDistance','cameraPan','fieldOfView','cameraFrameX','cameraFrameY'];
const copy=value=>JSON.parse(JSON.stringify(value));
const shortestAngle=(a,b)=>((b-a+180)%360+360)%360-180;
// Distance is multiplicative: an equal zoom ratio should take equal time at
// either end of the distance range. Pan is measured in visible frame widths.
export function cameraTransitionDuration(from,to){
  const distanceA=Math.max(.0001,from.cameraDistance??10),distanceB=Math.max(.0001,to.cameraDistance??10);
  const fovA=from.fieldOfView??45,fovB=to.fieldOfView??45;
  const span=2*Math.sqrt(distanceA*distanceB)*Math.tan((fovA+fovB)*Math.PI/720);
  const panA=from.cameraPan??[0,0,0],panB=to.cameraPan??[0,0,0];
  const travel=Math.max(
    Math.log(Math.max(distanceA,distanceB)/Math.min(distanceA,distanceB))*900,
    Math.abs(shortestAngle(from.cameraYaw??0,to.cameraYaw??0))*10,
    Math.abs((to.cameraPitch??0)-(from.cameraPitch??0))*10,
    Math.hypot(...panA.map((v,i)=>(panB[i]??0)-v))/Math.max(.0001,span)*700,
    Math.abs(Math.log(Math.tan(fovB*Math.PI/360)/Math.tan(fovA*Math.PI/360)))*900,
    Math.hypot((to.cameraFrameX??0)-(from.cameraFrameX??0),(to.cameraFrameY??0)-(from.cameraFrameY??0))*700
  );
  return Math.min(4000,140+travel*.6);
}
export function interpolateCamera(from,to,progress,{easing='smooth'}={}){
  if(progress>=1)return copy(to);
  if(progress<=0)return {...copy(from),projection:to.projection};
  const t=easing==='linear'?progress:progress*progress*(3-2*progress),result={...copy(from),projection:to.projection};
  for(const key of keys){
    const a=from[key]??0,b=to[key]??0;
    if(key==='cameraPan')result[key]=a.map((v,i)=>v+(b[i]-v)*t);
    else if(key==='cameraYaw'){const delta=((b-a+180)%360+360)%360-180;result[key]=((a+delta*t+180)%360+360)%360-180;}
    else if(key==='cameraDistance')result[key]=Math.exp(Math.log(a)+(Math.log(b)-Math.log(a))*t);
    else result[key]=a+(b-a)*t;
  }
  return result;
}
export function createCameraTransition({read,write,finish,onCancel=()=>{},requestFrame=requestAnimationFrame,cancelFrame=cancelAnimationFrame,now=()=>performance.now(),duration=650}){
  let frame=null,generation=0;
  function cancel(){generation++;const active=frame!==null;if(active)cancelFrame(frame);frame=null;if(active)onCancel();}
  return {cancel,start(target){
    cancel();const token=generation,from=copy(read()),to=copy(target),start=now();
    function step(time){
      if(token!==generation)return;
      const progress=Math.min(1,Math.max(0,(time-start)/duration));
      frame=null;write(interpolateCamera(from,to,progress));
      if(token!==generation)return;
      if(progress===1)finish();else frame=requestFrame(step);
    }
    frame=requestFrame(step);
  }};
}
