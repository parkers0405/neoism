//! Explicit foreground targeting. Tokens are snapshots, not application sandboxes.
use anyhow::{Context, ensure};
use serde::Serialize;
use std::sync::Mutex;
#[derive(Clone, Debug, Serialize)]
pub(super) struct Window {
    pub id:String, pub pid:u32, pub app:String, pub title:String,
    pub x:i32, pub y:i32, pub width:u32, pub height:u32, pub focused:bool,
}
static TOKENS: Mutex<Vec<(String,Window,std::time::Instant,u64)>> = Mutex::new(Vec::new());
pub(super) fn list() -> anyhow::Result<serde_json::Value> {
    let windows=native_list()?;
    let mut tokens=TOKENS.lock().map_err(|_|anyhow::anyhow!("Window state poisoned"))?;
    *tokens=windows.into_iter().map(|w|(format!("{:032x}",rand::random::<u128>()),w,std::time::Instant::now(),super::STOP.load(std::sync::atomic::Ordering::SeqCst))).collect();
    Ok(serde_json::json!({"windows":tokens.iter().map(|(token,w,_,_)|serde_json::json!({"target":token,"window":w})).collect::<Vec<_>>(),"coordinates":"desktop logical bounds; input still requires display screenshot image pixels, NOT window-relative coordinates","input":"foreground only; focus can race external user/apps; no sandbox or allowlist guarantee","tokens":"expire after 30 seconds, stop, or next list; identity revalidated before use"}))
}
pub(super) fn resolve(token:&str) -> anyhow::Result<Window> {
    resolve_from(&TOKENS.lock().map_err(|_|anyhow::anyhow!("Window state poisoned"))?,token,super::STOP.load(std::sync::atomic::Ordering::SeqCst))
}
fn resolve_from(tokens:&[(String,Window,std::time::Instant,u64)],token:&str,epoch:u64)->anyhow::Result<Window> {
    tokens.iter().find(|(t,_,created,generation)|t==token && created.elapsed()<std::time::Duration::from_secs(30) && *generation==epoch).map(|(_,w,_,_)|w.clone()).context("Unknown/stale window target; list windows again")
}
fn same_identity(a:&Window,b:&Window)->bool { a.id==b.id && a.pid==b.pid && a.app==b.app }
pub(super) fn same_target(a:Option<&Window>,b:Option<&Window>)->bool {
    match (a,b) {
        (None,None)=>true,
        (Some(a),Some(b))=>same_identity(a,b) && (a.x,a.y,a.width,a.height)==(b.x,b.y,b.width,b.height),
        _=>false,
    }
}
pub(super) fn validate(target:&Window, foreground:bool)->anyhow::Result<Window> {
    validate_list(target,foreground,native_list()?)
}
pub(super) fn validate_list(target:&Window,foreground:bool,windows:Vec<Window>)->anyhow::Result<Window> {
    let current=windows.into_iter().find(|w|same_identity(w,target)).context("Target window disappeared or identity changed")?;
    if foreground {
        ensure!(current.focused,"Target is not foreground; observe/focus explicitly before input");
        ensure!((current.x,current.y,current.width,current.height)==(target.x,target.y,target.width,target.height),"Target window moved/resized; list windows and take another screenshot");
    }
    Ok(current)
}
pub(super) fn condition(target:&Window,closed:bool)->anyhow::Result<bool> {
    condition_with(target,closed,native_exists,native_list)
}
fn condition_with(target:&Window,closed:bool,exists:impl FnOnce(&Window)->anyhow::Result<bool>,list:impl FnOnce()->anyhow::Result<Vec<Window>>)->anyhow::Result<bool> {
    // Visibility/Space membership is not lifetime. Closed never consults the
    // presentation list (nor AX focus), which can legitimately be empty.
    if closed { return exists(target).map(|exists|!exists); }
    Ok(list()?.into_iter().any(|w|same_identity(&w,target) && w.focused))
}
pub(super) fn focus(token:&str,mut check:impl FnMut()->anyhow::Result<()>)->anyhow::Result<()> {
    let target=resolve(token)?;
    ensure!(native_exists(&target)?,"Target window closed or identity changed");
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
        if id==target.id {
            return Ok(client["pid"].as_u64().context("Missing client PID")?==u64::from(target.pid));
        }
    }
    Ok(false)
}
#[cfg(target_os="windows")]
fn native_exists(target:&Window)->anyhow::Result<bool> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{IsWindow,GetWindowThreadProcessId};
    let hwnd=target.id.parse::<usize>()? as _;
    if unsafe { IsWindow(hwnd) }==0 { return Ok(false); }
    let mut pid=0;
    let thread=unsafe { GetWindowThreadProcessId(hwnd,&mut pid) };
    if thread==0 {
        if unsafe { IsWindow(hwnd) }==0 { return Ok(false); }
        anyhow::bail!("Cannot read native window owner: {}",std::io::Error::last_os_error());
    }
    Ok(pid==target.pid)
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
        let id=v["address"].as_str().context("Missing window address")?.to_owned();
        ensure!(id.starts_with("0x") && id.len()>2 && id[2..].bytes().all(|c|c.is_ascii_hexdigit()),"Invalid native window address");
        Ok(Window {focused:active["address"].as_str()==Some(&id),id,pid:u32::try_from(v["pid"].as_u64().context("Missing window PID")?)?,app:v["class"].as_str().unwrap_or_default().into(),title:v["title"].as_str().unwrap_or_default().into(),x:i32::try_from(v["at"][0].as_i64().context("Missing x")?)?,y:i32::try_from(v["at"][1].as_i64().context("Missing y")?)?,width:u32::try_from(v["size"][0].as_u64().context("Missing width")?)?,height:u32::try_from(v["size"][1].as_u64().context("Missing height")?)?})
    }).collect()
}
#[cfg(target_os="linux")]
fn native_focus(w:&Window,check:&mut impl FnMut()->anyhow::Result<()>)->anyhow::Result<()> {
    // Read-only capability probe. Lua-config Hyprland interprets dispatch as
    // hl.dispatch(<Lua expression>), not the old dispatcher/argument grammar.
    let lua=hypr_lua_mode(&request("/eval return")?)?;
    let payload=hypr_focus_payload(&w.id,lua)?;
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
fn native_list()->anyhow::Result<Vec<Window>> {
    #[cfg(target_os="windows")] let _dpi=super::platform::DpiGuard::new()?;
    #[cfg(target_os="macos")] let focused=macos::focused_id()?;
    xcap::Window::all()?.into_iter().map(|w|Ok(Window{id:w.id()?.to_string(),pid:w.pid()?,app:w.app_name()?,title:w.title()?,x:w.x()?,y:w.y()?,width:w.width()?,height:w.height()?,focused:{
        #[cfg(target_os="windows")] { w.is_focused()? }
        #[cfg(target_os="macos")] { Some(w.id()?)==focused }
    }})).collect()
}
#[cfg(target_os="windows")]
fn native_focus(w:&Window,check:&mut impl FnMut()->anyhow::Result<()>)->anyhow::Result<()> {
    check()?;
    use windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow;
    ensure!(unsafe { SetForegroundWindow(w.id.parse::<usize>()? as _) } != 0,"Windows rejected foreground activation"); Ok(())
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
        let array=Owned(unsafe { CGWindowListCopyWindowInfo(1<<3,target.id.parse()?) });
        ensure!(!array.0.is_null(),"Cannot query native window lifetime");
        let count=unsafe { CFArrayGetCount(array.0) };
        if count==0 { return Ok(false); }
        ensure!(count==1,"Ambiguous native window identity reply");
        let dictionary=unsafe { CFArrayGetValueAtIndex(array.0,0) };
        let number=unsafe { CFDictionaryGetValue(dictionary,kCGWindowOwnerPID) };
        ensure!(!number.is_null(),"Native window reply missing owner PID");
        let mut pid=0i32;
        ensure!(unsafe { CFNumberGetValue(number,3,(&mut pid as *mut i32).cast()) },"Cannot decode native window owner PID");
        Ok(u32::try_from(pid)?==target.pid)
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
    let id=w.id.parse::<u32>()?;
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
    #[test] fn stale_tokens_fail_closed_without_selecting_current_foreground() {
        let window=Window{id:"helium".into(),pid:5,app:"Helium".into(),title:String::new(),x:0,y:0,width:10,height:10,focused:true};
        let mut tokens=vec![("opaque".into(),window,std::time::Instant::now(),7)];
        assert!(resolve_from(&tokens,"opaque",7).is_ok());
        assert!(resolve_from(&tokens,"opaque",8).is_err());
        assert!(resolve_from(&tokens,"not-supplied-by-this-caller",7).is_err());
        tokens[0].2-=std::time::Duration::from_secs(31);
        assert!(resolve_from(&tokens,"opaque",7).is_err());
        assert!(resolve_from(&[],"opaque",7).is_err());
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
