use serde::Deserialize;
use serde_json::{json, Value};
use std::{collections::HashMap, fs, io::{self, BufRead, BufReader, Read, Write}, net::{TcpListener, TcpStream}, path::PathBuf, sync::{Arc, Mutex, OnceLock, mpsc::{self, SyncSender}, atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering}}, thread, time::Duration};
use tauri::{Manager, Emitter};

const REQUEST_LIMIT: usize = 1024 * 1024;
const RESPONSE_LIMIT: usize = 32 * 1024 * 1024;
const ACTIONS: &[&str] = &["get_scene","patch_scene","get_export_settings","patch_export_settings","export","job_status","cancel_job","import_dem","import_texture","list_views","save_view","apply_view","delete_view","get_layout","set_layout","export_layout","get_video","set_video","apply_preset","list_presets","save_preset","pause","restore_snapshot","capture_snapshot","inspect_scene","set_view_frame","design_views","set_view_sequence","fit_view","focus_region","begin_control","end_control","get_control_state"];
type Reply = Result<Value, String>;
static PENDING: OnceLock<Mutex<HashMap<String,SyncSender<Reply>>>> = OnceLock::new();
static REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static CONTROL_READY: AtomicBool = AtomicBool::new(false);
fn pending() -> &'static Mutex<HashMap<String,SyncSender<Reply>>> { PENDING.get_or_init(||Mutex::new(HashMap::new())) }
struct Bridge { stop: Arc<AtomicBool>, descriptor: PathBuf }
static BRIDGE: Mutex<Option<Bridge>> = Mutex::new(None);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request { token: String, method: String, #[serde(default)] args: Option<ControlArgs> }
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlArgs { action: String, payload: Value }

fn authenticate(line: &[u8], token: &str) -> Result<Request, String> {
    if line.len() > REQUEST_LIMIT { return Err("请求超过1 MiB限制".into()); }
    let request: Request = serde_json::from_slice(line).map_err(|_| "请求JSON无效".to_string())?;
    let supplied = request.token.as_bytes();
    let expected = token.as_bytes();
    let difference = supplied.iter().zip(expected).fold(supplied.len() ^ expected.len(), |difference, (a,b)| difference | usize::from(a ^ b));
    if difference != 0 { return Err("认证失败".into()); }
    match request.method.as_str() {
        "status" | "capture_png" if request.args.is_none() => {},
        "control" => { let args=request.args.as_ref().ok_or("缺少控制参数")?;if !ACTIONS.contains(&args.action.as_str()) { return Err("不支持的控制动作".into()); } if !args.payload.is_object() { return Err("控制payload须为对象".into()); } },
        _ => return Err("不支持的方法或参数".into()),
    }
    Ok(request)
}

fn read_request(mut reader: impl BufRead) -> Result<Vec<u8>, String> {
    let mut line = Vec::new();
    reader.by_ref().take((REQUEST_LIMIT + 1) as u64).read_until(b'\n', &mut line).map_err(|e| format!("读取请求失败：{e}"))?;
    if line.len() > REQUEST_LIMIT { return Err("请求超过1 MiB限制".into()); }
    if line.last() != Some(&b'\n') { return Err("请求须以换行结束".into()); }
    Ok(line)
}

struct LimitedResponse(Vec<u8>);
impl Write for LimitedResponse {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.0.len().saturating_add(bytes.len()) >= RESPONSE_LIMIT { return Err(io::Error::new(io::ErrorKind::InvalidData, "响应超过32 MiB限制")); }
        self.0.extend_from_slice(bytes); Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}
fn response_bytes(value: &Value) -> Result<Vec<u8>, String> {
    let mut output = LimitedResponse(Vec::new());
    serde_json::to_writer(&mut output, value).map_err(|_| "响应超过32 MiB限制".to_string())?;
    output.0.push(b'\n'); Ok(output.0)
}
fn control_with_timeout(action: &str, payload: Value, timeout: Duration, emit: impl FnOnce(Value)->Result<(),String>) -> Reply {
    let id=format!("{}-{}",std::process::id(),REQUEST_SEQUENCE.fetch_add(1,Ordering::Relaxed));
    let expires_at=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|e|e.to_string())?.as_millis().saturating_add(timeout.as_millis());
    let (sender,receiver)=mpsc::sync_channel(1);
    pending().lock().map_err(|e|e.to_string())?.insert(id.clone(),sender);
    let sent=emit(json!({"id":id,"action":action,"payload":payload,"expiresAt":expires_at.min(u64::MAX as u128) as u64}));
    let reply=match sent {
        Err(error)=>Err(format!("控制请求未送达：{error}")),
        Ok(())=>receiver.recv_timeout(timeout).unwrap_or_else(|_|Err(format!("控制请求{id}未在期限内收到回执；执行结果未知，请查询状态后再操作"))),
    };
    pending().lock().map_err(|e|e.to_string())?.remove(&id);
    reply
}
#[tauri::command]
pub fn mcp_control_reply(id: String, result: Option<Value>, error: Option<String>) -> Result<(),String> {
    let sender=pending().lock().map_err(|e|e.to_string())?.remove(&id).ok_or("请求不存在或已超时")?;
    let reply=match error { Some(error)=>Err(error),None=>Ok(result.unwrap_or(Value::Null)) };
    sender.try_send(reply).map_err(|_|"请求已结束，回执未接收".into())
}
#[tauri::command]
pub fn mcp_control_ready() -> Result<(),String> { CONTROL_READY.store(true,Ordering::Release);Ok(()) }
fn dispatch_request(app: &tauri::AppHandle, request: Request) -> Reply {
    if request.method=="control" {
        if !CONTROL_READY.load(Ordering::Acquire) { return Err("应用控制界面尚未就绪；请求未执行".into()); }
        let args=request.args.ok_or("缺少控制参数")?;
        return control_with_timeout(&args.action,args.payload,Duration::from_secs(15),|value|app.emit("mcp-control-request",value).map_err(|e|e.to_string()));
    }
    dispatch(app,&request.method)
}
fn dispatch(app: &tauri::AppHandle, method: &str) -> Result<Value, String> {
    let runtime = super::handle(&app.state::<super::Runtime>());
    if method == "status" {
        return match runtime {
            Ok(handle) => {let mut status=serde_json::to_value(handle.status()).map_err(|e|e.to_string())?;status["controlReady"]=json!(CONTROL_READY.load(Ordering::Acquire));Ok(status)},
            Err(error) => Ok(json!({"ready":false,"controlReady":CONTROL_READY.load(Ordering::Acquire),"error":error})),
        };
    }
    let handle = runtime?;
    match method {
        "capture_png" => {
            if handle.status().exporting { return Err("正在导出".into()); }
            let (pixels, width, height, mut metadata) = handle.capture_frame_with_metadata()?;
            metadata["instancePid"]=json!(std::process::id());
            metadata["imageWidth"]=json!(width);metadata["imageHeight"]=json!(height);
            metadata["frameId"]=json!(format!("{}:{}:{}",std::process::id(),metadata["frameGeneration"],metadata["frameSequence"]));
            metadata["capturedAtUnixMs"]=json!(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|e|e.to_string())?.as_millis() as u64);
            metadata["identityAvailable"]=json!(true);
            // The output framebuffer contains scene pixels only, never the WebView HUD.
            metadata["hudIncluded"]=json!(false);
            let png = super::encode_capture_png(pixels, width, height)?;
            if png.len() > RESPONSE_LIMIT / 4 - 1024 { return Err("捕获PNG超过桥接响应容量".into()); }
            let _=app.emit("mcp-control-activity",json!({}));
            Ok(json!({"png": png,"metadata":metadata}))
        }
        _ => Err("不支持的方法".into()),
    }
}
fn serve(mut stream: TcpStream, app: &tauri::AppHandle, token: &str) {
    // Accepted sockets may inherit the nonblocking listener mode on Windows.
    if stream.set_nonblocking(false).is_err() { return; }
    if stream.set_read_timeout(Some(Duration::from_secs(60))).and_then(|_| stream.set_write_timeout(Some(Duration::from_secs(5)))).is_err() { return; }
    let _ = stream.set_nodelay(true);
    let reader_stream = match stream.try_clone() { Ok(value)=>value, Err(_)=>return };
    let mut reader = BufReader::new(reader_stream);
    loop {
        // Preserve buffered bytes across requests; authenticate every message.
        match reader.fill_buf() { Ok(bytes) if bytes.is_empty()=>break, Err(_)=>break, _=>{} }
        let result = read_request(&mut reader).and_then(|line| authenticate(&line, token)).and_then(|request| dispatch_request(app, request));
        let failed = result.is_err();
        let response = match result { Ok(result) => json!({"ok":true,"result":result}), Err(error) => json!({"ok":false,"error":error}) };
        let bytes = response_bytes(&response).unwrap_or_else(|error| format!("{{\"ok\":false,\"error\":{}}}\n", serde_json::to_string(&error).unwrap()).into_bytes());
        if stream.write_all(&bytes).is_err() || failed { break; }
    }
}

fn session_token() -> Result<String, String> {
    #[link(name="bcrypt")]
    unsafe extern "system" { fn BCryptGenRandom(algorithm: isize, buffer: *mut u8, length: u32, flags: u32) -> i32; }
    let mut bytes = [0u8;32];
    let status = unsafe { BCryptGenRandom(0, bytes.as_mut_ptr(), bytes.len() as u32, 2) };
    if status < 0 { return Err(format!("生成桥接凭证失败：{status:#x}")); }
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub fn start(app: tauri::AppHandle) -> Result<(), String> {
    let mut bridge = BRIDGE.lock().map_err(|e| e.to_string())?;
    if bridge.is_some() { return Ok(()); }
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).map_err(|e| e.to_string())?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let token = session_token()?;
    let directory = PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA不可用")?).join("DemStudio").join("mcp");
    fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let descriptor = directory.join(format!("{}.json", std::process::id()));
    fs::write(&descriptor, serde_json::to_vec(&json!({"pid":std::process::id(),"port":port,"token":token,"transportVersion":2,"executable":std::env::current_exe().ok()})).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let stop = Arc::new(AtomicBool::new(false));
    let signal = stop.clone();
    if let Err(error) = thread::Builder::new().name("mcp-bridge".into()).spawn(move || {
        let active=Arc::new(AtomicUsize::new(0));
        while !signal.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((mut stream, peer)) if peer.ip().is_loopback() => {
                    if active.load(Ordering::Acquire)>=4 {let _=stream.set_write_timeout(Some(Duration::from_secs(1)));let _=stream.write_all(b"{\"ok\":false,\"error\":\"bridge_busy\"}\n");continue;}
                    active.fetch_add(1,Ordering::AcqRel);
                    let app=app.clone();let token=token.clone();let count=active.clone();
                    if thread::Builder::new().name("mcp-request".into()).spawn(move||{struct Release(Arc<AtomicUsize>);impl Drop for Release{fn drop(&mut self){self.0.fetch_sub(1,Ordering::AcqRel);}}let _release=Release(count);serve(stream,&app,&token);}).is_err(){active.fetch_sub(1,Ordering::AcqRel);}
                },
                Ok(_) => {},
                Err(error) if error.kind()==io::ErrorKind::WouldBlock => thread::sleep(Duration::from_millis(50)),
                Err(_) => break,
            }
        }
    }) { let _ = fs::remove_file(&descriptor); return Err(error.to_string()); }
    *bridge = Some(Bridge { stop, descriptor }); Ok(())
}

pub fn stop() {
    CONTROL_READY.store(false,Ordering::Release);
    if let Ok(mut bridge) = BRIDGE.lock() { if let Some(bridge) = bridge.take() { bridge.stop.store(true, Ordering::Release); let _ = fs::remove_file(bridge.descriptor); } }
    if let Ok(mut requests)=pending().lock(){requests.clear();}
}
