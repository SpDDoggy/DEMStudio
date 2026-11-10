export function bounceLimit(parameters) {
  return parameters.experimentalBounces ? 100 : 15;
}

export function switchSkyModel(parameters, model) {
  if (parameters.skyModel === model) return false;
  parameters.skyModel = model;
  parameters.skyIntensity = model === 'atmosphere' ? 45 : 100;
  return true;
}

export function toggleExperimentalBounces(parameters) {
  parameters.experimentalBounces = !parameters.experimentalBounces;
  parameters.maxBounces = Math.min(parameters.maxBounces, bounceLimit(parameters));
  return parameters.experimentalBounces;
}

export function acceptsExperimentalShortcut(event) {
  const backquote = event.code ? event.code === 'Backquote' : event.key === '`' || event.key === '~';
  return backquote && !event.repeat && !event.isComposing && !event.defaultPrevented &&
    event.ctrlKey && event.shiftKey && !event.altKey && !event.metaKey &&
    !event.target?.closest?.('dialog');
}
