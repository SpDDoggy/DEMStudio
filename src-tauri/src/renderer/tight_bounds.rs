//! Extra Bilinear node bounds. Raw/mip inputs remain immutable.
//! Values are raw normalized maxima, not world-height or rounded ray bounds.
use super::paged_source::{Brick,MipLevel,BRICK_EDGE,COARSE_LEVEL};

pub(super) fn enabled(surface:&str)->bool {
    surface=="bilinear"&&std::env::var("DEMSTUDIO_TIGHT_BOUNDS").as_deref()==Ok("on")
}
pub(super) const LOW_BYTES:u64=544_000;

fn checked_value(v:f32)->f32{if v.is_finite()&&v>=0.{v}else{f32::INFINITY}}
fn initial_level(mut raw:impl FnMut(u32,u32)->f32,size:[u32;2],offset:u32,
                 origin:[i32;2],edge:u32,level:u32)->Result<MipLevel,String>{
    if size.contains(&0)||offset>1||level>30||edge==0{return Err("紧界尺寸、偏移或层级无效".into());}
    let scale=1i64<<level;
    if origin.iter().any(|&v|i64::from(v)%scale!=0){return Err("紧界原点未全局对齐".into());}
    let count=(edge as usize).checked_mul(edge as usize).ok_or("紧界尺寸溢出")?;
    let mut values=vec![0.;count];
    for y in 0..edge{for x in 0..edge{
        let cx=i64::from(origin[0])+i64::from(x)*scale-i64::from(offset);
        let cy=i64::from(origin[1])+i64::from(y)*scale-i64::from(offset);
        let x0=cx.clamp(0,i64::from(size[0])-1)as u32;
        let x1=(cx+scale).clamp(0,i64::from(size[0])-1)as u32;
        let y0=cy.clamp(0,i64::from(size[1])-1)as u32;
        let y1=(cy+scale).clamp(0,i64::from(size[1])-1)as u32;
        let mut h=0f32;
        for gy in y0..=y1{for gx in x0..=x1{h=h.max(checked_value(raw(gx,gy)));}}
        values[(y*edge+x)as usize]=h;
    }}
    Ok(MipLevel{level,edge,values})
}
fn parent(child:&MipLevel)->Result<MipLevel,String>{
    if child.edge<2||child.edge%2!=0{return Err("紧界父层尺寸无效".into());}
    let edge=child.edge/2;let mut values=vec![0.;(edge*edge)as usize];
    for y in 0..edge{for x in 0..edge{
        let at=((2*y)*child.edge+2*x)as usize;
        values[(y*edge+x)as usize]=child.values[at].max(child.values[at+1])
            .max(child.values[at+child.edge as usize]).max(child.values[at+child.edge as usize+1]);
    }}
    Ok(MipLevel{level:child.level+1,edge,values})
}
/// All original samples are read-only. Inaccessible outer halo nodes become +INF;
/// using brick-edge clamping there would invent an unsafe low bound.
pub(super) fn build_low(brick:&Brick,size:[u32;2],offset:u32)->Result<Vec<MipLevel>,String>{
    let raw=brick.levels.first().filter(|v|v.level==0&&v.edge==BRICK_EDGE
        &&v.values.len()==(BRICK_EDGE*BRICK_EDGE)as usize).ok_or("紧界缺少完整原始砖块")?;
    let sample=|x:u32,y:u32|{
        let ix=i64::from(x)-i64::from(brick.origin[0]);let iy=i64::from(y)-i64::from(brick.origin[1]);
        if ix<0||iy<0||ix>=i64::from(BRICK_EDGE)||iy>=i64::from(BRICK_EDGE){f32::INFINITY}
        else{raw.values[iy as usize*BRICK_EDGE as usize+ix as usize]}
    };
    let mut levels=vec![initial_level(sample,size,offset,brick.origin,BRICK_EDGE/2,1)?];
    while levels.last().unwrap().level<COARSE_LEVEL-1{levels.push(parent(levels.last().unwrap())?);}
    Ok(levels)
}
/// Callback supplies globally clamped, normalized Y-up source samples in RAM.
/// Do not perform physical per-texel IO in this callback. Logical padding must
/// include the geographic border, independently of the original raw texture.
pub(super) fn build_coarse(size:[u32;2],padded:u32,offset:u32,
    raw:impl FnMut(u32,u32)->f32)->Result<Vec<MipLevel>,String>{
    if size.contains(&0)||offset>1{return Err("紧界尺寸或偏移无效".into());}
    let required=size[0].max(size[1]).checked_add(offset*2).ok_or("紧界逻辑尺寸溢出")?;
    if !padded.is_power_of_two()||padded<32||padded<required{return Err("紧界粗层未覆盖逻辑范围".into());}
    let mut levels=vec![initial_level(raw,size,offset,[0,0],padded/32,5)?];
    while levels.last().unwrap().edge>1{levels.push(parent(levels.last().unwrap())?);}
    Ok(levels)
}

/// Streaming source scan: each normalized Y-up sample must be projected once.
pub(super) struct CoarseAccumulator{size:[u32;2],offset:u32,level:MipLevel,seen:u64}
impl CoarseAccumulator{
    pub(super) fn new(size:[u32;2],padded:u32,offset:u32)->Result<Self,String>{
        let required=size[0].max(size[1]).checked_add(offset.checked_mul(2).ok_or("紧界偏移溢出")?).ok_or("紧界逻辑尺寸溢出")?;
        if size.contains(&0)||offset>1||!padded.is_power_of_two()||padded<32||padded<required{return Err("紧界粗层未覆盖逻辑范围".into());}
        let edge=padded/32;let count=(edge as usize).checked_mul(edge as usize).ok_or("紧界尺寸溢出")?;
        Ok(Self{size,offset,level:MipLevel{level:5,edge,values:vec![0.;count]},seen:0})
    }
    pub(super) fn project_sample(&mut self,x:u32,y:u32,value:f32)->Result<(),String>{
        if x>=self.size[0]||y>=self.size[1]{return Err("紧界投影样本超出源范围".into());}
        self.seen+=1;let n=self.level.edge;let qx=u64::from(x)+u64::from(self.offset);let qy=u64::from(y)+u64::from(self.offset);
        // A node [k*S,(k+1)*S] owns both endpoints. Source-edge samples
        // additionally cover every virtual coordinate clamped onto that edge.
        let x0=if x==0{0}else{qx.saturating_sub(32).div_ceil(32).min(u64::from(n-1))as u32};
        let y0=if y==0{0}else{qy.saturating_sub(32).div_ceil(32).min(u64::from(n-1))as u32};
        let x1=if x==self.size[0]-1{n-1}else{(qx/32).min(u64::from(n-1))as u32};
        let y1=if y==self.size[1]-1{n-1}else{(qy/32).min(u64::from(n-1))as u32};
        let value=checked_value(value);
        for cy in y0..=y1{for cx in x0..=x1{let v=&mut self.level.values[(cy*n+cx)as usize];*v=v.max(value);}}
        Ok(())
    }
    pub(super) fn finish(self)->Result<Vec<MipLevel>,String>{
        if self.seen!=u64::from(self.size[0])*u64::from(self.size[1]){return Err("紧界源扫描未完整投影".into());}
        let mut levels=vec![self.level];while levels.last().unwrap().edge>1{levels.push(parent(levels.last().unwrap())?);}Ok(levels)
    }
}



