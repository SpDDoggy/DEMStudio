// Keep the native linear multiplier compatible with existing renderer callers.
export function nativeHeightScaling(parameters) {
  const mode = parameters.heightScaleMode;
  const value = parameters.heightScale;
  const scale = mode === 'nonlinear' ? (value < 1 ? value * value : 2 * value - 1) : value;
  return {
    heightScale: .2 * (mode === 'accelerated' ? scale * scale : scale),
    heightScaleMode: mode,
    heightCurve: mode === 'nonlinear' ? scale : 1,
  };
}
