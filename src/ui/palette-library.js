import { rampLibrary, rampGradient } from './state.js';
const make = (id, name, colors, positions) => ({id, name, stops: colors.split(' ').map((color,i,a)=>({position:positions ? positions[i]/100 : i/(a.length-1),color:'#'+color.toLowerCase()}))});
export const defaultPaletteDocument = {version:1,palettes:[
  ...Object.entries(rampLibrary.continuous).map(([id,p])=>({id,name:p.label,stops:p.colors.map((color,i,a)=>({position:i/(a.length-1),color}))})),
  make('terra-verde','真境山岳','253f36 506c52 8d9a78 baa384 d0ccbb f3f5f1'),
  make('limestone-ridge','石灰岩脊','24474a 58736b 89958b b5ae9d d8d3c8 f5f3e9'),
  make('boreal-veil','苔原冷杉','203e3d 3d6158 718a75 98a997 c5c9c0 f3f2ec'),
  make('red-canyon','赤色峡谷','5a3d35 895744 bc7c59 d7a57b e8ccad f5e5cb'),
  make('glacier-blue','冰川北境','244553 3f7184 75a9b7 b2d5da dbeae9 fafcf9'),
  make('basalt-field','玄武火山','1c282c 394346 677070 918c82 c0b8ac e8e5df'),
  make('atlas-classic','经典地貌图','345b50 6f9378 b3bc92 d2bc84 b88b69 f4ebe5'),
  make('strata-elevation','地层高程谱','234e69 377c83 78a89a d0cc9d ca9570 f2e9d9'),
  make('warm-limestone','暖白石灰岩','626d5a 838976 a19b8d bfb9ac d7d5cc e5e8e5',[0,24,51,76,94,100]),
  make('cedar-limestone','雪松与石灰岩','31574e 658576 9c9e83 c0aa8f a8b4b4 e8ede8',[0,19,38,60,80,100]),
  make('himalayan-strata','雪域层岩','42625c 79919a a89b87 bd968b b5c8c9 ebf0ea',[0,19,38,61,82,100]),
  make('mineral-confection','矿物糖霜','27658c 67bac1 c3dcc7 f0d2a9 eba18d ba5f85',[0,17,36,56,79,100]),
]};
export function normalizePaletteDocument(document) {
  if (document?.version !== 1 || !Array.isArray(document.palettes) || !document.palettes.length) throw new Error('配色文件无效');
  const ids=new Set();
  return {version:1,palettes:document.palettes.map(p=>{
    if(typeof p.id!=='string'||!p.id.trim()||ids.has(p.id)||typeof p.name!=='string'||!p.name.trim()||!Array.isArray(p.stops)||p.stops.length<2||p.stops.length>16) throw new Error('配色无效');
    ids.add(p.id);
    const stops=p.stops.map(s=>{if(!Number.isFinite(s.position)||s.position<0||s.position>1||!/^#[\da-f]{6}$/i.test(s.color))throw new Error('色标无效');return {position:s.position,color:s.color.toLowerCase()};}).sort((a,b)=>a.position-b.position);
    if(stops[0].position!==0||stops.at(-1).position!==1||stops.some((s,i)=>i&&s.position<=stops[i-1].position))throw new Error('色标位置须从0到100严格递增');
    return {id:p.id,name:p.name.trim(),stops};
  })};
}
export function palettePreview(stops,mode,reverse=false) {
  const display=stops.map(s=>({...s}));
  if(mode==='discrete'&&display.at(-1).position===1)display.forEach(s=>s.position*=(display.length-1)/display.length);
  return rampGradient(display,mode,reverse);
}
