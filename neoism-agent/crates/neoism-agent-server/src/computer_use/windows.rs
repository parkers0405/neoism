//! Explicit foreground targeting. Tokens are snapshots, not application sandboxes.
use anyhow::{Context, ensure};
use serde::Serialize;
use std::sync::Mutex;
#[derive(Clone, Debug, Serialize)]
pub(super) struct Window {
    #[serde(serialize_with="serialize_native_id")]
    pub id:String,
    pub pid:u32, pub app:String, pub title:String,
    pub x:i32, pub y:i32, pub width:u32, pub height:u32, pub focused:bool,
}
fn serialize_native_id<S:serde::Serializer>(id:&str,serializer:S)->Result<S::Ok,S::Error> {
    // Preserve the public native-ID shape; lifetime evidence is internal only.
    serializer.serialize_str(id.split(['|','#']).next().unwrap_or_default())
}
// `id` is a qualified native identity: native ID | lifetime evidence # snapshot
// nonce. Keep it self-contained so retained frames cannot alias a reopened window.
// Only native_id() may be passed to OS APIs. Process start guards PID reuse, NOT
// same-process window-ID reuse; only Hyprland stableId supplies that evidence.
const MAX_TARGETS:usize=1024;
#[derive(Clone)]
struct Token { token:String, window:Window, epoch:u64 }
static TOKENS: Mutex<Vec<Token>> = Mutex::new(Vec::new());
fn native_id(w:&Window)->&str { identity_id(w).split('|').next().unwrap_or_default() }
fn identity_id(w:&Window)->&str { w.id.split('#').next().unwrap_or_default() }
fn reconcile(tokens:&mut Vec<Token>,windows:Vec<Window>,epoch:u64)->Vec<(String,Window)> {
    // A complete successful enumeration is the eviction boundary. Never keep
    // absent windows or old geometry revisions; never recycle opaque tokens.
    let old=std::mem::take(tokens);
    for mut window in windows.into_iter().take(MAX_TARGETS) {
        if tokens.iter().any(|t|native_id(&t.window)==native_id(&window)) { continue; }
        let token=old.iter().find(|t|t.epoch==epoch && same_identity(&t.window,&window)
            && bounds(&t.window)==bounds(&window)).map(|t|t.token.clone())
            .unwrap_or_else(||format!("{:032x}",rand::random::<u128>()));
        window.id=format!("{}#{token}",identity_id(&window));
        tokens.push(Token{token,window,epoch});
    }
    tokens.iter().map(|t|(t.token.clone(),t.window.clone())).collect()
}
pub(super) fn list() -> anyhow::Result<serde_json::Value> {
    let epoch=super::STOP.load(std::sync::atomic::Ordering::SeqCst);
    let windows=native_list()?;
    let mut tokens=TOKENS.lock().map_err(|_|anyhow::anyhow!("Window state poisoned"))?;
    ensure!(epoch==super::STOP.load(std::sync::atomic::Ordering::SeqCst),"Window listing revoked by stop");
    let windows=reconcile(&mut tokens,windows,epoch);
    Ok(serde_json::json!({"windows":windows.iter().map(|(token,w)|serde_json::json!({"target":token,"window":w})).collect::<Vec<_>>(),"coordinates":"desktop logical bounds; input still requires display screenshot image pixels, NOT window-relative coordinates","input":"foreground only; focus can race external user/apps; no sandbox or allowlist guarantee","tokens":"stable for unchanged listed identity and geometry; revoked by absence, changed snapshot, stop or bounded cache eviction; identity revalidated before use; screenshot frames still expire after 30 seconds","targetCapacity":MAX_TARGETS}))
}
pub(super) fn resolve(token:&str) -> anyhow::Result<Window> {
    resolve_from(&TOKENS.lock().map_err(|_|anyhow::anyhow!("Window state poisoned"))?,token,super::STOP.load(std::sync::atomic::Ordering::SeqCst))
}
fn resolve_from(tokens:&[Token],token:&str,epoch:u64)->anyhow::Result<Window> {
    tokens.iter().find(|t|t.token==token && t.epoch==epoch).map(|t|t.window.clone()).context("Unknown/stale window target; list windows again")
}
fn same_identity(a:&Window,b:&Window)->bool { identity_id(a)==identity_id(b) && a.pid==b.pid && a.app==b.app }
pub(super) fn process_birth(w:&Window)->anyhow::Result<String> { process_start(w.pid) }
pub(super) fn same_process(a:&Window,b:&Window)->anyhow::Result<bool> {
    Ok(a.pid==b.pid && a.app==b.app && process_birth(a)?==process_birth(b)?)
}
fn bounds(w:&Window)->(i32,i32,u32,u32) { (w.x,w.y,w.width,w.height) }
pub(super) fn same_target(a:Option<&Window>,b:Option<&Window>)->bool {
    match (a,b) {
        (None,None)=>true,
        (Some(a),Some(b))=>a.id==b.id && same_identity(a,b) && bounds(a)==bounds(b),
        _=>false,
    }
}
fn revoke(target:&Window) {
    if let Ok(mut tokens)=TOKENS.lock() { tokens.retain(|t|t.window.id!=target.id); }
}
pub(super) fn validate(target:&Window, foreground:bool)->anyhow::Result<Window> {
    if let Some((_,token))=target.id.split_once('#') { resolve(token)?; }
    #[cfg(target_os="linux")]
    if foreground { return validate_active_with(target,request,hypr_identity); }
    let windows=native_list()?;
    if !windows.iter().any(|w|same_identity(w,target) && bounds(w)==bounds(target)) { revoke(target); }
    validate_list(target,foreground,windows)
}
pub(super) fn validate_list(target:&Window,foreground:bool,windows:Vec<Window>)->anyhow::Result<Window> {
    let mut current=windows.into_iter().find(|w|same_identity(w,target)).context("Target window disappeared or identity changed")?;
    ensure!(bounds(&current)==bounds(target),"Target window moved/resized; list windows and take another screenshot");
    if foreground {
        ensure!(current.focused,"Target is not foreground; observe/focus explicitly before input");
    }
    // Preserve the observation nonce for screenshot/selector binding.
    current.id=target.id.clone();
    Ok(current)
}
pub(super) fn condition(target:&Window,closed:bool)->anyhow::Result<bool> {
    condition_with(target,closed,native_exists,native_list)
}
fn condition_with(target:&Window,closed:bool,exists:impl FnOnce(&Window)->anyhow::Result<bool>,list:impl FnOnce()->anyhow::Result<Vec<Window>>)->anyhow::Result<bool> {
    // Visibility/Space membership is not lifetime. Closed never consults the
    // presentation list (nor AX focus), which can legitimately be empty.
    if closed {
        return exists(target).map(|exists| { if !exists { revoke(target); } !exists });
    }
    let windows=list()?;
    if !windows.iter().any(|w|same_identity(w,target)) { revoke(target); }
    Ok(windows.into_iter().any(|w|same_identity(&w,target) && w.focused))
}
pub(super) fn focus(token:&str,mut check:impl FnMut()->anyhow::Result<()>)->anyhow::Result<()> {
    let target=resolve(token)?;
    if !native_exists(&target)? { revoke(&target); anyhow::bail!("Target window closed or identity changed"); }
    check()?;
    native_focus(&target,&mut check)?;
    check()?;
    // OS focus requests are not guaranteed; report only confirmed foreground.
    let current=validate(&target,false)?;
    check()?;
    ensure!(current.focused,"OS did not confirm foreground focus; observe again");
    Ok(())
}

#[cfg(target_os="linux")]
fn native_exists(target:&Window)->anyhow::Result<bool> {
    let clients:Vec<serde_json::Value>=serde_json::from_str(&request("j/clients")?)?;
    // No mapped/hidden/active-workspace filter and no foreground query.
    for client in clients {
        let id=client["address"].as_str().context("Missing client identity")?;
        if id==native_id(target) {
            return Ok(hypr_identity(&client)?==identity_id(target)
                && client["class"].as_str().unwrap_or_default()==target.app);
        }
    }
    Ok(false)
}
#[cfg(target_os="windows")]
fn native_exists(target:&Window)->anyhow::Result<bool> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{IsWindow,GetWindowThreadProcessId};
    let hwnd=native_id(target).parse::<usize>()? as _;
    if unsafe { IsWindow(hwnd) }==0 { return Ok(false); }
    let mut pid=0;
    let thread=unsafe { GetWindowThreadProcessId(hwnd,&mut pid) };
    if thread==0 {
        if unsafe { IsWindow(hwnd) }==0 { return Ok(false); }
        anyhow::bail!("Cannot read native window owner: {}",std::io::Error::last_os_error());
    }
    Ok(pid==target.pid && qualified_identity(native_id(target),pid)?==identity_id(target))
}
#[cfg(target_os="macos")]
fn native_exists(target:&Window)->anyhow::Result<bool> { macos::exists(target) }
#[cfg(not(any(target_os="linux",target_os="macos",target_os="windows")))]
fn native_exists(_: &Window)->anyhow::Result<bool> { anyhow::bail!("Window existence probe unsupported") }

// AX no-value is normal (e.g. Finder desktop). Permission, unsupported-attribute,
// messaging and invalid-element errors must remain errors, not missing focus.
#[cfg(any(target_os="macos",test))]
fn ax_optional_value(status:i32,has_value:bool)->anyhow::Result<bool> {
    match (status,has_value) {
        (0,true)=>Ok(true),
        (-25212,false)=>Ok(false), // kAXErrorNoValue
        _=>anyhow::bail!("AX attribute query failed (status {status}, value={has_value}); check Accessibility permission"),
    }
}

#[cfg(target_os="linux")]
fn request(command:&str)->anyhow::Result<String> {
    super::latency::count("window_ipc_requests",1);
    super::latency::measure("window_ipc",||request_inner(command))
}
#[cfg(target_os="linux")]
fn request_inner(command:&str)->anyhow::Result<String> {
    use std::{io::{Read,Write},os::unix::net::UnixStream,time::Duration};
    let signature=std::env::var("HYPRLAND_INSTANCE_SIGNATURE").context("Window targeting currently requires Hyprland native IPC on Linux")?;
    ensure!(!signature.is_empty() && signature.bytes().all(|c| c.is_ascii_alphanumeric() || c==b'_' || c==b'-'),"Invalid Hyprland instance signature");
    let path=std::path::PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").context("No XDG_RUNTIME_DIR")?).join("hypr").join(signature).join(".socket.sock");
    let mut stream=UnixStream::connect(path)?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    stream.write_all(command.as_bytes())?;
    let mut result=String::new();
    stream.take(4*1024*1024+1).read_to_string(&mut result)?;
    ensure!(result.len()<=4*1024*1024,"Hyprland reply exceeds limit");
    Ok(result)
}
#[cfg(target_os="linux")]
fn native_list()->anyhow::Result<Vec<Window>> {
    let clients:Vec<serde_json::Value>=serde_json::from_str(&request("j/clients")?)?;
    let active:serde_json::Value=serde_json::from_str(&request("j/activewindow")?)?;
    clients.into_iter().filter(|v|v["mapped"].as_bool()==Some(true)).map(|v| {
        let identity=hypr_identity(&v)?;
        let focused=active["address"]==v["address"] && hypr_identity(&active)?==identity;
        hypr_window(&v,focused,identity)
    }).collect()
}
#[cfg(target_os="linux")]
fn hypr_identity(v:&serde_json::Value)->anyhow::Result<String> { hypr_identity_with(v,process_start) }
#[cfg(any(target_os="linux",test))]
fn hypr_identity_with(v:&serde_json::Value,start:impl FnOnce(u32)->anyhow::Result<String>)->anyhow::Result<String> {
    let id=v["address"].as_str().context("Missing window address")?;
    ensure!(id.starts_with("0x") && id.len()>2 && id.len()<=18 && id[2..].bytes().all(|c|c.is_ascii_hexdigit()),"Invalid native window address");
    let pid=u32::try_from(v["pid"].as_u64().context("Missing window PID")?)?;
    let stable=match v.get("stableId") {
        None|Some(serde_json::Value::Null)=>"unavailable".to_owned(),
        Some(serde_json::Value::Number(n)) if n.as_u64().is_some()=>format!("number:{n}"),
        Some(serde_json::Value::String(s)) if !s.is_empty() && s.len()<=128 && s.bytes().all(|c|c.is_ascii_alphanumeric() || c==b'-')=>format!("string:{s}"),
        _=>anyhow::bail!("Invalid Hyprland stable identity"),
    };
    Ok(format!("{id}|{pid}|{}|{stable}",start(pid)?))
}
#[cfg(target_os="linux")]
fn process_start(pid:u32)->anyhow::Result<String> {
    let instance=std::env::var("HYPRLAND_INSTANCE_SIGNATURE")?;
    ensure!(!instance.is_empty() && instance.bytes().all(|c|c.is_ascii_alphanumeric() || c==b'_' || c==b'-'),"Invalid compositor instance");
    let stat=std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    Ok(format!("{instance}:{}",proc_start(&stat)?))
}
#[cfg(any(target_os="linux",test))]
fn proc_start(stat:&str)->anyhow::Result<u64> {
    // comm can contain spaces AND parentheses. Field 22 is index 19 after ')'.
    stat.rsplit_once(')').context("Malformed process stat")?.1.split_whitespace().nth(19)
        .context("Missing process start time")?.parse().context("Invalid process start time")
}
#[cfg(any(target_os="linux",test))]
fn hypr_window(v:&serde_json::Value,focused:bool,identity:String)->anyhow::Result<Window> {
    ensure!(v["mapped"].as_bool()==Some(true),"Active window is not mapped");
    let w=Window {id:identity,pid:u32::try_from(v["pid"].as_u64().context("Missing window PID")?)?,app:v["class"].as_str().context("Missing window class")?.into(),title:v["title"].as_str().unwrap_or_default().into(),focused,
        x:i32::try_from(v["at"][0].as_i64().context("Missing x")?)?,y:i32::try_from(v["at"][1].as_i64().context("Missing y")?)?,
        width:u32::try_from(v["size"][0].as_u64().context("Missing width")?)?,height:u32::try_from(v["size"][1].as_u64().context("Missing height")?)?};
    ensure!(w.width>0 && w.height>0,"Invalid window bounds");
    Ok(w)
}
#[cfg(target_os="linux")]
fn validate_active_with(target:&Window,mut ipc:impl FnMut(&str)->anyhow::Result<String>,identity:impl FnOnce(&serde_json::Value)->anyhow::Result<String>)->anyhow::Result<Window> {
    validate_active_snapshot(target,&ipc("j/activewindow")?,identity)
}
#[cfg(any(target_os="linux",test))]
fn validate_active_snapshot(target:&Window,reply:&str,identity:impl FnOnce(&serde_json::Value)->anyhow::Result<String>)->anyhow::Result<Window> {
    let active:serde_json::Value=serde_json::from_str(reply)?;
    // Another focused window does not prove the selected target disappeared.
    // Do not revoke its token; explicit focus still checks the target lifetime.
    ensure!(active["address"].as_str()==Some(native_id(target)), "Target is not foreground; call computer.focus with this target before observation or input");
    // Fresh exact active object per edge; never a cached foreground verdict.
    let current=hypr_window(&active,true,identity(&active)?)?;
    if native_id(&current)==native_id(target) && (!same_identity(&current,target) || bounds(&current)!=bounds(target)) { revoke(target); }
    validate_list(target,true,vec![current])
}
#[cfg(target_os="linux")]
fn native_focus(w:&Window,check:&mut impl FnMut()->anyhow::Result<()>)->anyhow::Result<()> {
    // Read-only capability probe. Lua-config Hyprland interprets dispatch as
    // hl.dispatch(<Lua expression>), not the old dispatcher/argument grammar.
    let lua=hypr_lua_mode(&request("/eval return")?)?;
    let payload=hypr_focus_payload(native_id(w),lua)?;
    check()?;
    hypr_ack(&request(&payload)?)
}
#[cfg(any(target_os="linux",test))]
fn hypr_lua_mode(reply:&str)->anyhow::Result<bool> {
    match reply.trim() {
        "ok"=>Ok(true),
        "eval is only supported with the lua config manager" | "unknown request"=>Ok(false),
        other=>anyhow::bail!("Cannot determine Hyprland dispatch dialect: {}",other.chars().take(512).collect::<String>()),
    }
}
#[cfg(any(target_os="linux",test))]
fn hypr_focus_payload(id:&str,lua:bool)->anyhow::Result<String> {
    ensure!(id.starts_with("0x") && id.len()>2 && id.len()<=18 && id[2..].bytes().all(|c|c.is_ascii_hexdigit()),"Invalid native window address");
    Ok(if lua { format!("/dispatch hl.dsp.focus({{ window = \"address:{id}\" }})") }
       else { format!("/dispatch focuswindow address:{id}") })
}
#[cfg(any(target_os="linux",test))]
fn hypr_ack(reply:&str)->anyhow::Result<()> {
    ensure!(reply.trim()=="ok","Hyprland rejected focus request: {}",reply.chars().take(1024).collect::<String>());
    Ok(())
}
#[cfg(any(target_os="macos",target_os="windows"))]
fn qualified_identity(id:&str,pid:u32)->anyhow::Result<String> {
    // Neither HWND nor CGWindowID exposes a universal window birth serial.
    // Require process birth evidence (permission failure is an error), retain
    // unchanged observations, and revoke on every observed absence/change.
    Ok(format!("{id}|{pid}|{}",process_start(pid)?))
}
#[cfg(target_os="windows")]
fn process_start(pid:u32)->anyhow::Result<String> {
    use windows_sys::Win32::{Foundation::{CloseHandle,FILETIME},System::Threading::{OpenProcess,GetProcessTimes,PROCESS_QUERY_LIMITED_INFORMATION}};
    let process=unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION,0,pid) };
    ensure!(!process.is_null(),"Cannot query process lifetime: {}",std::io::Error::last_os_error());
    let mut birth:FILETIME=unsafe { std::mem::zeroed() };
    let mut exit=birth; let mut kernel=birth; let mut user=birth;
    let ok=unsafe { GetProcessTimes(process,&mut birth,&mut exit,&mut kernel,&mut user) };
    let error=std::io::Error::last_os_error();
    unsafe { CloseHandle(process); }
    ensure!(ok!=0,"Cannot query process lifetime: {error}");
    Ok(((u64::from(birth.dwHighDateTime)<<32)|u64::from(birth.dwLowDateTime)).to_string())
}
#[cfg(target_os="macos")]
fn process_start(pid:u32)->anyhow::Result<String> {
    let mut info:libc::proc_bsdinfo=unsafe { std::mem::zeroed() };
    let size=std::mem::size_of_val(&info) as i32;
    let read=unsafe { libc::proc_pidinfo(i32::try_from(pid)?,libc::PROC_PIDTBSDINFO,0,(&mut info as *mut libc::proc_bsdinfo).cast(),size) };
    ensure!(read==size && info.pbi_pid==pid,"Cannot query process lifetime: {}",std::io::Error::last_os_error());
    Ok(format!("{}:{}",info.pbi_start_tvsec,info.pbi_start_tvusec))
}
#[cfg(not(any(target_os="linux",target_os="macos",target_os="windows")))]
fn process_start(_:u32)->anyhow::Result<String> { anyhow::bail!("Process lifetime identity unsupported on this platform") }
#[cfg(any(target_os="macos",target_os="windows"))]
fn native_list()->anyhow::Result<Vec<Window>> {
    #[cfg(target_os="windows")] let _dpi=super::platform::DpiGuard::new()?;
    #[cfg(target_os="macos")] let focused=macos::focused_id()?;
    xcap::Window::all()?.into_iter().map(|w|Ok(Window{id:qualified_identity(&w.id()?.to_string(),w.pid()?)?,pid:w.pid()?,app:w.app_name()?,title:w.title()?,x:w.x()?,y:w.y()?,width:w.width()?,height:w.height()?,focused:{
        #[cfg(target_os="windows")] { w.is_focused()? }
        #[cfg(target_os="macos")] { Some(w.id()?)==focused }
    }})).collect()
}
#[cfg(target_os="windows")]
fn native_focus(w:&Window,check:&mut impl FnMut()->anyhow::Result<()>)->anyhow::Result<()> {
    check()?;
    use windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow;
    ensure!(unsafe { SetForegroundWindow(native_id(w).parse::<usize>()? as _) } != 0,"Windows rejected foreground activation"); Ok(())
}
#[cfg(target_os="macos")]
fn native_focus(w: &Window,check:&mut impl FnMut()->anyhow::Result<()>)->anyhow::Result<()> { macos::focus(w,check) }
#[cfg(target_os="macos")]
mod macos {
    use super::*;
    use std::ffi::{c_char,c_void};
    type Ref = *const c_void;
    #[link(name="ApplicationServices",kind="framework")]
    unsafe extern "C" {
        fn AXUIElementCreateApplication(pid:i32)->Ref;
        fn AXUIElementCreateSystemWide()->Ref;
        fn AXIsProcessTrusted()->bool;
        fn AXUIElementCopyAttributeValue(element:Ref,attribute:Ref,value:*mut Ref)->i32;
        fn AXUIElementSetAttributeValue(element:Ref,attribute:Ref,value:Ref)->i32;
        fn AXUIElementPerformAction(element:Ref,action:Ref)->i32;
        fn _AXUIElementGetWindow(element:Ref,id:*mut u32)->i32;
    }
    #[link(name="CoreFoundation",kind="framework")]
    unsafe extern "C" {
        fn CFStringCreateWithCString(allocator:Ref,text:*const c_char,encoding:u32)->Ref;
        fn CFRelease(value:Ref);
        fn CFArrayGetCount(array:Ref)->isize;
        fn CFArrayGetValueAtIndex(array:Ref,index:isize)->Ref;
        fn CFDictionaryGetValue(dictionary:Ref,key:Ref)->Ref;
        fn CFNumberGetValue(number:Ref,kind:isize,value:*mut c_void)->bool;
        static kCFBooleanTrue:Ref;
    }
    struct Owned(Ref);
    impl Drop for Owned { fn drop(&mut self) { if !self.0.is_null() { unsafe { CFRelease(self.0); } } } }
    fn string(value:&std::ffi::CStr)->anyhow::Result<Owned> {
        let value=Owned(unsafe { CFStringCreateWithCString(std::ptr::null(),value.as_ptr(),0x08000100) });
        ensure!(!value.0.is_null(),"Cannot allocate AX attribute"); Ok(value)
    }
    #[link(name="CoreGraphics",kind="framework")]
    unsafe extern "C" {
        fn CGWindowListCopyWindowInfo(options:u32,relative_to:u32)->Ref;
        static kCGWindowOwnerPID:Ref;
    }
    pub(super) fn exists(target:&Window)->anyhow::Result<bool> {
        // kCGWindowListOptionIncludingWindow, WITHOUT OnScreenOnly: includes
        // minimized/hidden/off-Space windows and does not depend on AX focus.
        let array=Owned(unsafe { CGWindowListCopyWindowInfo(1<<3,native_id(target).parse()?) });
        ensure!(!array.0.is_null(),"Cannot query native window lifetime");
        let count=unsafe { CFArrayGetCount(array.0) };
        if count==0 { return Ok(false); }
        ensure!(count==1,"Ambiguous native window identity reply");
        let dictionary=unsafe { CFArrayGetValueAtIndex(array.0,0) };
        let number=unsafe { CFDictionaryGetValue(dictionary,kCGWindowOwnerPID) };
        ensure!(!number.is_null(),"Native window reply missing owner PID");
        let mut pid=0i32;
        ensure!(unsafe { CFNumberGetValue(number,3,(&mut pid as *mut i32).cast()) },"Cannot decode native window owner PID");
        let pid=u32::try_from(pid)?;
        Ok(pid==target.pid && qualified_identity(native_id(target),pid)?==identity_id(target))
    }
    fn attribute(element:Ref,name:&std::ffi::CStr)->anyhow::Result<Option<Owned>> {
        let name=string(name)?;
        let mut value=std::ptr::null();
        let status=unsafe { AXUIElementCopyAttributeValue(element,name.0,&mut value) };
        let value=Owned(value);
        Ok(if ax_optional_value(status,!value.0.is_null())? { Some(value) } else { None })
    }
    pub(super) fn focused_id()->anyhow::Result<Option<u32>> {
        ensure!(unsafe { AXIsProcessTrusted() },"Accessibility permission required to query exact foreground window");
        let system=Owned(unsafe { AXUIElementCreateSystemWide() });
        ensure!(!system.0.is_null(),"Cannot create AX system element");
        let Some(app)=attribute(system.0,c"AXFocusedApplication")? else { return Ok(None); };
        let Some(window)=attribute(app.0,c"AXFocusedWindow")? else { return Ok(None); };
        let mut id=0;
        ensure!(unsafe { _AXUIElementGetWindow(window.0,&mut id) }==0,"Cannot resolve exact AX window identity");
        Ok(Some(id))
    }
    pub(super) fn focus(w:&Window,check:&mut impl FnMut()->anyhow::Result<()>)->anyhow::Result<()> {
    let app=Owned(unsafe { AXUIElementCreateApplication(i32::try_from(w.pid)?) });
    ensure!(!app.0.is_null(),"Cannot access AX application");
    let windows=string(c"AXWindows")?;
    let mut array=std::ptr::null();
    let status=unsafe { AXUIElementCopyAttributeValue(app.0,windows.0,&mut array) };
    let array=Owned(array);
    ensure!(status==0 && !array.0.is_null(),"Cannot enumerate AX windows; grant Accessibility permission");
    let id=native_id(w).parse::<u32>()?;
    for index in 0..unsafe { CFArrayGetCount(array.0) } {
        let element=unsafe { CFArrayGetValueAtIndex(array.0,index) };
        let mut candidate=0;
        if unsafe { _AXUIElementGetWindow(element,&mut candidate) }==0 && candidate==id {
            let front=string(c"AXFrontmost")?;
            let raise=string(c"AXRaise")?;
            check()?;
            ensure!(unsafe { AXUIElementSetAttributeValue(app.0,front.0,kCFBooleanTrue) }==0,"macOS rejected application foreground request");
            check()?;
            ensure!(unsafe { AXUIElementPerformAction(element,raise.0) }==0,"macOS rejected exact-window raise");
            return Ok(());
        }
    }
    anyhow::bail!("No exact AX window identity match; refusing app-only activation")
    }
}
#[cfg(not(any(target_os="linux",target_os="macos",target_os="windows")))]
fn native_list()->anyhow::Result<Vec<Window>> { anyhow::bail!("Window targeting unsupported") }
#[cfg(not(any(target_os="linux",target_os="macos",target_os="windows")))]
fn native_focus(_: &Window,_:&mut impl FnMut()->anyhow::Result<()>)->anyhow::Result<()> { anyhow::bail!("Window focus unsupported") }
#[cfg(test)] mod tests {
    use super::*;
    fn window()->Window { Window{id:"0x123|5|birth1|stable1".into(),pid:5,app:"editor".into(),title:String::new(),x:0,y:0,width:10,height:10,focused:true} }
    #[test] fn unchanged_list_preserves_tokens_without_a_turn_timeout() {
        let mut tokens=Vec::new();
        let a=reconcile(&mut tokens,vec![window()],7);
        let mut renamed=window(); renamed.title="renamed".into(); renamed.focused=false;
        for _ in 0..100 {
            let b=reconcile(&mut tokens,vec![renamed.clone()],7);
            assert_eq!(a[0].0,b[0].0);
            assert!(same_target(Some(&a[0].1),Some(&b[0].1)));
            assert_eq!(b[0].1.title,"renamed");
            assert_eq!(serde_json::to_value(&b[0].1).unwrap()["id"],"0x123");
        }
        assert!(resolve_from(&tokens,&a[0].0,7).is_ok());
        assert!(resolve_from(&tokens,&a[0].0,8).is_err());
        assert!(resolve_from(&tokens,"unknown",7).is_err());
        let b=reconcile(&mut tokens,vec![window()],8);
        assert_ne!(a[0].0,b[0].0);
        assert!(!same_target(Some(&a[0].1),Some(&b[0].1)));
    }
    #[test] fn changed_lifetime_geometry_and_missing_windows_never_retarget() {
        for change in ["process","stable","pid","app","x","y","width","height","missing"] {
            let mut tokens=Vec::new();
            let a=reconcile(&mut tokens,vec![window()],1);
            let mut w=window();
            match change {
                "process"=>w.id="0x123|5|birth2|stable1".into(),
                "stable"=>w.id="0x123|5|birth1|stable2".into(),
                "pid"=>w.pid+=1,"app"=>w.app="other".into(),
                "x"=>w.x+=1,"y"=>w.y+=1,"width"=>w.width+=1,"height"=>w.height+=1,
                _=>{ reconcile(&mut tokens,vec![],1); },
            }
            let b=reconcile(&mut tokens,vec![w],1);
            assert_ne!(a[0].0,b[0].0,"{change}");
            assert!(resolve_from(&tokens,&a[0].0,1).is_err(),"{change}");
            assert!(!same_target(Some(&a[0].1),Some(&b[0].1)),"{change}");
            // Even a geometry round-trip cannot resurrect the original selector.
            let c=reconcile(&mut tokens,vec![window()],1);
            assert_ne!(a[0].0,c[0].0,"{change}");
        }
    }
    #[test] fn cache_is_bounded_and_does_not_evict_unchanged_entries_on_reorder() {
        let mut tokens=Vec::new();
        let windows:Vec<_>=(0..MAX_TARGETS+10).map(|n| {let mut w=window(); w.id=format!("0x{n:x}|birth"); w}).collect();
        let a=reconcile(&mut tokens,windows.clone(),1);
        assert_eq!(tokens.len(),MAX_TARGETS);
        let mut retained=windows[..MAX_TARGETS].to_vec(); retained.reverse();
        let b=reconcile(&mut tokens,retained,1);
        assert_eq!(a[0].0,b[MAX_TARGETS-1].0);
        reconcile(&mut tokens,vec![],1);
        assert!(tokens.is_empty());
        assert!(resolve_from(&tokens,&a[0].0,1).is_err());
    }
    fn client()->serde_json::Value { serde_json::json!({"address":"0x123","pid":5,"stableId":"1a2b","mapped":true,"class":"editor","title":"private","at":[0,0],"size":[10,10]}) }
    fn fixture_identity(v:&serde_json::Value)->anyhow::Result<String> { hypr_identity_with(v,|_|Ok("birth1".into())) }
    #[test] fn focused_snapshot_requires_exact_identity_and_bounds() {
        let v=client(); let target=hypr_window(&v,true,fixture_identity(&v).unwrap()).unwrap();
        assert!(validate_active_snapshot(&target,&v.to_string(),fixture_identity).is_ok());
        for (field,value) in [("address",serde_json::json!("0x124")),("pid",serde_json::json!(6)),("stableId",serde_json::json!(18)),("class",serde_json::json!("other")),("mapped",serde_json::json!(false)),("at",serde_json::json!([1,0])),("size",serde_json::json!([10,11])),("size",serde_json::json!([0,10]))] {
            let mut changed=v.clone(); changed[field]=value;
            assert!(validate_active_snapshot(&target,&changed.to_string(),fixture_identity).is_err(),"{field}");
        }
        for field in ["address","pid","stableId","class","mapped","at","size"] {
            let mut changed=v.clone(); changed.as_object_mut().unwrap().remove(field);
            assert!(validate_active_snapshot(&target,&changed.to_string(),fixture_identity).is_err(),"missing {field}");
        }
        for bad in ["{}","null","[]","not json"] { assert!(validate_active_snapshot(&target,bad,fixture_identity).is_err()); }
        assert!(validate_active_snapshot(&target,&v.to_string(),|_|anyhow::bail!("process access denied")).is_err());
        assert!(validate_active_snapshot(&target,&v.to_string(),|v|hypr_identity_with(v,|_|Ok("birth2".into()))).is_err());
        let mut renamed=v.clone(); renamed["title"]=serde_json::json!("new");
        assert!(validate_active_snapshot(&target,&renamed.to_string(),fixture_identity).is_ok());
    }
    #[test] fn another_active_window_is_focus_loss_not_identity_loss() {
        let target=window();
        let error=validate_active_snapshot(&target,r#"{"address":"0x999"}"#,|_| panic!("unrelated window identity must not be inspected")).unwrap_err();
        assert!(error.to_string().contains("not foreground"));
        assert!(!error.to_string().contains("disappeared"));
    }
    #[test] fn background_guard_preserves_lifetime_and_bounds_without_requiring_focus() {
        let mut tokens=Vec::new();
        let mut listed=window(); listed.focused=false;
        let listed=reconcile(&mut tokens,vec![listed],9);
        let target=resolve_from(&tokens,&listed[0].0,9).unwrap();

        assert!(validate_list(&target,false,vec![target.clone()]).is_ok());
        assert!(validate_list(&target,true,vec![target.clone()]).unwrap_err().to_string().contains("not foreground"));

        let mut moved=target.clone(); moved.x+=1;
        assert!(validate_list(&target,false,vec![moved]).unwrap_err().to_string().contains("moved/resized"));
        let mut reused=target.clone(); reused.id="0x123|5|birth2|stable2".into();
        assert!(validate_list(&target,false,vec![reused]).unwrap_err().to_string().contains("disappeared or identity changed"));
        assert!(validate_list(&target,false,vec![]).unwrap_err().to_string().contains("disappeared or identity changed"));

        reconcile(&mut tokens,vec![],9);
        assert!(resolve_from(&tokens,&listed[0].0,9).is_err());
        let reopened=reconcile(&mut tokens,vec![window()],9);
        assert_ne!(listed[0].0,reopened[0].0,"close/reopen must not resurrect the listed target");
    }
    #[cfg(target_os="linux")]
    #[test] fn each_focused_validation_uses_one_fresh_ipc_request() {
        let mut requests=Vec::new();
        let v=client(); let target=hypr_window(&v,true,fixture_identity(&v).unwrap()).unwrap();
        for _ in 0..100 {
            assert!(validate_active_with(&target,|command| { requests.push(command.to_owned()); Ok(v.to_string()) },fixture_identity).is_ok());
        }
        assert_eq!(requests,vec!["j/activewindow";100]);
        assert!(validate_active_with(&target,|_|Ok("{}".into()),fixture_identity).is_err());
        assert!(validate_active_with(&target,|_|anyhow::bail!("IPC denied"),fixture_identity).is_err());
    }
    #[test] fn lifetime_evidence_is_strict_and_process_stat_handles_nested_comm() {
        let v=client();
        let strong=fixture_identity(&v).unwrap();
        let mut old=v.clone(); old.as_object_mut().unwrap().remove("stableId");
        assert_ne!(strong,fixture_identity(&old).unwrap());
        for bad in [serde_json::json!({}),serde_json::json!(-1),serde_json::json!("a#b"),serde_json::json!("")] {
            let mut malformed=v.clone(); malformed["stableId"]=bad;
            assert!(fixture_identity(&malformed).is_err());
        }
        let mut fields=vec!["0";20]; fields[0]="S"; fields[19]="12345";
        assert_eq!(proc_start(&format!("5 (name (with) spaces) {}",fields.join(" "))).unwrap(),12345);
        for bad in ["","5 (name) S","5 no paren"] { assert!(proc_start(bad).is_err()); }
        assert_eq!(native_id(&window()),"0x123");
    }
    #[test] fn hyprland_focus_dialects_and_replies_are_exact_and_bounded() {
        assert!(hypr_lua_mode("ok\n").unwrap());
        assert!(!hypr_lua_mode("eval is only supported with the lua config manager").unwrap());
        assert!(!hypr_lua_mode("unknown request").unwrap());
        assert!(hypr_lua_mode("permission denied").is_err());
        assert_eq!(hypr_focus_payload("0x123abc",false).unwrap(),"/dispatch focuswindow address:0x123abc");
        assert_eq!(hypr_focus_payload("0x123abc",true).unwrap(),"/dispatch hl.dsp.focus({ window = \"address:0x123abc\" })");
        for bad in ["0x","123","0x12;exit","0x1\" })", "0x1\n"] { assert!(hypr_focus_payload(bad,true).is_err()); }
        assert!(hypr_ack("ok\r\n").is_ok());
        for bad in ["", "ok but failed", "No such window found", "[string]: syntax error near 'address'"] {
            let error=hypr_ack(bad).unwrap_err().to_string();
            assert!(error.contains(bad));
        }
    }
    #[test] fn closed_uses_lifetime_not_visibility_or_focus() {
        let target=Window{id:"123".into(),pid:5,app:"editor".into(),title:String::new(),x:0,y:0,width:10,height:10,focused:false};
        // Hidden, minimized and off-Space all remain alive. The presentation
        // list may be empty or fail to read focus; closed must never consult it.
        for _visibility in ["hidden","minimized","off-space","no AX focus"] {
            assert!(!condition_with(&target,true,|_|Ok(true),||panic!("closed queried presentation list")).unwrap());
        }
        assert!(condition_with(&target,true,|_|Ok(false),||panic!("closed queried presentation list")).unwrap());
        assert!(condition_with(&target,true,|_|anyhow::bail!("native probe denied"),||Ok(Vec::new())).is_err());
        assert!(!condition_with(&target,false,|_|panic!("foreground queried lifetime"),||Ok(vec![target.clone()])).unwrap());
    }
    #[test] fn mac_absent_focus_is_optional_but_permission_and_transport_errors_are_not() {
        assert!(!ax_optional_value(-25212,false).unwrap());
        assert!(ax_optional_value(0,true).unwrap());
        for status in [-25211,-25204,-25205,-25202,-25200] {
            assert!(ax_optional_value(status,false).is_err(),"AX error {status} must propagate");
        }
        assert!(ax_optional_value(0,false).is_err(),"malformed successful reply is not absent focus");
    }
    #[test] fn identity_does_not_depend_on_title_but_rejects_reused_handle() {
        let a=Window{id:"123".into(),pid:5,app:"editor".into(),title:"old".into(),x:0,y:0,width:10,height:10,focused:true};
        let mut b=a.clone(); b.title="new".into(); assert!(same_identity(&a,&b));
        assert!(same_target(Some(&a),Some(&b)));
        b.x+=1; assert!(!same_target(Some(&a),Some(&b))); b.x=a.x;
        assert!(!same_target(Some(&a),None));
        b.pid+=1; assert!(!same_identity(&a,&b));
        b.pid=a.pid; b.app="other".into(); assert!(!same_identity(&a,&b));
    }
}
