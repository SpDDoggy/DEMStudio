//! Precompute the original bc_bound raw maxima, independently per mip level.
use super::paged_source::MipLevel;
pub(super) fn enabled(surface:&str)->bool{
    surface=="bicubic"&&std::env::var("DEMSTUDIO_BICUBIC_BOUNDS").as_deref()!=Ok("raw")
}
pub(super) fn level_values(level:&MipLevel,origin:[i32;2],size:[u32;2],offset:u32)->Vec<f32>{
    let scale=1i64<<level.level;
    let mut result=vec![0.;level.values.len()];
    for y in 0..level.edge {for x in 0..level.edge {
        let cell=[i64::from(origin[0])+i64::from(x)*scale,i64::from(origin[1])+i64::from(y)*scale];
        let mut lo=[0;2];let mut hi=[0;2];
        for axis in 0..2{
            let max=i64::from(size[axis])-1;
            lo[axis]=(cell[axis]-1-i64::from(offset)).clamp(0,max)/scale;
            hi[axis]=(cell[axis]+scale+1-i64::from(offset)).clamp(0,max)/scale;
        }
        let mut maximum=0f32;
        for gy in lo[1]..=hi[1]{for gx in lo[0]..=hi[0]{
            let ix=((gx*scale-i64::from(origin[0]))/scale).clamp(0,i64::from(level.edge)-1);
            let iy=((gy*scale-i64::from(origin[1]))/scale).clamp(0,i64::from(level.edge)-1);
            maximum=maximum.max(level.values[(iy*i64::from(level.edge)+ix)as usize]);
        }}
        result[(y*level.edge+x)as usize]=maximum;
    }}
    result
}


