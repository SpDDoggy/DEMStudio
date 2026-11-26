// World Z is up; top view keeps +X to the right and +Y upwards.
export const cameraPresets = Object.freeze({
  top: { label: '顶', cameraYaw: -90, cameraPitch: 90, projection: 'orthographic' },
  front: { label: '前', cameraYaw: -90, cameraPitch: 0, projection: 'orthographic' },
  back: { label: '后', cameraYaw: 90, cameraPitch: 0, projection: 'orthographic' },
  left: { label: '左', cameraYaw: 180, cameraPitch: 0, projection: 'orthographic' },
  right: { label: '右', cameraYaw: 0, cameraPitch: 0, projection: 'orthographic' },
  iso: { label: '等轴', cameraYaw: 45, cameraPitch: 35.26439, projection: 'isometric' },
});

export function applyCameraPreset(parameters, name) {
  const preset = cameraPresets[name] || navigationPresets[name];
  if (!preset) return false;
  for (const key of ['cameraYaw', 'cameraPitch', 'projection']) parameters[key] = preset[key];
  return true;
}

function cameraBasis(parameters = {}) {
  const finite = (value, fallback) => Number.isFinite(value) ? value : fallback;
  const yaw = (finite(parameters.cameraYaw, 45) % 360) * Math.PI / 180;
  const pitch = (parameters.projection === 'isometric'
    ? 35.26439 : Math.max(0, Math.min(90, finite(parameters.cameraPitch, 35)))) * Math.PI / 180;
  // Analytic basis remains orthonormal even directly above the terrain.
  return {
    right: [-Math.sin(yaw), Math.cos(yaw), 0],
    up: [-Math.cos(yaw) * Math.sin(pitch), -Math.sin(yaw) * Math.sin(pitch), Math.cos(pitch)],
    back: [Math.cos(yaw) * Math.cos(pitch), Math.sin(yaw) * Math.cos(pitch), Math.sin(pitch)],
  };
}

export function projectCameraAxes(parameters = {}) {
  const {right, up, back} = cameraBasis(parameters);
  return ['X', 'Y', 'Z'].map((name, index) => ({
    name, x: right[index], y: -up[index], depth: back[index],
  })).sort((a, b) => a.depth - b.depth);
}

const faces = [
  { name: 'top', normal: [0, 0, 1], vertices: [[-1,-1,1],[1,-1,1],[1,1,1],[-1,1,1]] },
  { name: 'front', normal: [0, -1, 0], vertices: [[-1,-1,-1],[1,-1,-1],[1,-1,1],[-1,-1,1]] },
  { name: 'back', normal: [0, 1, 0], vertices: [[1,1,-1],[-1,1,-1],[-1,1,1],[1,1,1]] },
  { name: 'left', normal: [-1, 0, 0], vertices: [[-1,1,-1],[-1,-1,-1],[-1,-1,1],[-1,1,1]] },
  { name: 'right', normal: [1, 0, 0], vertices: [[1,-1,-1],[1,1,-1],[1,1,1],[1,-1,1]] },
];

export function projectViewCube(parameters = {}) {
  const {right, up, back} = cameraBasis(parameters);
  const dot = (a,b) => a.reduce((sum, value, index) => sum + value * b[index], 0);
  return faces.map(face => ({
    name: face.name, label: cameraPresets[face.name].label,
    depth: dot(face.normal, back),
    points: face.vertices.map(vertex => [dot(vertex,right), -dot(vertex,up)]),
    center: [dot(face.normal,right), -dot(face.normal,up)],
  })).filter(face => face.depth > .000001).sort((a,b) => a.depth-b.depth);
}
// Beveled navigation geometry; standard scene axes and six preset entries stay unchanged.
const directions=[['right',[1,0,0]],['left',[-1,0,0]],['back',[0,1,0]],['front',[0,-1,0]],['top',[0,0,1]],['bottom',[0,0,-1]]];
const directionLabel={right:'右',left:'左',back:'后',front:'前',top:'顶',bottom:'底'};
const navigationPresets={};
const bevelFaces=[];
const a=.72;
const dot3=(x,y)=>x.reduce((sum,value,index)=>sum+value*y[index],0);
function addBevelFace(names,vertices,normal,kind) {
  const length=Math.hypot(...normal),unit=normal.map(value=>value/length);
  const name=names.length===1?names[0]:`bevel-${names.join('-')}`;
  const label=names.map(name=>directionLabel[name]).join('');
  const enabled=normal[2]>=0 && names[0]!=='bottom';
  if(enabled&&names.length>1)navigationPresets[name]={label,cameraYaw:Math.atan2(normal[1],normal[0])*180/Math.PI,cameraPitch:Math.atan2(normal[2],Math.hypot(normal[0],normal[1]))*180/Math.PI,projection:'orthographic'};
  // Sort each convex facet in its own plane before projecting.
  const center=vertices[0].map((_,i)=>vertices.reduce((sum,v)=>sum+v[i],0)/vertices.length);
  const u=vertices[0].map((value,i)=>value-center[i]);
  const v=[unit[1]*u[2]-unit[2]*u[1],unit[2]*u[0]-unit[0]*u[2],unit[0]*u[1]-unit[1]*u[0]];
  vertices.sort((p,q)=>Math.atan2(dot3(p.map((n,i)=>n-center[i]),v),dot3(p.map((n,i)=>n-center[i]),u))-Math.atan2(dot3(q.map((n,i)=>n-center[i]),v),dot3(q.map((n,i)=>n-center[i]),u)));
  bevelFaces.push({name,label,vertices,normal:unit,kind,enabled});
}
for(const [name,normal] of directions) {
 const axis=normal.findIndex(Boolean),other=[0,1,2].filter(i=>i!==axis);
 addBevelFace([name],[-1,1].flatMap(s=>[-1,1].map(t=>{const p=normal.slice();p[other[0]]=s*a;p[other[1]]=t*a;return p;})),normal,'face');
}
for(let i=0;i<directions.length;i++)for(let j=i+1;j<directions.length;j++) {
 const [n1,p]=directions[i],[n2,q]=directions[j];if(dot3(p,q)!==0)continue;
 const normal=p.map((v,k)=>v+q[k]),axis=normal.findIndex(v=>!v);
 const vertices=[-a,a].flatMap(t=>[0,1].map(side=>normal.map((v,k)=>k===axis?t:v*(side?(p[k]?a:1):(p[k]?1:a)))));
 addBevelFace([n1,n2],vertices,normal,'edge');
}
for(const x of [-1,1])for(const y of [-1,1])for(const z of [-1,1]) {
 const normal=[x,y,z],names=[x>0?'right':'left',y>0?'back':'front',z>0?'top':'bottom'];
 addBevelFace(names,[0,1,2].map(axis=>normal.map((v,k)=>v*(k===axis?1:a))),normal,'corner');
}
export function projectNavigationCube(parameters={}) {
 const {right,up,back}=cameraBasis(parameters);
 return bevelFaces.map(face=>({...face,depth:dot3(face.normal,back),points:face.vertices.map(vertex=>[dot3(vertex,right),-dot3(vertex,up)]),center:[dot3(face.normal,right),-dot3(face.normal,up)]})).filter(face=>face.depth>.000001).sort((a,b)=>a.depth-b.depth);
}
export function roundedFacePath(points,radius=1.2) {
 const toward=(p,q)=>{const t=Math.min(.2,radius/Math.hypot(q[0]-p[0],q[1]-p[1]));return p.map((value,i)=>value+(q[i]-value)*t);};
 return points.map((p,i)=>{const before=toward(p,points[(i+points.length-1)%points.length]),after=toward(p,points[(i+1)%points.length]);return `${i?'L':'M'}${before.join(',')}Q${p.join(',')} ${after.join(',')}`;}).join('')+'Z';
}
