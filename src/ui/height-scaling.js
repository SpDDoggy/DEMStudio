// Keep the native linear multiplier compatible with existing renderer callers.
export function nativeHeightScaling(parameters) {
  if (parameters._transitionHeight != null) {
    const height = validateTransitionHeight(parameters._transitionHeight);
    return {...height,heightScaleMode: 'nonlinear'};
  }
  const mode = parameters.heightScaleMode;
  const value = parameters.heightScale;
  const scale = mode === 'nonlinear'
    ? (value < 1 ? value : value <= 15 ? 4 * value - 3 : 57 + (value - 15) * (999 - 57) / (500 - 15))
    : value;
  return {
    heightScale: .2 * (mode === 'accelerated' ? scale * scale : scale),
    heightScaleMode: mode,
    heightCurve: mode === 'nonlinear' ? scale : 1,
  };
}

// A mixed scale/curve cannot generally be represented by one UI slider value.
// Keep the effective pair so cancelling, saving and retargeting lose no shape.
export function validateTransitionHeight(value) {
  if (!value || !Number.isFinite(value.heightScale) || value.heightScale <= 0 ||
      !Number.isFinite(value.heightCurve) || value.heightCurve < .01 || value.heightCurve > 1000)
    throw new Error('地形过渡参数无效');
  return {heightScale:value.heightScale,heightCurve:value.heightCurve};
}
