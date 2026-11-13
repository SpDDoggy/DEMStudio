//! Raw Bilinear node extrema only. No visibility certificate or GPU short circuit.
//! Each node includes every leaf corner, including both shared endpoints.
use super::paged_source::{Brick,BRICK_EDGE,COARSE_LEVEL};
pub(super) const LOW_BYTES:u64=(320*320+160*160+80*80+40*40)*8;
pub(super) const UNKNOWN:[f32;2]=[f32::NEG_INFINITY,f32::INFINITY];
const EMPTY:[f32;2]=[f32::INFINITY,f32::NEG_INFINITY];
#[derive(Clone,Debug,PartialEq)]
pub(super) struct BoundsLevel{pub level:u32,pub edge:u32,pub values:Vec<[f32;2]>}
fn include(bounds:&mut [f32;2],value:f32){
    if value.is_finite(){bounds[0]=bounds[0].min(value);bounds[1]=bounds[1].max(value);}else{*bounds=UNKNOWN;}
}
fn validate(size:[u32;2],padded:u32,offset:u32)->Result<(),String>{
    let required=size[0].max(size[1]).checked_add(offset.checked_mul(2).ok_or("可见性界偏移溢出")?).ok_or("可见性界尺寸溢出")?;
    if size.contains(&0)||offset>1||!padded.is_power_of_two()||padded<32||padded<required{return Err("可见性界尺寸或逻辑覆盖无效".into());}Ok(())
}
fn initial(mut raw:impl FnMut(u32,u32)->f32,size:[u32;2],offset:u32,origin:[i32;2],edge:u32,level:u32)->Result<BoundsLevel,String>{
    if size.contains(&0)||offset>1||level>30||edge==0{return Err("可见性界层级或尺寸无效".into());}
    let scale=1i64<<level;
    if origin.iter().any(|&v|i64::from(v)%scale!=0){return Err("可见性界原点未全局对齐".into());}
    let count=(edge as usize).checked_mul(edge as usize).ok_or("可见性界分配溢出")?;
    let mut values=vec![EMPTY;count];
    for y in 0..edge{for x in 0..edge{
        let cell=[i64::from(origin[0])+i64::from(x)*scale-i64::from(offset),i64::from(origin[1])+i64::from(y)*scale-i64::from(offset)];
        let lo=[cell[0].clamp(0,i64::from(size[0])-1)as u32,cell[1].clamp(0,i64::from(size[1])-1)as u32];
        let hi=[(cell[0]+scale).clamp(0,i64::from(size[0])-1)as u32,(cell[1]+scale).clamp(0,i64::from(size[1])-1)as u32];
        let bounds=&mut values[(y*edge+x)as usize];
        for gy in lo[1]..=hi[1]{for gx in lo[0]..=hi[0]{include(bounds,raw(gx,gy));}}
    }}Ok(BoundsLevel{level,edge,values})
}
fn parent(child:&BoundsLevel)->Result<BoundsLevel,String>{
    if child.edge<2||child.edge%2!=0||child.values.len()!=child.edge as usize*child.edge as usize{return Err("可见性界父层输入无效".into());}
    let edge=child.edge/2;let mut values=vec![EMPTY;(edge*edge)as usize];
    for y in 0..edge{for x in 0..edge{
        let at=((2*y)*child.edge+2*x)as usize;let result=&mut values[(y*edge+x)as usize];
        for index in [at,at+1,at+child.edge as usize,at+child.edge as usize+1]{
            let v=child.values[index];result[0]=result[0].min(v[0]);result[1]=result[1].max(v[1]);
        }
    }}Ok(BoundsLevel{level:child.level+1,edge,values})
}
pub(super) fn build_low(brick:&Brick,size:[u32;2],offset:u32)->Result<Vec<BoundsLevel>,String>{
    let raw=brick.levels.first().filter(|v|v.level==0&&v.edge==BRICK_EDGE&&v.values.len()==BRICK_EDGE as usize*BRICK_EDGE as usize).ok_or("可见性界缺少完整raw砖块")?;
    let sample=|x:u32,y:u32|{
        let ix=i64::from(x)-i64::from(brick.origin[0]);let iy=i64::from(y)-i64::from(brick.origin[1]);
        if ix<0||iy<0||ix>=i64::from(BRICK_EDGE)||iy>=i64::from(BRICK_EDGE){f32::NAN}else{raw.values[iy as usize*BRICK_EDGE as usize+ix as usize]}
    };
    let mut levels=vec![initial(sample,size,offset,brick.origin,BRICK_EDGE/2,1)?];
    while levels.last().unwrap().level<COARSE_LEVEL-1{levels.push(parent(levels.last().unwrap())?);}Ok(levels)
}
/// Supplies normalized Y-up source values; this callback must not do per-texel IO.
pub(super) fn build_coarse(size:[u32;2],padded:u32,offset:u32,raw:impl FnMut(u32,u32)->f32)->Result<Vec<BoundsLevel>,String>{
    validate(size,padded,offset)?;let mut levels=vec![initial(raw,size,offset,[0,0],padded/32,5)?];
    while levels.last().unwrap().edge>1{levels.push(parent(levels.last().unwrap())?);}Ok(levels)
}
/// Exact texture payload (not driver allocation granularity) for LOD5..top RG32F.
pub(super) fn coarse_bytes(padded:u32)->Option<u64>{
    if !padded.is_power_of_two()||padded<32{return None;}let mut edge=u64::from(padded/32);let mut bytes=0u64;
    loop{bytes=bytes.checked_add(edge.checked_mul(edge)?.checked_mul(8)?)?;if edge==1{return Some(bytes);}edge/=2;}
}
/// Each logical Y-up source coordinate must be projected exactly once by caller.
/// Retains only the LOD5 grid and count, with no source-sized bitmap or samples.
pub(super) struct CoarseAccumulator{size:[u32;2],offset:u32,level:BoundsLevel,seen:u64}
impl CoarseAccumulator{
    pub(super) fn new(size:[u32;2],padded:u32,offset:u32)->Result<Self,String>{
        validate(size,padded,offset)?;let edge=padded/32;let count=(edge as usize).checked_mul(edge as usize).ok_or("可见性界分配溢出")?;
        Ok(Self{size,offset,level:BoundsLevel{level:5,edge,values:vec![EMPTY;count]},seen:0})
    }
    pub(super) fn project_sample(&mut self,x:u32,y:u32,value:f32)->Result<(),String>{
        if x>=self.size[0]||y>=self.size[1]{return Err("可见性界源样本越界".into());}
        self.seen=self.seen.checked_add(1).ok_or("可见性界扫描计数溢出")?;
        let n=self.level.edge;let q=[u64::from(x)+u64::from(self.offset),u64::from(y)+u64::from(self.offset)];
        let lo=[if x==0{0}else{q[0].saturating_sub(32).div_ceil(32).min(u64::from(n-1))as u32},if y==0{0}else{q[1].saturating_sub(32).div_ceil(32).min(u64::from(n-1))as u32}];
        let hi=[if x==self.size[0]-1{n-1}else{(q[0]/32).min(u64::from(n-1))as u32},if y==self.size[1]-1{n-1}else{(q[1]/32).min(u64::from(n-1))as u32}];
        for cy in lo[1]..=hi[1]{for cx in lo[0]..=hi[0]{include(&mut self.level.values[(cy*n+cx)as usize],value);}}Ok(())
    }
    pub(super) fn finish(self)->Result<Vec<BoundsLevel>,String>{
        if self.seen!=u64::from(self.size[0])*u64::from(self.size[1])||self.level.values.iter().any(|v|v[0]>v[1]){return Err("可见性界源扫描不完整".into());}
        let mut levels=vec![self.level];while levels.last().unwrap().edge>1{levels.push(parent(levels.last().unwrap())?);}Ok(levels)
    }
}

