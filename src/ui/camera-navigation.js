// Pan is measured in map-maximum-edge units, matching the native camera target.
export function cameraPanDelta(parameters, dx, dy, viewport) {
  const { width, height } = viewport;
  if (![dx, dy, width, height].every(Number.isFinite) || width <= 0 || height <= 0) return [0, 0, 0];
  const imageWidth = Math.max(1, parameters.width);
  const imageHeight = Math.max(1, parameters.height);
  const coverScale = Math.max(width / imageWidth, height / imageHeight);
  const displayedShortest = Math.min(imageWidth, imageHeight) * coverScale;
  const units = 2 * (1.4 * parameters.cameraDistance / 10)
    * Math.tan(parameters.fieldOfView * Math.PI / 360) / displayedShortest;
  const yaw = parameters.cameraYaw * Math.PI / 180;
  const pitch = (parameters.projection === 'isometric'
    ? 35.26439 : Math.max(0, Math.min(90, parameters.cameraPitch))) * Math.PI / 180;
  const right = [-Math.sin(yaw), Math.cos(yaw), 0];
  const up = [-Math.cos(yaw) * Math.sin(pitch), -Math.sin(yaw) * Math.sin(pitch), Math.cos(pitch)];
  return right.map((component, index) => (-dx * component + dy * up[index]) * units);
}

export function nativePanPixels(payload, devicePixelRatio) {
  const scale = Number.isFinite(devicePixelRatio) && devicePixelRatio > 0 ? devicePixelRatio : 1;
  return [payload.dx / scale, payload.dy / scale];
}

export function resetCameraParameters(parameters, fields) {
  for (const key of ['cameraYaw', 'cameraPitch', 'cameraDistance', 'fieldOfView', 'projection']) parameters[key] = fields[key].value;
  parameters.cameraPan = [0, 0, 0];
}
