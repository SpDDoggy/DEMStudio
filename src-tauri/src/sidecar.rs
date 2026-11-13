use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fs::{self, OpenOptions}, io::{Read, Write}, path::{Path, PathBuf}, sync::{Mutex, atomic::{AtomicU64, Ordering}}};

pub const MAX_BYTES:u64=1024*1024;
const SCHEMA:&str="demstudio.source-settings";
static SERIAL:AtomicU64=AtomicU64::new(0);
#[derive(Default)]
pub struct SidecarIo(pub Mutex<()>);
#[derive(Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {schema:String,version:u32,settings:Value}
fn validate(value:&Value)->Result<(),String>{
    let d:Document=serde_json::from_value(value.clone()).map_err(|_|"设置快照格式无效")?;
    if d.schema!=SCHEMA || d.version!=1 || !d.settings.is_object(){return Err("设置快照版本无效".into());}
    Ok(())
}
pub fn checked_path(current:Option<&Path>,requested:&str)->Result<PathBuf,String>{
    let current=current.ok_or("请先导入图像")?;
    let requested=fs::canonicalize(requested).map_err(|e|e.to_string())?;
    if requested!=fs::canonicalize(current).map_err(|e|e.to_string())?{return Err("设置快照的源图已切换".into());}
    let mut name=current.as_os_str().to_os_string();name.push(".ds");Ok(PathBuf::from(name))
}
pub fn load(path:&Path)->Result<Option<Value>,String>{
    let file=match fs::File::open(path){Ok(f)=>f,Err(e) if e.kind()==std::io::ErrorKind::NotFound=>return Ok(None),Err(e)=>return Err(e.to_string())};
    let mut bytes=Vec::new();file.take(MAX_BYTES+1).read_to_end(&mut bytes).map_err(|e|e.to_string())?;
    if bytes.len() as u64>MAX_BYTES{return Err("设置快照过大".into());}
    let value=serde_json::from_slice(&bytes).map_err(|_|"设置快照格式无效")?;validate(&value)?;Ok(Some(value))
}
pub fn save(path:&Path,value:Value)->Result<(),String>{
    validate(&value)?;
    let bytes=serde_json::to_vec_pretty(&value).map_err(|e|e.to_string())?;
    if bytes.len() as u64>MAX_BYTES{return Err("设置快照过大".into());}
    let mut temp_name=path.as_os_str().to_os_string();
    temp_name.push(format!(".tmp-{}-{}",std::process::id(),SERIAL.fetch_add(1,Ordering::Relaxed)));
    let temp=PathBuf::from(temp_name);
    let result=(||{
        let mut file=OpenOptions::new().write(true).create_new(true).open(&temp).map_err(|e|e.to_string())?;
        file.write_all(&bytes).and_then(|_|file.sync_all()).map_err(|e|e.to_string())?;drop(file);
        replace(&temp,path).map_err(|e|e.to_string())
    })();
    if result.is_err(){let _=fs::remove_file(&temp);}result
}
#[cfg(windows)]
fn replace(from:&Path,to:&Path)->std::io::Result<()>{
    use std::os::windows::ffi::OsStrExt;
    #[link(name="kernel32")]
    extern "system"{fn MoveFileExW(from:*const u16,to:*const u16,flags:u32)->i32;}
    let from:Vec<u16>=from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to:Vec<u16>=to.as_os_str().encode_wide().chain(Some(0)).collect();
    if unsafe{MoveFileExW(from.as_ptr(),to.as_ptr(),1|8)}==0{Err(std::io::Error::last_os_error())}else{Ok(())}
}
#[cfg(not(windows))]
fn replace(from:&Path,to:&Path)->std::io::Result<()>{fs::rename(from,to)}


