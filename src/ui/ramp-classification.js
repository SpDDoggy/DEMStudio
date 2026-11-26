import {resampleRamp,sampleRampColor} from './state.js';

export function validRampRange(range) {
  return Array.isArray(range) && range.length === 2 && range.every(Number.isFinite) && range[1] > range[0];
}

// Histogram quantiles use occupied-bin upper edges, rather than inventing a
// uniform within-bin distribution. Results are bounded preview estimates.
export function histogramQuantile(distribution, fraction) {
  if (!validRampRange(distribution?.range) || !Array.isArray(distribution.bins) || !distribution.bins.length || distribution.bins.some(n=>!Number.isFinite(n)||n<0)) throw new Error('高程分布无效');
  const {range,bins}=distribution, total=bins.reduce((sum,n)=>sum+n,0);
  if (!(total>0)) throw new Error('高程分布为空');
  if (!Number.isFinite(fraction)||fraction<0||fraction>1) throw new Error('分位数无效');
  if (fraction===0) return range[0];
  if (fraction===1) return range[1];
  let cumulative=0;
  for(let i=0;i<bins.length;i++) {
    cumulative+=bins[i];
    if(bins[i]>0 && cumulative>=fraction*total) return range[0]+(range[1]-range[0])*(i+1)/bins.length;
  }
  return range[1];
}

export function sampleQuantile(samples,fraction,range) {
  const values=samples.filter(value=>Number.isFinite(value)&&value>=range[0]&&value<=range[1]).sort((a,b)=>a-b);
  if(!values.length) throw new Error('高程分布为空');
  const position=(values.length-1)*fraction,index=Math.floor(position),weight=position-index;
  return values[index]*(1-weight)+values[Math.min(index+1,values.length-1)]*weight;
}

export function classifyRamp({method='equal',count,range,distribution,stops,mode='discrete',center=null}) {
  if(!validRampRange(range)) throw new Error('高程范围无效');
  if(!Number.isInteger(count)||count<2||count>16) throw new Error('分级数量无效');
  if(!['continuous','discrete'].includes(mode)) throw new Error('过渡方式无效');
  if(!['equal','quantile','custom'].includes(method)) throw new Error('分级方式无效');
  const span=range[1]-range[0], discrete=mode==='discrete';
  if(center!==null && (!Number.isFinite(center)||center<=range[0]||center>=range[1]||method!=='equal')) throw new Error('中心高程无效');
  if(center!==null&&!discrete&&count<3) throw new Error('中心色标至少需要三个色标');
  if(method==='custom') {
    if(!Array.isArray(stops)||stops.length<2||stops.length>16||stops.some((s,i)=>!Number.isFinite(s.position)||s.position<0||s.position>1||(i>0&&s.position<=stops[i-1].position)) || discrete&&(stops[0].position!==0||stops.at(-1).position>=1)) throw new Error('自定义分级无效');
    return {stops:stops.map(s=>({...s})),boundaries:[...stops.map(s=>range[0]+s.position*span),...(discrete?[range[1]]:[])],actualCount:stops.length,method};
  }
  const intervals=discrete?count:count-1;
  const coordinate=t=>center===null?range[0]+t*span:t<=.5?range[0]+t*2*(center-range[0]):center+(t-.5)*2*(range[1]-center);
  const samples=Array.isArray(distribution?.samples)?distribution.samples:null;
  if(method==='quantile' && !samples && (!validRampRange(distribution?.range)||distribution.range[0]<range[0]||distribution.range[1]>range[1])) throw new Error('分布不在赋色范围内');
  const quantile=fraction=>samples?sampleQuantile(samples,fraction,range):histogramQuantile(distribution,fraction);
  if(method==='quantile') quantile(.5);
  const edges=[range[0]];
  const coordinates=Array.from({length:intervals+1},(_,i)=>i/intervals);
  if(center!==null&&!discrete) {
    let nearest=1;
    for(let i=2;i<intervals;i++)if(Math.abs(coordinates[i]-.5)<Math.abs(coordinates[nearest]-.5))nearest=i;
    coordinates[nearest]=.5;
  }
  for(let i=1;i<intervals;i++) {
    const value=method==='quantile'?quantile(i/intervals):coordinate(coordinates[i]);
    if(value>edges.at(-1)&&value<range[1]) edges.push(value);
  }
  edges.push(range[1]);
  const actualCount=discrete?edges.length-1:edges.length;
  if(actualCount<2) throw new Error('有效分布不足以形成两级');
  const colors=resampleRamp(stops,actualCount,mode);
  if(center!==null&&!discrete) {
    const first=stops[0].position,last=stops.at(-1).position;
    const anchors=stops.map(stop=>({...stop,position:(stop.position-first)/(last-first)}));
    colors.forEach((stop,index)=>{stop.color=sampleRampColor(anchors,coordinates[index],'continuous');});
  }
  return {stops:colors.map((s,i)=>({...s,position:(edges[i]-range[0])/span})),boundaries:edges,actualCount,method};
}
