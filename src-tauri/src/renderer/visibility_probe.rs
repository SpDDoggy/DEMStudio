//! Bounded experimental coarse classifier. Does not certify original float bi_interval.
use super::visibility_bounds::BoundsLevel;
pub(super) const SHADER_HEADER:&str=include_str!("visibility_probe.glsl");
#[derive(Clone,Copy,Debug,PartialEq,Eq)]#[repr(u32)]pub(super) enum Classification{Clear=0,Occluded=1,Unknown=2}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]pub(super) struct ProbeResult{pub classification:Classification,pub nodes:u32}
#[derive(Clone,Copy,Debug)]pub(super) struct ProbeParams{
    pub padded:u32,pub size:[u32;2],pub geo_offset:u32,
    pub height_scale:f32,pub height_offset:f32,pub quant_step:f32,
    pub volume_min:[f32;3],pub volume_max:[f32;3],
    pub origin:[f32;3],pub direction:[f32;3],pub interval:[f32;2],pub node_budget:u32,
}
fn margin(m:f32)->f32{1e-5f32.max(m*7.62939453125e-6)}
fn clip(p:&ProbeParams,lo:[f32;3],hi:[f32;3],segment:&mut [f32;2])->u32{
    let mut near=[0.;3];let mut far=[0.;3];for a in 0..3{let x=(lo[a]-p.origin[a])/p.direction[a];let y=(hi[a]-p.origin[a])/p.direction[a];near[a]=x.min(y);far[a]=x.max(y);}
    segment[0]=segment[0].max(near[0].max(near[1].max(near[2])));segment[1]=segment[1].min(far[0].min(far[1].min(far[2])));
    if segment.iter().any(|v|!v.is_finite()){return 2;}
    let m=margin(segment[0].abs().max(segment[1].abs()));if segment[0]>segment[1]+m{0}else if segment[1]<=segment[0]+m{2}else{1}
}
pub(super) fn classify_cpu(levels:&[BoundsLevel],p:&ProbeParams)->ProbeResult{
    let mut result=ProbeResult{classification:Classification::Unknown,nodes:0};
    if !p.padded.is_power_of_two()||p.padded<32||p.padded.trailing_zeros()>20||p.size.contains(&0)||p.geo_offset>1
        ||u64::from(p.padded)<u64::from(p.size[0].max(p.size[1]))+2*u64::from(p.geo_offset)
        ||p.node_budget==0||p.height_scale<0.||p.quant_step<0.
        ||p.origin.iter().chain(&p.direction).chain(&p.interval).chain(&p.volume_min).chain(&p.volume_max).chain([&p.height_scale,&p.height_offset,&p.quant_step]).any(|v|!v.is_finite())
        ||p.direction.contains(&0.){return result;}
    let top=p.padded.trailing_zeros();
    if levels.len()!=(top-4)as usize||levels.iter().enumerate().any(|(i,l)|l.level!=i as u32+5||l.edge!=p.padded>>(i+5)||l.values.len()!=l.edge as usize*l.edge as usize){return result;}
    let lower=[p.volume_min[0].max(0.),p.volume_min[1].max(0.),p.volume_min[2]];
    let upper=[p.volume_max[0].min((p.size[0]+2*p.geo_offset)as f32),p.volume_max[1].min((p.size[1]+2*p.geo_offset)as f32),p.volume_max[2]];
    if (0..3).any(|a|lower[a]>=upper[a]){return result;}
    let mut interval=p.interval;if interval[1]<0.{interval[1]=f32::MAX;}
    match clip(p,lower,upper,&mut interval){0=>{result.classification=Classification::Clear;return result;},2=>return result,_=>{}}
    let mut stack=vec![[0u32,0,top]];let mut uncertain=false;
    while let Some(node)=stack.pop(){
        if result.nodes>=p.node_budget{return result;}result.nodes+=1;
        let scale=(1u32<<node[2])as f32;
        let lo=[lower[0].max(node[0]as f32*scale),lower[1].max(node[1]as f32*scale),lower[2]];
        let hi=[upper[0].min((node[0]+1)as f32*scale),upper[1].min((node[1]+1)as f32*scale),upper[2]];
        if (0..3).any(|a|lo[a]>=hi[a]){continue;}
        let mut segment=interval;match clip(p,lo,hi,&mut segment){0=>continue,2=>{uncertain=true;continue;},_=>{}}
        let level=&levels[(node[2]-5)as usize];let mut bounds=level.values[(node[1]*level.edge+node[0])as usize];
        if bounds.iter().any(|v|!v.is_finite())||bounds[0]>bounds[1]{uncertain=true;continue;}
        for b in &mut bounds{*b*=p.height_scale;if p.quant_step>0.{*b=(*b/p.quant_step).ceil()*p.quant_step;}*b+=p.height_offset;}
        let z=[p.origin[2]+p.direction[2]*segment[0],p.origin[2]+p.direction[2]*segment[1]];
        if bounds.iter().chain(&z).any(|v|!v.is_finite()){uncertain=true;continue;}
        let m=margin(bounds[0].abs().max(bounds[1].abs()).max(z[0].abs().max(z[1].abs())));
        if z[0].min(z[1])>bounds[1]+m{continue;}
        if bounds[0]>0.&&z[0].max(z[1])<bounds[0]-m{result.classification=Classification::Occluded;return result;}
        if node[2]==5{uncertain=true;continue;}
        if stack.len()+4>64{return result;}
        for child in (0..4).rev(){stack.push([node[0]*2+(child&1),node[1]*2+(child>>1),node[2]-1]);}
    }
    result.classification=if uncertain{Classification::Unknown}else{Classification::Clear};result
}


/// Sets probe uniforms and unit12 coarse texture on the caller's active program.
/// Does not alter any ray state or integrate the classifier into query execution.
pub(super) unsafe fn bind(gl:&glow::Context,program:glow::Program,texture:glow::Texture,p:&ProbeParams){
    use glow::HasContext;
    let active=gl.get_parameter_i32(glow::ACTIVE_TEXTURE)as u32;gl.active_texture(glow::TEXTURE0+12);gl.bind_texture(glow::TEXTURE_2D,Some(texture));gl.active_texture(active);
    for(name,value)in[("vp_bounds",12),("vp_top",p.padded.trailing_zeros()as i32),("vp_padded",p.padded as i32),("vp_geo_offset",p.geo_offset as i32)]{gl.uniform_1_i32(gl.get_uniform_location(program,name).as_ref(),value);}
    gl.uniform_1_u32(gl.get_uniform_location(program,"vp_budget").as_ref(),p.node_budget);
    gl.uniform_2_i32(gl.get_uniform_location(program,"vp_size").as_ref(),p.size[0]as i32,p.size[1]as i32);
    for(name,v)in[("vp_height",[p.height_scale,p.height_offset,p.quant_step]),("vp_volume_min",p.volume_min),("vp_volume_max",p.volume_max)]{gl.uniform_3_f32(gl.get_uniform_location(program,name).as_ref(),v[0],v[1],v[2]);}
}

