//! Frozen per-export scheduling/resources. Never mutates process environment.
pub(super) const DEFAULT_GEO_ACCELERATION:bool=true;
fn eligible_route(geo:bool,bilinear:bool,source:bool,image:bool)->bool{geo&&bilinear&&source&&image}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(super) enum SourceExportExecution{Fragment,Compute8x4}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(super) struct SourceExportPolicy{
    pub tight_bounds:bool,
    pub tight_descent:bool,
    pub leaf_gather:bool,
    pub phase_limit:u32,
    pub profile:&'static str,
}
impl SourceExportPolicy{
    /// Derived only from the already frozen profile; never reads environment.
    pub fn execution(self)->SourceExportExecution{
        match self.profile{"accelerated-compute"=>SourceExportExecution::Compute8x4,_=>SourceExportExecution::Fragment}
    }
    pub fn legacy()->Self{Self{
        tight_bounds:std::env::var("DEMSTUDIO_TIGHT_BOUNDS").as_deref()==Ok("on"),
        tight_descent:cfg!(test)&&std::env::var("DEMSTUDIO_DIAGNOSTIC_TIGHT_DESCENT").as_deref()==Ok("on"),
        leaf_gather:std::env::var("DEMSTUDIO_BILINEAR_LEAF_GATHER").as_deref()==Ok("on"),
        phase_limit:32,profile:"original",
    }}
    pub fn legacy_image()->Result<Self,String>{
        let mut policy=Self::legacy();

        Ok(policy)
    }
    pub fn resolve(geo:bool,bilinear:bool,source:bool,image:bool)->Result<Self,String>{
        let profile=std::env::var("DEMSTUDIO_GEO_EXPORT_PROFILE").map_err(|e|match e{
            std::env::VarError::NotPresent=>String::new(),_=>"invalid Geo export profile".into()});
        let profile=match profile{Ok(value)=>Some(value),Err(value)if value.is_empty()=>None,Err(error)=>return Err(error)};
        let explicit=std::env::vars_os().any(|(key,_)|key.to_str().is_some_and(experimental_variable));
        let legacy=if bilinear&&image{Self::legacy_image()?}else{Self::legacy()};
        Self::resolve_with(legacy,profile.as_deref(),eligible_route(geo,bilinear,source,image),explicit)
    }
    fn resolve_with(legacy:Self,profile:Option<&str>,eligible:bool,explicit:bool)->Result<Self,String>{
        let requested=match profile{None=>DEFAULT_GEO_ACCELERATION,Some("original")=>false,
            Some("accelerated"|"accelerated-compute")=>true,_=>return Err("invalid Geo export profile".into())};
        if requested&&eligible&&!explicit{
            let profile=if profile.is_none()||profile==Some("accelerated-compute"){"accelerated-compute"}else{"accelerated"};
            Ok(Self{tight_bounds:true,tight_descent:true,leaf_gather:true,phase_limit:64,profile})
        }else{Ok(legacy)}
    }
}
fn experimental_variable(name:&str)->bool{
    let upper=name.to_ascii_uppercase();
    let name=upper.as_str();
    name.starts_with("DEMSTUDIO_DIAGNOSTIC_")||matches!(name,
        "DEMSTUDIO_TIGHT_BOUNDS"|"DEMSTUDIO_BILINEAR_LEAF_GATHER"|"DEMSTUDIO_BILINEAR_PHASE_LIMIT"|
        "DEMSTUDIO_BICUBIC_PHASE_LIMIT"|"DEMSTUDIO_GEO_MERGED"|"DEMSTUDIO_ANCESTOR_SEED"|
        "DEMSTUDIO_DIRECTIONAL_SKIP"|"DEMSTUDIO_PRIMARY_SUN_CERT"|"DEMSTUDIO_TERMINAL_VISIBILITY"|
        "DEMSTUDIO_LAZY_BOUNDARY"|"DEMSTUDIO_ASYNC_SAMPLES"|"DEMSTUDIO_PHASE_QUEUE"|
        "DEMSTUDIO_PHASE_COMPUTE"|"DEMSTUDIO_SAMPLE_WAVEFRONT"|"DEMSTUDIO_VISIBILITY_PROBE"|
        "DEMSTUDIO_QUERY_ONLY"|"DEMSTUDIO_WALK_PROFILE"|"DEMSTUDIO_WALK_CLOCK"|
        "DEMSTUDIO_INTEGRATION_WORK"|"DEMSTUDIO_RAYSTATE"|"DEMSTUDIO_INTEGRATOR"|
        "DEMSTUDIO_IMAGE_STATIC_STATE"|"DEMSTUDIO_BILINEAR_SHADOW_PRECISION"|
        "DEMSTUDIO_BILINEAR_STABLE_ROOT"|"DEMSTUDIO_COHERENT_QUERY")
        ||name.starts_with("DEMSTUDIO_BILINEAR_")||name.starts_with("DEMSTUDIO_BICUBIC_")||name.starts_with("DEMSTUDIO_ROOT_")
        ||name.starts_with("DEMSTUDIO_ASYNC_")||name.starts_with("DEMSTUDIO_WAVEFRONT_")
}
