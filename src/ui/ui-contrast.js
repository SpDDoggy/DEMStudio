function rgb(hex) {
  return [1,3,5].map(i => parseInt(hex.slice(i,i+2),16)/255);
}
export function luminance(hex) {
  return rgb(hex).map(v => v <= .04045 ? v/12.92 : ((v+.055)/1.055)**2.4)
    .reduce((sum,v,i) => sum+v*[.2126,.7152,.0722][i],0);
}
export function contrastTheme(parameters) {
  return 'dark';
}
export function glassAppearance(parameters) {
  const colors = parameters.backgroundMode === 'gradient'
    ? [parameters.backgroundColor,parameters.backgroundColor2]
    : [parameters.backgroundMode === 'solid' ? parameters.backgroundColor : parameters.skyColor];
  const brightness=Math.sqrt(colors.reduce((sum,color) => sum+luminance(color ?? '#a9c9e8'),0)/colors.length);
  return {alpha:.50+.30*brightness,popupAlpha:.74+.14*brightness};
}
export function applyUiContrast(parameters, root=document.documentElement) {
  root.dataset.uiContrast='dark';
  const {alpha,popupAlpha}=glassAppearance(parameters);
  root.style.setProperty('--glass-alpha',alpha.toFixed(3));
  root.style.setProperty('--popup-alpha',popupAlpha.toFixed(3));
}
