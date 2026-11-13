//! Opt-in diagnostic counters. One final readback; no per-Draw CPU sync.
use glow::HasContext;

const LEGACY_COUNTERS: usize = 16;
const ROOT_METRICS: usize = 12;
const ROOT_BUCKETS: usize = 12;
const ROOT_GROUP_STRIDE: usize = ROOT_METRICS + ROOT_BUCKETS;
const COUNTERS: usize = LEGACY_COUNTERS + 2 * ROOT_GROUP_STRIDE;
const BYTES: usize = COUNTERS * 8;
const NAMES: [&str; LEGACY_COUNTERS] = [
    "coarse_bounds", "leaf_bounds", "patches", "root_entries", "monotone_roots",
    "isolated_nodes", "negative_rejects", "maximum_isolation_depth",
    "intersection_queries", "shadow_queries", "vertical_intersections", "ray_polynomials",
    "shadow_endpoint_occlusions", "shadow_negative_rejects", "integrator_invocations", "isolated_subdivisions",
];
const ROOT_NAMES: [&str; ROOT_METRICS] = [
    "polynomial_tasks", "solver_calls", "isolator_entries", "top_monotone_roots",
    "top_precheck_negative_roots", "isolator_top_negative_roots",
    "shadow_endpoint_tasks", "shadow_negative_tasks", "root_v2_roots",
    "logical_splits", "maximum_logical_splits", "reconstruction_splits",
];
const SPLIT_BOUNDS: [(u32, Option<u32>); ROOT_BUCKETS] = [
    (0,Some(0)), (1,Some(1)), (2,Some(3)), (4,Some(7)), (8,Some(15)),
    (16,Some(23)), (24,Some(24)), (25,Some(31)), (32,Some(63)),
    (64,Some(127)), (128,Some(255)), (256,None),
];

pub(super) fn enabled() -> bool {
    std::env::var("DEMSTUDIO_BICUBIC_PROFILE").as_deref() == Ok("on")
}

pub(super) const SHADER_HEADER: &str = r#"
#define BC_PROFILE_COUNTERS
layout(std430,binding=3) buffer BicubicProfile { uint bc_profile_words[]; };
// Legacy 16 counters, then WORLD/SHADOW groups: 12 metrics + 12 split bins.
// Aggregate locally; no per-root atomics and no additional CPU synchronization.
uint bc_profile_local[64]=uint[64](
    0u,0u,0u,0u,0u,0u,0u,0u,0u,0u,0u,0u,0u,0u,0u,0u,
    0u,0u,0u,0u,0u,0u,0u,0u,0u,0u,0u,0u,0u,0u,0u,0u,
    0u,0u,0u,0u,0u,0u,0u,0u,0u,0u,0u,0u,0u,0u,0u,0u,
    0u,0u,0u,0u,0u,0u,0u,0u,0u,0u,0u,0u,0u,0u,0u,0u);
int bc_profile_root_base=16;
uint bc_profile_top_splits=0u,bc_profile_replay_splits=0u;
int bc_profile_split_bucket(uint n){
    if(n==0u)return 0;if(n==1u)return 1;if(n<4u)return 2;
    if(n<8u)return 3;if(n<16u)return 4;if(n<24u)return 5;
    if(n==24u)return 6;if(n<32u)return 7;if(n<64u)return 8;
    if(n<128u)return 9;if(n<256u)return 10;return 11;
}
void bc_profile_task(bool shadow){bc_profile_local[shadow?40:16]++;}
void bc_profile_shadow_fast(int metric){
    bc_profile_local[40+metric]++;bc_profile_local[52]++;
}
uint bc_profile_root_begin(bool shadow){
    bc_profile_root_base=shadow?40:16;bc_profile_local[bc_profile_root_base+1]++;
    bc_profile_top_splits=0u;bc_profile_replay_splits=0u;
    return bc_profile_local[15];
}
void bc_profile_root_note(int metric){bc_profile_local[bc_profile_root_base+metric]++;}
void bc_profile_top_negative(int depth){if(depth==0)bc_profile_root_note(5);}
void bc_profile_root_end(uint before){
    // Legacy counter15 includes general-isolator and RootV2 splits. The original
    // top monotone loop did not increment it; count those splits separately.
    uint splits=bc_profile_local[15]-before+bc_profile_top_splits;
    bc_profile_local[bc_profile_root_base+9]+=splits;
    bc_profile_local[bc_profile_root_base+10]=max(bc_profile_local[bc_profile_root_base+10],splits);
    bc_profile_local[bc_profile_root_base+11]+=bc_profile_replay_splits;
    bc_profile_local[bc_profile_root_base+12+bc_profile_split_bucket(splits)]++;
}
void bc_profile_flush(){
    bc_profile_local[14]++;
    for(int i=0;i<64;i++){
        uint value=bc_profile_local[i];
        if(i==7||i==26||i==50){atomicMax(bc_profile_words[i*2],value);}
        else if(value!=0u){
            uint old=atomicAdd(bc_profile_words[i*2],value);
            if(old>0xffffffffu-value)atomicAdd(bc_profile_words[i*2+1],1u);
        }
    }
}
"#;

fn root_group_report(values:&[u64;COUNTERS],base:usize)->serde_json::Value{
    let mut metrics=serde_json::Map::new();
    for (i,name) in ROOT_NAMES.iter().enumerate(){metrics.insert((*name).into(),serde_json::json!(values[base+i]));}
    let histogram:Vec<_>=SPLIT_BOUNDS.iter().enumerate().map(|(i,(min,max))|
        serde_json::json!({"min":min,"max_inclusive":max,"tasks":values[base+ROOT_METRICS+i]})).collect();
    let histogram_tasks:u64=(0..ROOT_BUCKETS).map(|i|values[base+ROOT_METRICS+i]).sum();
    let tasks=values[base];let calls=values[base+1];
    let ratio=|n:u64,d:u64|if d==0{None}else{Some(n as f64/d as f64)};
    serde_json::json!({"metrics":metrics,"logical_split_histogram":histogram,
        "isolator_fraction_of_solver_calls":ratio(values[base+2],calls),
        "top_monotone_fraction_of_solver_calls":ratio(values[base+3],calls),
        "root_v2_fraction_of_solver_calls":ratio(values[base+8],calls),
        "top_negative_fraction_of_polynomial_tasks":ratio(values[base+4]+values[base+5]+values[base+7],tasks),
        "accounting_valid":histogram_tasks==tasks
            && calls==values[base+2]+values[base+3]+values[base+4]
            && tasks==calls+values[base+6]+values[base+7]
            && values[base+5]<=values[base+2]&&values[base+8]<=values[base+2]})
}

fn report_from_bytes(bytes:&[u8;BYTES])->serde_json::Value{
    let values=std::array::from_fn(|i|{
        let lo=u32::from_ne_bytes(bytes[i*8..i*8+4].try_into().unwrap()) as u64;
        let hi=u32::from_ne_bytes(bytes[i*8+4..i*8+8].try_into().unwrap()) as u64;
        lo|(hi<<32)
    });
    let mut counters=serde_json::Map::new();
    for (i,name) in NAMES.iter().enumerate(){counters.insert((*name).into(),serde_json::json!(values[i]));}
    serde_json::json!({"enabled":true,"diagnostic_only":true,"readback_bytes":BYTES,"counters":counters,
        "root_difficulty":{"schema_version":1,"world":root_group_report(&values,LEGACY_COUNTERS),
            "shadow":root_group_report(&values,LEGACY_COUNTERS+ROOT_GROUP_STRIDE),
            "scope":"all nonvertical polynomial tasks reaching the root stage, including shadow endpoint/negative shortcuts; excludes interval, row/patch rejection and vertical branches; actual primitive entry defines WORLD/SHADOW, including terminal WORLD routed through visibility",
            "classification_scope":"isolator_entries records actual calls, including immediate depth-zero negative returns; top_negative counters use actual root-level branches only; child negative rejects never classify a whole task; RootV2 is an isolator subset, not an exclusive top-level route",
            "split_scope":"per-task logical De Casteljau splits include top-monotone and RootV2 loops; reconstruction_splits separately counts rebuild arithmetic; pre-solver shadow shortcuts have zero splits; no hardware timing or occupancy inference",
            "instrumentation_scope":"full diagnostic population; 64 per-invocation local counters and one final 512-byte readback; instrumented timing is not production throughput"}})
}

pub(super) struct BicubicProfile { buffer: glow::Buffer }
impl BicubicProfile {
    pub unsafe fn create(gl: &glow::Context) -> Result<Self, String> {
        let buffer=gl.create_buffer()?;
        gl.bind_buffer(glow::SHADER_STORAGE_BUFFER,Some(buffer));
        gl.buffer_data_u8_slice(glow::SHADER_STORAGE_BUFFER,&[0u8;BYTES],glow::DYNAMIC_READ);
        let error=gl.get_error();
        gl.bind_buffer(glow::SHADER_STORAGE_BUFFER,None);
        if error!=glow::NO_ERROR {gl.delete_buffer(buffer);return Err(format!("Bicubic 诊断计数分配失败: GL {error:#x}"));}
        let result=Self{buffer};result.bind(gl);Ok(result)
    }
    pub unsafe fn bind(&self,gl:&glow::Context){
        gl.bind_buffer_base(glow::SHADER_STORAGE_BUFFER,3,Some(self.buffer));
    }
    /// Caller reads once after all integration commands, before destroy.
    pub unsafe fn final_read(&self,gl:&glow::Context)->serde_json::Value{
        gl.memory_barrier(glow::SHADER_STORAGE_BARRIER_BIT|glow::BUFFER_UPDATE_BARRIER_BIT);
        let mut bytes=[0u8;BYTES];
        gl.bind_buffer(glow::SHADER_STORAGE_BUFFER,Some(self.buffer));
        gl.get_buffer_sub_data(glow::SHADER_STORAGE_BUFFER,0,&mut bytes);
        gl.bind_buffer(glow::SHADER_STORAGE_BUFFER,None);
        report_from_bytes(&bytes)
    }
    pub unsafe fn destroy(self,gl:&glow::Context){
        gl.bind_buffer_base(glow::SHADER_STORAGE_BUFFER,3,None);gl.delete_buffer(self.buffer);
    }
}


