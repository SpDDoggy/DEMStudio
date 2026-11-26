const clone=value=>JSON.parse(JSON.stringify(value));
const visible=e=>e.visible!==false;
const elements=layout=>layout.elements??[];
export function selectionIds(layout,ids,{expandGroups=true,includeHidden=false}={}){
  const selected=new Set(ids??[]),groups=new Set(expandGroups?elements(layout).filter(e=>selected.has(e.id)&&e.groupId).map(e=>e.groupId):[]);
  return elements(layout).filter(e=>(includeHidden||visible(e))&&(selected.has(e.id)||groups.has(e.groupId))).map(e=>e.id);
}
function bounds(items){
  if(!items.length)return null;
  const x=Math.min(...items.map(e=>e.x)),y=Math.min(...items.map(e=>e.y));
  return {x,y,width:Math.max(...items.map(e=>e.x+e.width))-x,height:Math.max(...items.map(e=>e.y+e.height))-y};
}
export function selectionBounds(layout,ids,{includeHidden=true}={}){const selected=new Set(selectionIds(layout,ids,{includeHidden}));return bounds(elements(layout).filter(e=>selected.has(e.id)));}
function movable(layout,ids){
  const visibleIds=selectionIds(layout,ids,{expandGroups:false});
  const selected=new Set(selectionIds(layout,visibleIds,{includeHidden:true}));
  const blocked=new Set(elements(layout).filter(e=>e.locked&&e.groupId).map(e=>e.groupId));
  return elements(layout).filter(e=>selected.has(e.id)&&!e.locked&&!blocked.has(e.groupId));
}
function units(items){
  const groups=new Map();
  for(const e of items){const id=e.groupId?`group:${e.groupId}`:`element:${e.id}`;if(!groups.has(id))groups.set(id,[]);groups.get(id).push(e);}
  return [...groups.values()].map(items=>({items,...bounds(items)}));
}
function move(unit,dx,dy){for(const e of unit.items){e.x+=dx;e.y+=dy;}}
export function alignElements(layout,ids,mode){
  const next=clone(layout),selected=units(movable(next,ids));if(!selected.length)return next;
  const box=selected.length===1?{x:0,y:0,width:1,height:1}:bounds(selected);
  for(const unit of selected){let dx=0,dy=0;
    if(mode==='left')dx=box.x-unit.x;
    if(mode==='center')dx=box.x+box.width/2-unit.x-unit.width/2;
    if(mode==='right')dx=box.x+box.width-unit.x-unit.width;
    if(mode==='top')dy=box.y-unit.y;
    if(mode==='middle')dy=box.y+box.height/2-unit.y-unit.height/2;
    if(mode==='bottom')dy=box.y+box.height-unit.y-unit.height;
    move(unit,dx,dy);
  }return next;
}
export function distributeElements(layout,ids,axis){
  const next=clone(layout),selected=units(movable(next,ids)),x=axis==='x'||axis==='horizontal';
  if(!['x','y','horizontal','vertical'].includes(axis)||selected.length<3)return next;
  const position=x?'x':'y',size=x?'width':'height';selected.sort((a,b)=>a[position]-b[position]);
  const first=selected[0],last=selected.at(-1),gap=(last[position]+last[size]-first[position]-selected.reduce((s,e)=>s+e[size],0))/(selected.length-1);
  let cursor=first[position]+first[size]+gap;
  for(let i=1;i<selected.length-1;i++){const unit=selected[i],delta=cursor-unit[position];move(unit,x?delta:0,x?0:delta);cursor+=unit[size]+gap;}return next;
}
export function groupElements(layout,ids,groupId){
  const next=clone(layout),selected=movable(next,ids);if(selected.length<2||typeof groupId!=='string'||!groupId)return next;
  for(const e of selected)e.groupId=groupId;return next;
}
export function ungroupElements(layout,ids){const next=clone(layout);for(const e of movable(next,ids))delete e.groupId;return next;}
export function transformSelection(layout,ids,{dx=0,dy=0,scaleX=1,scaleY=1,anchorX,anchorY}={}){
  const next=clone(layout),selected=movable(next,ids),box=bounds(selected);
  if(!box||![dx,dy,scaleX,scaleY].every(Number.isFinite)||scaleX<=0||scaleY<=0)return next;
  const ax=Number.isFinite(anchorX)?anchorX:box.x,ay=Number.isFinite(anchorY)?anchorY:box.y;
  for(const e of selected){e.x=ax+(e.x-ax)*scaleX+dx;e.y=ay+(e.y-ay)*scaleY+dy;e.width*=scaleX;e.height*=scaleY;if(e.type==='point')e.pointSize=Math.min(e.width*next.width,e.height*next.height);}
  return next;
}
export function snapSelection(layout,ids,{dx=0,dy=0,tolerance=.005}={}){
  const selected=movable(layout,ids),box=bounds(selected),guides=[];
  if(!box)return {layout:clone(layout),guides,dx:0,dy:0};
  const selectedIds=new Set(selected.map(e=>e.id));
  const targets=elements(layout).filter(e=>visible(e)&&!selectedIds.has(e.id));
  const deltas={x:dx,y:dy};
  for(const axis of ['x','y']){
    const size=axis==='x'?'width':'height',limit=typeof tolerance==='number'?tolerance:tolerance?.[axis]??0;
    const positions=[0,.5,1,...targets.flatMap(e=>[e[axis],e[axis]+e[size]/2,e[axis]+e[size]])];
    const sources=[box[axis],box[axis]+box[size]/2,box[axis]+box[size]];let best=null;
    for(const position of positions)for(const source of sources){const correction=position-source-deltas[axis];if(Math.abs(correction)<=limit&&(!best||Math.abs(correction)<Math.abs(best.correction)))best={correction,position};}
    if(best){deltas[axis]+=best.correction;guides.push({axis,position:best.position});}
  }
  return {layout:transformSelection(layout,ids,{dx:deltas.x,dy:deltas.y}),guides,dx:deltas.x,dy:deltas.y};
}
