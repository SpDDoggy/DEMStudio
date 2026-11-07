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
  const preset = cameraPresets[name];
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
