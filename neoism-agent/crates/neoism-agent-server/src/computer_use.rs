//! Built-in, opt-in desktop control. No shell commands and no separate MCP process.
//! All observation/input goes through the ordinary session MCP permission path.
use std::{io::Cursor, path::Path, sync::{Arc, Mutex, atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering}}, time::{Duration, Instant}};
use anyhow::{bail, ensure, Context};
use base64::{Engine, engine::general_purpose::STANDARD};
use enigo::Key;
#[cfg(any(test, not(target_os="linux")))]
use enigo::{Axis, Button, Direction, Mouse};
#[cfg(not(target_os="linux"))]
use enigo::{Enigo, Settings};
use neoism_agent_service_api::{BuiltinMcpCallResult, BuiltinMcpContent, BuiltinMcpService, BuiltinMcpTool, ServiceError, ServiceFuture};
use serde::Deserialize;
use serde_json::{json, Value};

#[path = "computer_use/platform.rs"]
mod platform;
#[path = "computer_use/shortcuts.rs"]
mod shortcuts;
#[cfg(target_os = "linux")]
#[path = "computer_use/linux_text.rs"]
mod linux_text;
#[cfg(target_os = "linux")]
#[path = "computer_use/linux_pointer.rs"]
mod linux_pointer;
#[cfg(target_os = "linux")]
#[path = "computer_use/linux_clipboard.rs"]
mod linux_clipboard;
#[cfg(all(test, target_os = "linux"))]
#[path = "computer_use/linux_browser_live_tests.rs"]
mod linux_browser_live_tests;
#[path = "computer_use/windows.rs"]
mod windows;
pub(crate) struct ComputerUse;
static SERIAL: Mutex<()> = Mutex::new(());
static FRAME: Mutex<Option<Frame>> = Mutex::new(None);
static STOP: AtomicU64 = AtomicU64::new(0);
const MAX_TEXT: usize = 512;
const DEADLINE: Duration = Duration::from_secs(10);

// Test-only, task-scoped injection: exercise the real MCP admission and worker
// without ever opening a desktop backend. No global override or production API.
#[cfg(test)]
pub(crate) static TEST_REVOCATION_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
#[cfg(test)]
type TestBackend = Arc<dyn Fn(&str, Value) -> anyhow::Result<BuiltinMcpCallResult> + Send + Sync>;
#[cfg(test)]
tokio::task_local! { static TEST_BACKEND: TestBackend; }
#[cfg(test)]
pub(crate) async fn with_test_backend<F: std::future::Future>(backend: TestBackend, future: F) -> F::Output {
    TEST_BACKEND.scope(backend, future).await
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub(super) struct Display {
    id: String,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}
#[derive(Clone)]
struct Frame { target: Option<windows::Window>, id: String, display: Display, width: u32, height: u32, created: Instant, epoch: u64 }

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    Move { frame: String, x: u32, y: u32 },
    Click { frame: String, x: u32, y: u32, #[serde(default)] button: MouseButton },
    Drag { frame: String, x: u32, y: u32, to_x: u32, to_y: u32 },
    Scroll { amount: i32, #[serde(default)] horizontal: bool },
    Text { text: String },
    Paste { text: String, clipboard_policy: String, #[serde(default)] paste_shortcut: PasteShortcut },
    Key { key: String, #[serde(default)] modifiers: Vec<String> },
}
#[derive(Debug, Default, Deserialize)]
enum PasteShortcut {
    #[default]
    #[serde(rename="control-v")]
    ControlV,
    #[serde(rename="control-shift-v")]
    ControlShiftV,
}
impl PasteShortcut {
    fn keys(&self)->Vec<Key> {
        match self {
            Self::ControlV=>vec![Key::Control,Key::Unicode('v')],
            Self::ControlShiftV=>vec![Key::Control,Key::Shift,Key::Unicode('v')],
        }
    }
}
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MouseButton { #[default] Left, Right, Middle }

fn clipboard_effects(value:&mut Value,changed:bool,uncertain:bool) {
    // Absence is intentional: never turn unknown ownership into a false claim.
    if changed {value["clipboardChanged"]=json!(true);}
    if uncertain {value["clipboardMayHaveChanged"]=json!(true);}
    if changed || uncertain {
        value["clipboardPolicy"]=json!("replace");
        value["clipboardWarning"]=json!("Clipboard was or may have been replaced; history may retain text. Previous content is not read or restored. Observe before any deliberate retry; no automatic retry.");
    }
}
fn input_timeout(total:Option<usize>,completed:usize,clipboard_changed:bool,clipboard_uncertain:bool)->BuiltinMcpCallResult {
    let mut value=json!({"status":"partial_unknown","applicationVerified":false,"error":"Computer input timed out; further input cancelled. Native input may have arrived or still be in flight; observe before any deliberate retry. No automatic retry.","partialActionPossible":true});
    if let Some(total)=total {
        value["completed"]=json!(completed);value["total"]=json!(total);
        value["screenshotError"]=json!("No final screenshot available after timeout");
    }
    clipboard_effects(&mut value,clipboard_changed,clipboard_uncertain);
    let mut result=text(value);result.is_error=Some(true);result
}
fn dispatched(method:&str)->BuiltinMcpCallResult {
    text(json!({"status":"dispatched_unverified","applicationVerified":false,"method":method,"note":"Native event acceptance is not application consumption. Verify the effect with a new observation; do not blindly retry."}))
}
fn text(value: Value) -> BuiltinMcpCallResult {
    BuiltinMcpCallResult { content: vec![BuiltinMcpContent::Text { text: value.to_string(), annotations: None }], is_error: None }
}
pub(crate) fn admit<T>(read_enabled_state: impl FnOnce() -> anyhow::Result<T>) -> anyhow::Result<(u64, T)> {
    let generation = STOP.load(Ordering::SeqCst);
    Ok((generation, read_enabled_state()?))
}
pub(crate) fn stop() { STOP.fetch_add(1, Ordering::SeqCst); }

impl BuiltinMcpService for ComputerUse {
    fn id(&self) -> &str { "computer" }
    fn enabled_by_default(&self) -> bool { false }
    fn tools(&self) -> Vec<BuiltinMcpTool> {
        let mut tools:Vec<BuiltinMcpTool> = [
            ("capabilities", "Report host desktop backend, display geometry and prerequisites. Does not capture or inject input.", json!({"type":"object","properties":{},"additionalProperties":false})),
            ("screenshot", "Capture one display as PNG with a short-lived frame token. Images may contain private information. Pass image-pixel coordinates and that token to move/click.", json!({"type":"object","properties":{"settle_ms":{"type":"integer","minimum":0,"maximum":3000,"default":1200,"description":"Bounded visual settling before returning image; 0 captures immediately. Not application readiness."},"display":{"type":"string"},"target":{"type":"string","description":"Optional foreground window token; binds image to this target for pointer input"}},"required":["display"],"additionalProperties":false})),
            ("input", "Control the HOST desktop (not a remote browser). One bounded action; no persistent key/button holds. Use a fresh screenshot frame for move/click/drag; drag uses to_x/to_y for its endpoint. Text is literal, max 512 Unicode characters, with no C0/C1 controls (including newline, CR, Tab); use explicit key actions for Return/Tab. Key names: enter, tab, escape, backspace, delete, space, up/down/left/right, home/end, page_up/page_down, or one ASCII letter/digit (case-insensitive; use shift modifier). Use text for other characters. Modifiers (case-insensitive): control/ctrl/Control_L, alt/option/Alt_L, shift/Shift_L, meta/super/Super_L/win/windows/logo/cmd/command. Modifier names can also be tapped as keys. Key aliases include Return, Esc, ArrowLeft/Right/Up/Down, PageUp/PgUp and PageDown/PgDn. Scroll amount -20..20; positive is down/right. Cancellation can leave partial text.", json!({"type":"object","properties":{"target":{"type":"string","description":"Optional windows token; require unchanged foreground window. Not a sandbox."},"action":{"type":"string","enum":["move","click","drag","scroll","text","key"]},"frame":{"type":"string"},"x":{"type":"integer","minimum":0},"y":{"type":"integer","minimum":0},"to_x":{"type":"integer","minimum":0},"to_y":{"type":"integer","minimum":0},"button":{"type":"string","enum":["left","right","middle"]},"amount":{"type":"integer","minimum":-20,"maximum":20},"horizontal":{"type":"boolean"},"text":{"type":"string","maxLength":512,"description":"Literal Unicode only; no C0/C1 control characters including newline, CR or Tab. Use explicit key actions for Return/Tab."},"key":{"type":"string","description":"Case-insensitive named key or ASCII letter/digit. Super_L/super/meta tap the system modifier; Return=enter, Esc=escape. Use action:text for literal text."},"modifiers":{"type":"array","items":{"type":"string","description":"Case-insensitive: control/ctrl/Control_L, alt/option/Alt_L, shift/Shift_L, meta/super/Super_L/win/windows/logo/cmd/command."},"maxItems":4}},"required":["action"],"additionalProperties":false})),
            ("windows", "List native windows and snapshot target tokens. Re-list invalidates old tokens. Bounds are not screenshot pixel coordinates. Foreground targeting is not isolation.", json!({"type":"object","properties":{},"additionalProperties":false})),
            ("focus", "Request focus of one listed target window; confirm foreground or fail. Invalidates screenshot coordinates. Does not guarantee exclusive input routing.", json!({"type":"object","properties":{"target":{"type":"string"}},"required":["target"],"additionalProperties":false})),
            ("batch", "Run 1..16 already-decided input actions serially, stopping on first failure/cancellation. Optional final display screenshot is returned as model image with frame token. Do not batch blind semantic decisions: observe between decisions. Total text <=512 characters. Failed action may be partially delivered.", json!({"type":"object","properties":{"settle_ms":{"type":"integer","minimum":0,"maximum":3000,"default":1200,"description":"Final screenshot visual-settling budget; 0 captures immediately."},"actions":{"type":"array","minItems":1,"maxItems":16,"items":{"type":"object","description":"Same action object as input; no nested batches"}},"target":{"type":"string"},"screenshot":{"type":"string","description":"Display ID for final screenshot"}},"required":["actions"],"additionalProperties":false})),
            ("wait", "Bounded observation of one listed window becoming foreground or closing. No typing, clicks, browser semantics, or implicit retries of actions. Polls at 100ms; native calls can overrun.", json!({"type":"object","properties":{"target":{"type":"string"},"condition":{"type":"string","enum":["foreground","closed"]},"timeout_ms":{"type":"integer","minimum":100,"maximum":3000}},"required":["target","condition","timeout_ms"],"additionalProperties":false})),
            ("stop", "Cancel current computer action and invalidate screenshot coordinates. Does not wait for the desktop lock.", json!({"type":"object","properties":{},"additionalProperties":false})),
        ].into_iter().map(|(name, description, input_schema)| BuiltinMcpTool {
            name: name.into(), description: Some(description.into()), input_schema,
            annotations: Some(json!({"readOnlyHint": name == "capabilities" || name == "screenshot" || name == "windows" || name == "wait", "destructiveHint": name == "input" || name == "batch" || name == "focus", "openWorldHint": true})),
        }).collect();
        let input=tools.iter_mut().find(|t|t.name=="input").unwrap();
        input.description.as_mut().unwrap().push_str(" Linux text rejects non-BMP characters before input because native character delivery is not faithful. Explicit action:paste supports Unicode and LF/Tab, requires target and clipboard_policy:replace, and replaces the desktop clipboard; clipboard history may retain the text. Old clipboard is neither read nor restored. The source remains available until another owner replaces it. Paste may execute multiline commands in terminals or trigger application behavior. No automatic text-to-paste fallback. paste_shortcut is control-v (default) or control-shift-v; dispatch is not application verification.");
        input.input_schema["properties"]["action"]["enum"].as_array_mut().unwrap().push(json!("paste"));
        input.input_schema["properties"]["clipboard_policy"]=json!({"type":"string","enum":["replace"],"description":"Required for paste. Explicitly replaces clipboard without reading/restoring previous content; history may retain text."});
        input.input_schema["properties"]["paste_shortcut"]=json!({"type":"string","enum":["control-v","control-shift-v"],"default":"control-v"});
        input.input_schema["properties"]["text"]["description"]=json!("At most 512 Unicode characters. action:text forbids C0/C1 controls and on Linux non-BMP characters. action:paste permits LF/Tab/non-BMP but forbids NUL and requires explicit clipboard replacement.");
        input.input_schema["allOf"]=json!([{"if":{"properties":{"action":{"const":"paste"}},"required":["action"]},"then":{"required":["text","clipboard_policy"]}}]);
        let mut action_schema=tools.iter().find(|t|t.name=="input").unwrap().input_schema.clone();
        action_schema["properties"].as_object_mut().unwrap().remove("target");
        tools.iter_mut().find(|t|t.name=="batch").unwrap().input_schema["properties"]["actions"]["items"]=action_schema;
        for tool in &mut tools {
            match tool.name.as_str() {
                "input" => {
                    tool.description.as_mut().unwrap().push_str(" REQUIRED: pass target on every call, even after focus. No inherited focus. Abort on foreground loss; do not automatically refocus. Only deliberate individual desktop shortcuts/pointer actions may omit target with scope:desktop; never text.");
                    tool.input_schema["properties"]["target"]["description"]=json!("Required window token unless explicit scope:desktop. Validated before each key press/text character; not isolation.");
                    tool.input_schema["properties"]["scope"]=json!({"type":"string","enum":["desktop"],"description":"Explicit unguarded desktop shortcut/pointer action. Not text. Mutually exclusive with target."});
                    tool.input_schema["oneOf"]=json!([
                        {"required":["target"],"not":{"required":["scope"]}},
                        {"required":["scope"],"not":{"required":["target"]},"properties":{"action":{"enum":["key","move","click","drag","scroll"]}}}
                    ]);
                }
                "batch" => {
                    tool.description.as_mut().unwrap().push_str(" REQUIRED target binds ALL actions and final screenshot. No inherited focus, per-action overrides, or desktop scope. Focus loss stops remaining input without refocusing; failed action may be partial.");
                    tool.input_schema["required"]=json!(["actions","target"]);
                }
                "focus" => tool.description.as_mut().unwrap().push_str(" Returns confirmed target; does NOT bind later calls. Pass target explicitly on every input/batch/screenshot. Never retry focus automatically after user interference."),
                _ => {}
            }
        }
        tools
    }
    fn call_tool(&self, _: &Path, tool: &str, _: Value) -> Result<BuiltinMcpCallResult, ServiceError> {
        match tool {
            "stop" => { stop(); Ok(text(json!({"stopped":true,"note":"In-flight native calls finish; no subsequent input will be sent."}))) }
            "capabilities" => Ok(text(json!({"backend":platform::NAME,"requirements":platform::REQUIREMENTS,"input":"not_probed","screenshots":"not_probed","sessionApprovalRequired":true,"actionTimeoutSeconds":10,"maxTextCharacters":MAX_TEXT}))),
            _ => Err(ServiceError::new("Computer observation/input requires the permission-checked session tool executor; direct MCP calls are forbidden")),
        }
    }
    fn call_tool_authorized_async<'a>(&'a self, directory: &'a Path, tool: &'a str, arguments: Value, session_authorized: bool, cancel: Arc<AtomicBool>, revocation_generation: Option<u64>) -> ServiceFuture<'a, Result<BuiltinMcpCallResult, ServiceError>> {
        Box::pin(async move {
            if tool == "stop" { return self.call_tool(directory, tool, arguments); }
            if !session_authorized {
                if tool == "capabilities" { return self.call_tool(directory, tool, arguments); }
                return Err(ServiceError::new("Computer use requires MCP enablement and session permission approval"));
            }
            // A dropped future must also stop a spawn_blocking worker. Never use the
            // caller's token here: completing this call must not cancel the whole turn.
            let dropped = Arc::new(AtomicBool::new(false));
            let guard = CancelOnDrop(dropped.clone());
            let epoch = revocation_generation.ok_or_else(|| ServiceError::new("Missing computer-use admission generation"))?;
            let progress=Arc::new(AtomicUsize::new(0));
            let worker_progress=progress.clone();
            let batch_total=if tool=="batch" { arguments["actions"].as_array().map(Vec::len) } else { None };
            // Retain only action kinds for timeout reporting, never text payloads.
            let paste_indices:Vec<usize>=if tool=="batch" {arguments["actions"].as_array().into_iter().flatten().enumerate().filter_map(|(i,a)|(a["action"]=="paste").then_some(i)).collect()} else {Vec::new()};
            let input_requested=tool=="input";
            let input_paste=input_requested && arguments["action"]=="paste";
            let tool = tool.to_owned();
            #[cfg(test)]
            let test_backend = TEST_BACKEND.try_with(Arc::clone).ok();
            let result = tokio::time::timeout(DEADLINE, tokio::task::spawn_blocking(move || {
                run_worker(epoch, cancel, dropped, |check| {
                    let check=Check { progress:Some(worker_progress), ..check.clone() };
                    let check=&check;
                    #[cfg(test)]
                    if let Some(backend) = test_backend { return backend(&tool, arguments); }
                    execute(&tool, arguments, check)
                })
            })).await;
            drop(guard);
            match result {
                Ok(Ok(result)) => result.map_err(|e| ServiceError::new(format!("{e:#}"))),
                Ok(Err(e)) => Err(ServiceError::new(format!("Computer worker failed: {e}"))),
                Err(_) if batch_total.is_some() => {
                    let completed=progress.load(Ordering::SeqCst);
                    Ok(input_timeout(batch_total,completed,paste_indices.iter().any(|i|*i<completed),paste_indices.iter().any(|i|*i>=completed)))
                }
                Err(_) if input_requested => Ok(input_timeout(None,0,false,input_paste)),
                Err(_) => Err(ServiceError::new("Computer operation timed out; further input cancelled. Native calls cannot be preempted.")),
            }
        })
    }
}
struct CancelOnDrop(Arc<AtomicBool>);
impl Drop for CancelOnDrop { fn drop(&mut self) { self.0.store(true, Ordering::SeqCst); } }
#[derive(Clone)]
struct Check { progress:Option<Arc<AtomicUsize>>, target: Option<windows::Window>, cancel: Arc<AtomicBool>, dropped: Arc<AtomicBool>, epoch: u64, started: Instant }
impl Check {
    fn check(&self) -> anyhow::Result<()> {
        self.check_probe(|| {
            if let Some(target)=&self.target { windows::validate(target,true)?; }
            Ok(())
        })
    }
    fn check_probe(&self, probe:impl FnOnce()->anyhow::Result<()>) -> anyhow::Result<()> {
        let active=|| -> anyhow::Result<()> {
            ensure!(!self.cancel.load(Ordering::SeqCst) && !self.dropped.load(Ordering::SeqCst) && self.epoch == STOP.load(Ordering::SeqCst) && self.started.elapsed() < DEADLINE, "Computer operation stopped; partial input may have been delivered");
            Ok(())
        };
        active()?;
        probe()?;
        // Native foreground probes can block; revocation during a probe must
        // not admit the input event that follows it.
        active()
    }
}
fn run_worker<T>(epoch: u64, cancel: Arc<AtomicBool>, dropped: Arc<AtomicBool>, backend: impl FnOnce(&Check) -> anyhow::Result<T>) -> anyhow::Result<T> {
    // Never reload STOP here: an admission can have been revoked before this
    // blocking worker was scheduled. Tests use this same worker entry point.
    let check = Check { progress:None, target:None, epoch, cancel, dropped, started: Instant::now() };
    check.check()?;
    serialized(&SERIAL, || backend(&check))
}
// Catch while the unit-lock guard is OUTSIDE the unwind boundary: a native
// parser panic cannot poison ownership. A held lock still means genuinely busy.
fn serialized<T>(lock:&Mutex<()>,body:impl FnOnce()->anyhow::Result<T>)->anyhow::Result<T> {
    let _guard=match lock.try_lock() {
        Ok(guard)=>guard,
        Err(std::sync::TryLockError::WouldBlock)=>bail!("Computer is busy; active native operation has not returned"),
        Err(std::sync::TryLockError::Poisoned(error))=>{
            let guard=error.into_inner();
            invalidate_frame();
            lock.clear_poison(); // No protected data: this mutex only owns execution.
            guard
        }
    };
    native_boundary(body)
}
fn invalidate_frame() {
    let mut frame=FRAME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    *frame=None;
    FRAME.clear_poison();
}
fn native_boundary<T>(body:impl FnOnce()->anyhow::Result<T>)->anyhow::Result<T> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)) {
        Ok(result)=>result,
        Err(_)=>{
            invalidate_frame();
            anyhow::bail!("Native computer operation panicked; cleanup attempted and frame invalidated. Input may be partially delivered; observe before retrying")
        }
    }
}
fn execute(tool: &str, arguments: Value, check: &Check) -> anyhow::Result<BuiltinMcpCallResult> {
    check.check()?;
    if tool == "capabilities" {
        let mut capabilities = platform::capabilities();
        capabilities["sessionApprovalRequired"] = json!(true);
        check.check()?;
        return Ok(text(capabilities));
    }
    execute_locked(tool, arguments, check)
}

// Only run_worker owns SERIAL. Nested batch operations never re-lock it.
fn execute_locked(tool: &str, mut arguments: Value, check: &Check) -> anyhow::Result<BuiltinMcpCallResult> {
    check.check()?;
    if tool == "windows" {
        ensure!(arguments.as_object().is_some_and(|v|v.is_empty()),"windows accepts an empty object only");
        let result=windows::list()?;
        check.check()?;
        return Ok(text(result));
    }
    if tool == "focus" {
        #[derive(Deserialize)] #[serde(deny_unknown_fields)] struct Args { target:String }
        let args:Args=serde_json::from_value(arguments)?;
        *FRAME.lock().map_err(|_|anyhow::anyhow!("Frame state poisoned"))?=None;
        check.check()?;
        windows::focus(&args.target, || check.check())?;
        return Ok(text(json!({"focused":true,"target":args.target,"binding":"explicit-per-call","note":"Foreground confirmed now, not reserved. Pass this target to every input/batch and screenshot. No implicit focus inheritance; never automatically refocus after user interference."})));
    }
    if tool == "wait" {
        #[derive(Deserialize)] #[serde(rename_all="snake_case")] enum Condition { Foreground, Closed }
        #[derive(Deserialize)] #[serde(deny_unknown_fields)] struct Args { target:String, condition:Condition, timeout_ms:u64 }
        let args:Args=serde_json::from_value(arguments)?;
        ensure!((100..=3000).contains(&args.timeout_ms),"Wait timeout must be 100..3000 ms");
        let target=windows::resolve(&args.target)?;
        let start=Instant::now();
        loop {
            check.check()?;
            if windows::condition(&target,matches!(args.condition,Condition::Closed))? {
                check.check()?;
                return Ok(text(json!({"matched":true,"elapsedMs":start.elapsed().as_millis(),"note":"Observed condition only; not evidence of application task completion"})));
            }
            if start.elapsed()>=Duration::from_millis(args.timeout_ms) { return Ok(text(json!({"matched":false,"timedOut":true}))); }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    if tool == "batch" { return batch(arguments,check); }
    validate_input_scope(tool, &mut arguments)?;
    let target=arguments.as_object_mut().and_then(|v|v.remove("target")).filter(|v|!v.is_null()).map(|v| {
        windows::resolve(v.as_str().context("target must be a window token")?)
    }).transpose()?;
    let scoped = Check { progress:check.progress.clone(), cancel:check.cancel.clone(), dropped:check.dropped.clone(),epoch:check.epoch,started:check.started,target };
    let check = if scoped.target.is_some() { &scoped } else { check };
    check.check()?;
    let displays = platform::displays()?;
    check.check()?;
    match tool {
        "screenshot" => {
            #[derive(Deserialize)] #[serde(deny_unknown_fields)] struct Args { display: String, #[serde(default="default_settle_ms")] settle_ms:u64 }
            let args: Args = serde_json::from_value(arguments)?;
            let display = displays.into_iter().find(|d| d.id == args.display).context("Unknown display; call capabilities again")?;
            ensure!(u64::from(display.width) * u64::from(display.height) <= 64_000_000, "Display exceeds capture limit");
            ensure!(args.settle_ms<=3000,"settle_ms must be 0..3000");
            let (image,settling) = capture_settled(args.settle_ms,||check.check(),||platform::capture(&display))?;
            ensure!(platform::displays()?.contains(&display),"Display topology changed during capture; observe again");
            check.check()?;
            let image = image.thumbnail(1600, 1600);
            let mut bytes = Cursor::new(Vec::new());
            image.write_to(&mut bytes, image_rs::ImageFormat::Png)?;
            ensure!(bytes.get_ref().len() <= 8 * 1024 * 1024, "Screenshot exceeds encoded size limit");
            let current_target=check.target.as_ref().map(|target|windows::validate(target,true)).transpose()?;
            let frame = Frame { target:current_target, id: format!("{:032x}", rand::random::<u128>()), display, width: image.width(), height: image.height(), created: Instant::now(), epoch: check.epoch };
            check.check()?;
            let mut result = text(json!({"frame":frame.id,"display":frame.display,"imageWidth":frame.width,"imageHeight":frame.height,"coordinates":"image pixels, origin top-left; pass frame unchanged","expiresAfterSeconds":30,"targetWindow":frame.target,"settling":settling}));
            *FRAME.lock().map_err(|_| anyhow::anyhow!("Frame state poisoned"))? = Some(frame);
            result.content.push(BuiltinMcpContent::Image { data: STANDARD.encode(bytes.into_inner()), mime_type: "image/png".into(), annotations: None });
            Ok(result)
        }
        "input" => {
            let action: Action = serde_json::from_value(arguments)?;
            validate(&action)?;
            #[cfg(target_os="linux")]
            if let Action::Paste {ref text,ref paste_shortcut,..}=action {
                // Every failure from publication onward is conservatively partial:
                // clipboard ownership may have changed before an ACK/guard fails.
                let result=(|| {
                    linux_clipboard::publish(text,||check.check())?;
                    check.check()?;
                    linux_text::send_keys(&paste_shortcut.keys(),||check.check())?;
                    check.check()?;
                    Ok::<_,anyhow::Error>(())
                })();
                result.inspect_err(|_|invalidate_frame()).context("Paste failed: clipboard may have changed and input may have arrived. Observe before any deliberate retry; do not blindly retry")?;
                return Ok(self::text(json!({"status":"dispatched_unverified","applicationVerified":false,"method":"wayland_clipboard_paste","clipboardChanged":true,"clipboardPolicy":"replace","note":"Clipboard replaced; history may retain text. Source remains until another owner replaces it. Native dispatch does not prove the application consumed the paste. No automatic restoration or retry."})));
            }
            #[cfg(target_os="linux")]
            if let Action::Text { text: ref value } = action {
                linux_text::send(value, || check.check()).inspect_err(|_|invalidate_frame())?;
                check.check()?;
                return Ok(dispatched("wayland.virtual_keyboard.text"));
            }
            #[cfg(target_os="linux")]
            if let Action::Key { ref key, ref modifiers } = action {
                let mut keys=modifiers.iter().map(|m|modifier_key(m)).collect::<anyhow::Result<Vec<_>>>()?;
                keys.push(named_key(key)?);
                // Resolve and inject on one owned native map. Creating Enigo
                // first can itself change the seat map seen by another client.
                linux_text::send_keys(&keys,||check.check()).inspect_err(|_|invalidate_frame())?;
                check.check()?;
                return Ok(dispatched("wayland.virtual_keyboard.keys"));
            }
            #[cfg(target_os="linux")]
            {
                use linux_pointer::{Command, Point};
                let point=|id:&str,x,y|->anyhow::Result<(Display,Point)> {
                    let frame=coordinate_frame(id,x,y,&displays,check)?;
                    Ok((frame.display,Point{x,y,width:frame.width,height:frame.height}))
                };
                let (display,command)=match action {
                    Action::Move {frame,x,y}=>{let (d,p)=point(&frame,x,y)?;(d,Command::Move(p))},
                    Action::Click {frame,x,y,button}=>{let (d,p)=point(&frame,x,y)?;(d,Command::Click(p,match button {MouseButton::Left=>0x110,MouseButton::Right=>0x111,MouseButton::Middle=>0x112}))},
                    Action::Drag {frame,x,y,to_x,to_y}=>{let (d,a)=point(&frame,x,y)?;let (_,b)=point(&frame,to_x,to_y)?;(d,Command::Drag(a,b))},
                    Action::Scroll {amount,horizontal}=>{
                        ensure!(displays.len()==1,"Wayland pointer control requires exactly one output");
                        (displays[0].clone(),Command::Scroll{amount,horizontal})
                    },
                    Action::Text {..}|Action::Key {..}|Action::Paste {..}=>unreachable!("Linux keyboard dispatched above"),
                };
                linux_pointer::send(&display,command,||check.check()).inspect_err(|_|invalidate_frame())?;
                return Ok(dispatched("wayland.wlr_virtual_pointer"));
            }
            #[cfg(not(target_os="linux"))]
            {
            // Do not silently fall back to XWayland: it cannot control native windows.
            // Explicit chord/button guards own releases. Avoid Enigo re-entering
            // a panicking keymap parser from its Drop during an existing unwind.
            let mut input = Enigo::new(&Settings { open_prompt_to_get_permissions: false, release_keys_when_dropped:false, ..Settings::default() }).context("Native input unavailable (macOS requires Accessibility permission; Wayland requires virtual input protocols)")?;
            check.check()?;
            match action {
                Action::Move { frame, x, y } => { let (x,y) = coordinates(&frame,x,y,&displays,&input,check)?; check.check()?; platform::move_pointer(&mut input,x,y)?; }
                Action::Click { frame, x, y, button } => {
                    let (x,y) = coordinates(&frame,x,y,&displays,&input,check)?;
                    let button = match button { MouseButton::Left => Button::Left, MouseButton::Right => Button::Right, MouseButton::Middle => Button::Middle };
                    check.check()?;
                    platform::move_pointer(&mut input,x,y)?;
                    check.check()?;
                    // Always attempt release, including failed presses/cancellation.
                    with_button(&mut input,button,|_| Ok(()))?;
                }
                Action::Drag { frame, x, y, to_x, to_y } => {
                    let start = coordinates(&frame,x,y,&displays,&input,check)?;
                    let end = coordinates(&frame,to_x,to_y,&displays,&input,check)?;
                    check.check()?;
                    platform::move_pointer(&mut input,start.0,start.1)?;
                    check.check()?;
                    with_button(&mut input,Button::Left,|input| {
                        for step in 1..=20 {
                            check.check()?;
                            let axis = |a:i32,b:i32| (i64::from(a) + (i64::from(b)-i64::from(a))*step/20) as i32;
                            platform::move_pointer(input,axis(start.0,end.0),axis(start.1,end.1))?;
                            std::thread::sleep(Duration::from_millis(16));
                        }
                        Ok(())
                    })?;
                }
                Action::Scroll { amount, horizontal } => { input.scroll(amount,if horizontal { Axis::Horizontal } else { Axis::Vertical })?; }
                Action::Paste {..}=>bail!("Explicit clipboard paste is currently supported only on Linux Wayland"),
                Action::Text { text } => {
                    #[cfg(target_os="linux")] linux_text::send(&text, || check.check())?;
                    #[cfg(not(target_os="linux"))] for ch in text.chars() { check.check()?; platform::type_character(&mut input,ch)?; }
                }
                Action::Key { key, modifiers } => {
                    let mut keys = modifiers.iter().map(|m| modifier_key(m)).collect::<anyhow::Result<Vec<_>>>()?;
                    keys.push(named_key(&key)?);
                    let native = shortcuts::resolve(&keys)?;
                    shortcuts::send(&mut input, &native, || check.check())?;
                }
            }
            check.check()?;
            Ok(dispatched(platform::NAME))
            }
        }
        _ => bail!("Unknown computer tool {tool}"),
    }
}
// No remembered foreground: the authorized caller must name its intended window
// on EVERY input call. Desktop scope is an explicit escape hatch for individual
// global shortcuts/pointer actions, never literal typing or blind batches.
fn validate_input_scope(tool:&str, arguments:&mut Value)->anyhow::Result<()> {
    if tool != "input" { return Ok(()); }
    let object=arguments.as_object_mut().context("Input must be an object")?;
    let scope=object.remove("scope");
    let desktop=scope.as_ref().is_some_and(|s|s=="desktop");
    ensure!(scope.is_none() || desktop,"Only explicit scope:desktop is supported; otherwise supply target");
    let target=object.get("target");
    if desktop {
        ensure!(target.is_none(),"scope:desktop and target are mutually exclusive");
        ensure!(matches!(object.get("action").and_then(Value::as_str),Some("key"|"move"|"click"|"drag"|"scroll")),"Desktop scope forbids text and paste; list/focus a window and pass its target");
    } else {
        ensure!(target.and_then(Value::as_str).is_some_and(|t|!t.is_empty()),"Input requires an explicit window target even after focus. Foreground is not inherited. For deliberate global shortcuts/pointer actions only, use scope:desktop");
    }
    Ok(())
}

fn default_settle_ms()->u64 { 1200 }
// Visual quiescence only. A cursor can blink; workspace motion changes a
// material fraction of this small full-display sample. Never infer page load.
#[derive(Default)]
struct Settling { previous:Option<Vec<u8>>,quiet_since:Duration }
impl Settling {
    fn observe(&mut self,elapsed:Duration,sample:Vec<u8>)->bool {
        let changed=self.previous.as_ref().is_none_or(|old| {
            old.len()!=sample.len() || old.iter().zip(&sample).filter(|(a,b)|a.abs_diff(**b)>12).count()>sample.len()/200
        });
        if changed { self.quiet_since=elapsed; self.previous=Some(sample); }
        // Keep the quiet-period anchor, not just the preceding frame: slow
        // cumulative motion must also restart settling.
        elapsed>=Duration::from_millis(300) && elapsed.saturating_sub(self.quiet_since)>=Duration::from_millis(250)
    }
}
fn capture_settled(ms:u64,mut check:impl FnMut()->anyhow::Result<()>,mut capture:impl FnMut()->anyhow::Result<image_rs::DynamicImage>)->anyhow::Result<(image_rs::DynamicImage,Value)> {
    ensure!(ms<=3000,"settle_ms must be 0..3000");
    let start=Instant::now();
    let mut state=Settling::default();
    let mut captures=0;
    loop {
        check()?;
        let image=capture()?;
        check()?;
        captures+=1;
        let elapsed=start.elapsed();
        let stable=state.observe(elapsed,image.resize_exact(64,64,image_rs::imageops::FilterType::Triangle).into_rgb8().into_raw());
        if ms==0 || stable || elapsed>=Duration::from_millis(ms) {
            return Ok((image,json!({"settled":ms!=0 && stable,"timedOut":ms!=0 && !stable,"elapsedMs":elapsed.as_millis(),"captures":captures,"note":"Visual stability only; not application readiness"})));
        }
        // Check cancellation at <=25ms while waiting between 100ms probes.
        let until=(start.elapsed()+Duration::from_millis(100)).min(Duration::from_millis(ms));
        while start.elapsed()<until { check()?; std::thread::sleep(Duration::from_millis(25).min(until.saturating_sub(start.elapsed()))); }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Batch { actions: Vec<Value>, target: Option<String>, screenshot: Option<String>, #[serde(default="default_settle_ms")] settle_ms:u64 }
fn validate_batch(args:&Batch)->anyhow::Result<()> {
    ensure!(args.target.as_deref().is_some_and(|t|!t.is_empty()),"Batch requires an explicit window target even after focus; no implicit foreground inheritance or desktop batches");
    ensure!(args.settle_ms<=3000,"settle_ms must be 0..3000");
    ensure!(!args.actions.is_empty() && args.actions.len()<=16,"Batch requires 1..16 actions");
    let mut chars=0;
    for value in &args.actions {
        let action:Action=serde_json::from_value(value.clone())?;
        validate(&action)?;
        if let Action::Text { text }|Action::Paste {text,..}=action { chars+=text.chars().count(); }
    }
    ensure!(chars<=MAX_TEXT,"Batch text total exceeds 512 characters");
    Ok(())
}
// Generic loop keeps cancellation/partial-progress tests off the real desktop.
fn sequence(count:usize, mut step:impl FnMut(usize)->anyhow::Result<()>)->(usize,Option<String>) {
    for index in 0..count {
        if let Err(error)=step(index) { return (index,Some(format!("{error:#}"))); }
    }
    (count,None)
}
fn batch(arguments:Value,check:&Check)->anyhow::Result<BuiltinMcpCallResult> {
    batch_with(arguments,check,execute_locked)
}
fn batch_with(arguments:Value,check:&Check,mut operation:impl FnMut(&str,Value,&Check)->anyhow::Result<BuiltinMcpCallResult>)->anyhow::Result<BuiltinMcpCallResult> {
    let args:Batch=serde_json::from_value(arguments)?;
    validate_batch(&args)?; // Reject the entire malformed batch before side effects.
    let mut clipboard_changed=false;
    let mut clipboard_uncertain=false;
    let (completed,error)=sequence(args.actions.len(),|index| {
        check.check()?;
        let mut action=args.actions[index].clone();
        if let Some(target)=&args.target { action["target"]=json!(target); }
        let paste=action["action"]=="paste";
        if paste {clipboard_uncertain=true;}
        let dispatched=native_boundary(||operation("input",action,check))?;
        ensure!(dispatched.is_error!=Some(true),"Native action returned an error; application effects are unknown");
        if paste {clipboard_changed=true;clipboard_uncertain=false;}
        if let Some(progress)=&check.progress { progress.store(index+1,Ordering::SeqCst); }
        Ok(())
    });
    let mut summary=json!({"status":if error.is_some() {"partial_unknown"} else {"dispatched_unverified"},"applicationVerified":false,"completed":completed,"total":args.actions.len(),"failedIndex":error.as_ref().map(|_|completed),"error":error,"partialActionPossible":error.is_some(),"note":"Zero-based failedIndex; completed means native dispatch, not application consumption. No rollback or automatic retry."});
    clipboard_effects(&mut summary,clipboard_changed,clipboard_uncertain);
    let mut result=text(summary);
    result.is_error=error.as_ref().map(|_|true);
    if let Some(display)=args.screenshot {
        // Never capture after cancellation/revocation. A native failure may still
        // permit observation; capture failure must not hide delivered progress.
        match check.check().and_then(|_|native_boundary(||operation("screenshot",json!({"display":display,"target":args.target,"settle_ms":args.settle_ms}),check))) {
            Ok(image)=>result.content.extend(image.content),
            Err(e)=>{ result.content.extend(text(json!({"screenshotError":format!("{e:#}")})).content); result.is_error=Some(true); }
        }
    }
    Ok(result)
}

#[cfg(any(test, not(target_os="linux")))]
struct ButtonRelease<'a, M: Mouse> { input: &'a mut M, button: Button, released: bool }
#[cfg(any(test, not(target_os="linux")))]
impl<M: Mouse> Drop for ButtonRelease<'_, M> {
    fn drop(&mut self) { if !self.released { let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(||self.input.button(self.button,Direction::Release))); } }
}
#[cfg(any(test, not(target_os="linux")))]
fn with_button<M: Mouse>(input:&mut M,button:Button,action:impl FnOnce(&mut M)->anyhow::Result<()>) -> anyhow::Result<()> {
    let mut guard = ButtonRelease { input, button, released:false };
    let result = guard.input.button(button,Direction::Press).map_err(anyhow::Error::from).and_then(|_| action(guard.input));
    // Release on partial press, cancellation, error AND unwinding.
    let released = guard.input.button(button,Direction::Release);
    guard.released = released.is_ok();
    result?;
    released?;
    Ok(())
}

fn validate(action: &Action) -> anyhow::Result<()> {
    match action {
        Action::Text { text } => {
            ensure!(text.chars().count() <= MAX_TEXT && !text.chars().any(char::is_control), "Text must contain at most 512 characters and no C0/C1 control characters; use explicit key actions for Return/Tab");
            #[cfg(target_os="linux")]
            ensure!(text.chars().all(|c|u32::from(c)<=0xffff),"Linux native text cannot faithfully dispatch non-BMP characters; choose explicit clipboard paste with replacement policy if appropriate. No input was sent");
        },
        Action::Paste {text,clipboard_policy,..}=>{
            ensure!(clipboard_policy=="replace","Paste requires explicit clipboard_policy:replace");
            ensure!(text.chars().count()<=MAX_TEXT && !text.contains('\0'),"Paste must contain at most 512 Unicode characters and no NUL");
            #[cfg(not(target_os="linux"))]
            bail!("Explicit clipboard paste is currently supported only on Linux Wayland");
        },
        Action::Scroll { amount, .. } => ensure!((-20..=20).contains(amount), "Scroll amount outside -20..20"),
        Action::Key { key, modifiers } => {
            named_key(key)?;
            ensure!(modifiers.len() <= 4, "At most four modifiers");
            let mut seen = std::collections::HashSet::new();
            for m in modifiers { ensure!(seen.insert(modifier_key(m)?), "Duplicate modifier (including aliases)"); }
        }
        _ => {}
    }
    Ok(())
}
fn modifier_key(key: &str) -> anyhow::Result<Key> {
    Ok(match key.trim().to_ascii_lowercase().as_str() {
        "control" | "ctrl" | "control_l" | "ctrl_l" => Key::Control,
        "alt" | "option" | "alt_l" => Key::Alt,
        "shift" | "shift_l" => Key::Shift,
        "meta" | "super" | "super_l" | "win" | "windows" | "logo" | "cmd" | "command" => Key::Meta,
        _ => bail!("Unknown modifier {key}; use control/ctrl, alt/option, shift, or meta/super/command"),
    })
}
fn named_key(key: &str) -> anyhow::Result<Key> {
    if let Ok(modifier) = modifier_key(key) { return Ok(modifier); }
    let normalized = key.trim().to_ascii_lowercase();
    Ok(match normalized.as_str() {
        "enter" | "return" => Key::Return, "tab" => Key::Tab, "escape" | "esc" => Key::Escape, "backspace" | "bs" => Key::Backspace, "delete" | "del" => Key::Delete, "space" | "spacebar" => Key::Space,
        "up" | "arrowup" => Key::UpArrow, "down" | "arrowdown" => Key::DownArrow, "left" | "arrowleft" => Key::LeftArrow, "right" | "arrowright" => Key::RightArrow,
        "home" => Key::Home, "end" => Key::End, "page_up" | "pageup" | "pgup" | "prior" => Key::PageUp, "page_down" | "pagedown" | "pgdn" | "next" => Key::PageDown,
        s if s.len() == 1 && s.as_bytes()[0].is_ascii_alphanumeric() => {
            // Windows Unicode key fallback can type text on BOTH press and
            // release. Shortcut keys use stable virtual-key codes instead.
            #[cfg(target_os = "windows")]
            { Key::Other(u32::from(s.as_bytes()[0].to_ascii_uppercase())) }
            #[cfg(not(target_os = "windows"))]
            { Key::Unicode(s.chars().next().unwrap().to_ascii_lowercase()) }
        },
        _ => bail!("Unknown key {key}"),
    })
}
#[cfg(not(target_os="linux"))]
fn coordinates(id: &str, x: u32, y: u32, displays: &[Display], input: &Enigo, check: &Check) -> anyhow::Result<(i32,i32)> {
    let frame=coordinate_frame(id,x,y,displays,check)?;
    platform::coordinates(&frame.display, x, y, frame.width, frame.height, displays, input)
}
fn coordinate_frame(id:&str,x:u32,y:u32,displays:&[Display],check:&Check)->anyhow::Result<Frame> {
    let frame = FRAME.lock().map_err(|_| anyhow::anyhow!("Frame state poisoned"))?.clone().context("Take a screenshot first")?;
    validate_frame(&frame,id,x,y,displays,check.epoch)?;
    ensure!(windows::same_target(frame.target.as_ref(),check.target.as_ref()),"Screenshot target differs from input target; take a new screenshot with the same target");
    Ok(frame)
}
fn validate_frame(frame:&Frame,id:&str,x:u32,y:u32,displays:&[Display],epoch:u64) -> anyhow::Result<()> {
    ensure!(frame.id == id && frame.epoch == epoch && frame.created.elapsed() < Duration::from_secs(30), "Stale screenshot frame; take another screenshot");
    ensure!(displays.contains(&frame.display), "Display geometry changed; take another screenshot");
    ensure!(x < frame.width && y < frame.height, "Coordinates outside screenshot");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disable_between_enablement_read_and_worker_creation_delivers_zero_inputs() {
        let _revocation = TEST_REVOCATION_LOCK.blocking_lock();
        use std::sync::Barrier;
        let read = Barrier::new(2);
        let resume = Barrier::new(2);
        let enabled = AtomicBool::new(true);
        let backend_inputs = AtomicU64::new(0);
        std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                let (epoch, was_enabled) = admit(|| {
                    let value = enabled.load(Ordering::SeqCst);
                    read.wait(); // The enablement read succeeded; no worker yet.
                    resume.wait();
                    Ok(value)
                }).unwrap();
                assert!(was_enabled);
                run_worker(epoch, Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)), |_| {
                    backend_inputs.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            });
            read.wait();
            enabled.store(false, Ordering::SeqCst);
            stop(); // Same order as the config-disable route: persist, then revoke.
            resume.wait();
            assert!(worker.join().unwrap().is_err());
        });
        assert_eq!(backend_inputs.load(Ordering::SeqCst), 0);
    }
    #[derive(Default)] struct FakeMouse { events:Vec<Direction>, fail_press:bool }
    impl Mouse for FakeMouse {
        fn button(&mut self,_:Button,direction:Direction) -> enigo::InputResult<()> {
            self.events.push(direction);
            if self.fail_press && direction == Direction::Press { return Err(enigo::InputError::Simulate("partial press")); }
            Ok(())
        }
        fn move_mouse(&mut self,_:i32,_:i32,_:enigo::Coordinate) -> enigo::InputResult<()> { Ok(()) }
        fn scroll(&mut self,_:i32,_:Axis) -> enigo::InputResult<()> { Ok(()) }
        fn main_display(&self) -> enigo::InputResult<(i32,i32)> { Ok((1920,1080)) }
        fn location(&self) -> enigo::InputResult<(i32,i32)> { Ok((0,0)) }
    }
    #[test] fn button_release_survives_errors_and_unwinding() {
        for fail_press in [false,true] {
            let mut mouse = FakeMouse { fail_press,..Default::default() };
            assert!(with_button(&mut mouse,Button::Left,|_| bail!("cancelled")).is_err());
            assert_eq!(mouse.events,vec![Direction::Press,Direction::Release]);
        }
        let mut mouse = FakeMouse::default();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| with_button(&mut mouse,Button::Left,|_| panic!("worker panic"))));
        assert_eq!(mouse.events,vec![Direction::Press,Direction::Release]);
    }
    #[tokio::test] async fn unauthorized_async_calls_never_reach_backend() {
        for tool in ["input","batch","windows","focus","wait","screenshot"] {
            let result = ComputerUse.call_tool_authorized_async(Path::new("/"),tool,json!({}),false,Arc::new(AtomicBool::new(false)),None).await;
            assert!(result.is_err(),"{tool} must require session authorization");
        }
    }
    #[test]
    fn common_key_aliases_and_modifier_aliases_are_normalized() {
        for alias in ["Super_L", "super", "META", "win", "windows", "Command", "cmd", "logo"] {
            assert_eq!(modifier_key(alias).unwrap(), Key::Meta);
            assert_eq!(named_key(alias).unwrap(), Key::Meta);
        }
        for (alias, key) in [("Control_L", Key::Control), ("CTRL", Key::Control), ("Alt_L", Key::Alt), ("option", Key::Alt), ("Shift_L", Key::Shift), ("Return", Key::Return), ("Esc", Key::Escape), ("ArrowLeft", Key::LeftArrow), ("PageDown", Key::PageDown)] {
            assert_eq!(named_key(alias).unwrap(), key);
        }
        assert!(validate(&Action::Key { key: "r".into(), modifiers: vec!["super".into(), "Super_L".into()] }).is_err());
        assert!(validate(&Action::Key { key: "l".into(), modifiers: vec!["CTRL".into()] }).is_ok());
        assert!(modifier_key("not_a_modifier").is_err());
    }
    #[tokio::test] async fn native_panic_unwinds_worker_and_next_operation_succeeds() {
        let _revocation=TEST_REVOCATION_LOCK.lock().await;
        let released=AtomicBool::new(false);
        struct Cleanup<'a>(&'a AtomicBool);
        impl Drop for Cleanup<'_> { fn drop(&mut self) {self.0.store(true,Ordering::SeqCst);} }
        let run=|panic:bool|run_worker(STOP.load(Ordering::SeqCst),Arc::new(AtomicBool::new(false)),Arc::new(AtomicBool::new(false)),|_| {
            let _cleanup=Cleanup(&released);
            if panic { panic!("native parser regression"); }
            Ok(42)
        });
        assert!(run(true).unwrap_err().to_string().contains("panicked"));
        assert!(released.load(Ordering::SeqCst));
        assert!(!SERIAL.is_poisoned());
        assert_eq!(run(false).unwrap(),42);
        with_test_backend(Arc::new(|tool,_| {
            if tool=="input" {panic!("native parser panic through async worker");}
            assert_eq!(tool,"screenshot");
            Ok(text(json!({"mockCapture":true})))
        }),async {
            let epoch=STOP.load(Ordering::SeqCst);
            let failed=ComputerUse.call_tool_authorized_async(Path::new("/"),"input",json!({}),true,Arc::new(AtomicBool::new(false)),Some(epoch)).await;
            assert!(failed.is_err());
            let next=ComputerUse.call_tool_authorized_async(Path::new("/"),"screenshot",json!({}),true,Arc::new(AtomicBool::new(false)),Some(epoch)).await;
            assert!(next.is_ok(),"next async screenshot worker must not stay busy/poisoned");
        }).await;
        let lock=Mutex::new(());
        let _=std::panic::catch_unwind(|| { let _guard=lock.lock().unwrap(); panic!("old poisoned unit lock"); });
        assert!(lock.is_poisoned());
        assert_eq!(serialized(&lock,||Ok(7)).unwrap(),7);
        assert!(!lock.is_poisoned());
        let guard=lock.lock().unwrap();
        assert!(serialized(&lock,||Ok(7)).unwrap_err().to_string().contains("busy"));
        drop(guard);
        assert_eq!(serialized(&lock,||Ok(8)).unwrap(),8);
    }
    #[test] fn visual_settling_restarts_after_animation_and_ignores_tiny_cursor_changes() {
        let mut settle=Settling::default();
        assert!(!settle.observe(Duration::ZERO,vec![0;1000]));
        assert!(!settle.observe(Duration::from_millis(100),vec![100;1000]));
        assert!(!settle.observe(Duration::from_millis(300),vec![200;1000]));
        assert!(!settle.observe(Duration::from_millis(500),vec![200;1000]));
        let mut cursor=vec![200;1000]; cursor[0]=0;
        assert!(settle.observe(Duration::from_millis(600),cursor));
        let mut slow=Settling::default();
        assert!(!slow.observe(Duration::ZERO,vec![0;1000]));
        assert!(!slow.observe(Duration::from_millis(100),vec![10;1000]));
        assert!(!slow.observe(Duration::from_millis(200),vec![20;1000]));
        assert!(!slow.observe(Duration::from_millis(400),vec![20;1000]));
        assert!(slow.observe(Duration::from_millis(500),vec![20;1000]));
    }
    #[test] fn settling_is_bounded_and_cancellation_prevents_followup_capture() {
        let image=||image_rs::DynamicImage::new_rgb8(2,2);
        assert!(capture_settled(3001,||Ok(()),||panic!("invalid budget captured")).is_err());
        let (_,status)=capture_settled(0,||Ok(()),||Ok(image())).unwrap();
        assert_eq!(status["captures"],1); assert_eq!(status["settled"],false);
        let (_,status)=capture_settled(1,||Ok(()),||Ok(image())).unwrap();
        assert_eq!(status["timedOut"],true);
        let mut checks=0;
        let mut captures=0;
        assert!(capture_settled(1200,||{checks+=1;ensure!(checks<3,"cancelled during settle");Ok(())},||{captures+=1;Ok(image())}).is_err());
        assert_eq!(captures,1);
    }
    #[test] fn batch_bounds_preflight_and_partial_failure() {
        let parse=|mut v:Value| { v["target"]=json!("helium"); serde_json::from_value::<Batch>(v).unwrap() };
        for value in [json!({"actions":[]}),json!({"actions":vec![json!({"action":"key","key":"a"});17]}),json!({"actions":[{"action":"text","text":"x".repeat(300)},{"action":"text","text":"y".repeat(300)}]}),json!({"actions":[{"action":"text","text":"ok"},{"action":"batch"}]})] {
            assert!(validate_batch(&parse(value)).is_err());
        }
        assert!(validate_batch(&parse(json!({"actions":[{"action":"text","text":"λ".repeat(512)}]}))).is_ok());
        let mut visited=Vec::new();
        let (completed,error)=sequence(5,|index| { visited.push(index); ensure!(index!=2,"partial native failure"); Ok(()) });
        assert_eq!(visited,vec![0,1,2]); assert_eq!(completed,2); assert!(error.unwrap().contains("partial native failure"));
    }
    #[tokio::test] async fn batch_stops_before_next_action_on_cancel_or_revocation() {
        let _revocation=TEST_REVOCATION_LOCK.lock().await;
        for revoke in [false,true] {
            let check=Check { progress:None,target:None,cancel:Arc::new(AtomicBool::new(false)),dropped:Arc::new(AtomicBool::new(false)),epoch:STOP.load(Ordering::SeqCst),started:Instant::now() };
            let mut delivered=0;
            let (completed,error)=sequence(3,|_| {
                check.check()?;
                delivered+=1;
                if revoke { stop(); } else { check.cancel.store(true,Ordering::SeqCst); }
                Ok(())
            });
            assert_eq!(delivered,1); assert_eq!(completed,1); assert!(error.is_some());
        }
    }
    #[tokio::test] async fn batch_returns_progress_and_final_image_but_never_captures_after_cancel() {
        let _revocation=TEST_REVOCATION_LOCK.lock().await;
        for (cancel,panics) in [(false,false),(true,false),(false,true)] {
            let check=Check { progress:Some(Arc::new(AtomicUsize::new(0))),target:None,cancel:Arc::new(AtomicBool::new(false)),dropped:Arc::new(AtomicBool::new(false)),epoch:STOP.load(Ordering::SeqCst),started:Instant::now() };
            let mut input_count=0;
            let mut captures=0;
            let result=batch_with(json!({"target":"helium","actions":[{"action":"text","text":"one"},{"action":"text","text":"two"},{"action":"text","text":"never"}],"screenshot":"display"}),&check,|tool,args,check| {
                if tool=="input" {
                    input_count+=1;
                    if input_count==2 {
                        if cancel { check.cancel.store(true,Ordering::SeqCst); }
                        if panics { panic!("partial native panic"); }
                        bail!("partial native error");
                    }
                    Ok(dispatched("test.native"))
                } else {
                    captures+=1;
                    assert_eq!(args["display"],"display");
                    let mut result=text(json!({"frame":"new-frame","imageWidth":10,"imageHeight":10}));
                    result.content.push(BuiltinMcpContent::Image { data:"mock-only".into(),mime_type:"image/png".into(),annotations:None });
                    Ok(result)
                }
            }).unwrap();
            assert_eq!(input_count,2); assert_eq!(captures,usize::from(!cancel));
            assert_eq!(check.progress.unwrap().load(Ordering::SeqCst),1);
            assert_eq!(result.is_error,Some(true));
            let BuiltinMcpContent::Text { text:progress,.. }=&result.content[0] else { panic!("missing progress") };
            let progress:Value=serde_json::from_str(progress).unwrap();
            assert_eq!(progress["completed"],1); assert_eq!(progress["failedIndex"],1);
            assert_eq!(result.content.iter().any(|c|matches!(c,BuiltinMcpContent::Image {..})),!cancel);
        }
    }
    #[test] fn explicit_target_never_inherits_another_calls_focus() {
        // Even after focus succeeds in this OR another session, nothing is
        // remembered. Admission depends solely on this call's arguments.
        for action in [json!({"action":"text","text":"hello"}),json!({"action":"key","key":"enter"})] {
            assert!(validate_input_scope("input",&mut action.clone()).is_err());
            let mut explicit=action.clone(); explicit["target"]=json!("helium");
            assert!(validate_input_scope("input",&mut explicit).is_ok());
        }
        assert!(validate_input_scope("input",&mut json!({"scope":"desktop","action":"text","text":"unsafe"})).is_err());
        assert!(validate_input_scope("input",&mut json!({"scope":"desktop","target":"helium","action":"key","key":"tab"})).is_err());
        assert!(validate_input_scope("input",&mut json!({"scope":"desktop","action":"key","key":"tab","modifiers":["alt"]})).is_ok());
        assert!(validate_batch(&serde_json::from_value(json!({"actions":[{"action":"text","text":"hello"}]})).unwrap()).is_err());
        assert!(windows::resolve("unknown-or-other-session-unprovided-token").is_err());
    }
    #[tokio::test] async fn foreground_loss_stops_batch_and_binds_recovery_screenshot() {
        let _revocation=TEST_REVOCATION_LOCK.lock().await;
        let check=Check { progress:None,target:None,cancel:Arc::new(AtomicBool::new(false)),dropped:Arc::new(AtomicBool::new(false)),epoch:STOP.load(Ordering::SeqCst),started:Instant::now() };
        let helium=windows::Window {id:"helium".into(),pid:1,app:"Helium".into(),title:String::new(),x:0,y:0,width:100,height:100,focused:true};
        for loss_at in [0,1,2,3] {
            let mut sent=String::new();
            let mut snapshot_attempted=false;
            let result=batch_with(json!({"target":"helium","actions":[{"action":"text","text":"ab"},{"action":"text","text":"cd"}],"screenshot":"display"}),&check,|tool,args,check| {
                assert_eq!(args["target"],"helium");
                if tool=="screenshot" {snapshot_attempted=true; bail!("Target is not foreground");}
                // Same per-character guard as the platform injectors, with a
                // native foreground snapshot changing to Neoism mid-sequence.
                for ch in args["text"].as_str().unwrap().chars() {
                    check.check_probe(|| {
                        let mut current=helium.clone(); current.focused=sent.len()<loss_at;
                        windows::validate_list(&helium,true,vec![current])?; Ok(())
                    })?;
                    sent.push(ch);
                }
                Ok(dispatched("test.native"))
            }).unwrap();
            assert_eq!(sent.len(),loss_at);
            assert!(snapshot_attempted);
            let BuiltinMcpContent::Text {text:progress,..}=&result.content[0] else {panic!()};
            let progress:Value=serde_json::from_str(progress).unwrap();
            assert_eq!(progress["completed"],loss_at/2);
            assert_eq!(progress["failedIndex"],loss_at/2);
            assert_eq!(result.is_error,Some(true));
        }
        let mut reused=helium.clone(); reused.pid=2;
        assert!(windows::validate_list(&helium,true,vec![reused]).is_err());
    }
    #[cfg(target_os="linux")]
    #[test] fn batch_clipboard_effects_survive_success_failure_and_cancellation() {
        let _revocation=TEST_REVOCATION_LOCK.blocking_lock();
        for (fail_at,cancel_after,changed,uncertain,completed) in [
            (None,None,true,false,3),
            (Some(0),None,false,true,0),
            (Some(1),None,true,true,1),
            (Some(2),None,true,false,2),
            (None,Some(0),true,false,1),
        ] {
            let check=Check {progress:Some(Arc::new(AtomicUsize::new(0))),target:None,cancel:Arc::new(AtomicBool::new(false)),dropped:Arc::new(AtomicBool::new(false)),epoch:STOP.load(Ordering::SeqCst),started:Instant::now()};
            let mut calls=0;
            let result=batch_with(json!({"target":"window","actions":[
                {"action":"paste","text":"SECRET_PAYLOAD_A","clipboard_policy":"replace"},
                {"action":"paste","text":"SECRET_PAYLOAD_B","clipboard_policy":"replace"},
                {"action":"key","key":"enter"}
            ]}),&check,|_,_,check| {
                let index=calls;calls+=1;
                if fail_at==Some(index) {bail!("simulated native failure");}
                if cancel_after==Some(index) {check.cancel.store(true,Ordering::SeqCst);}
                Ok(dispatched("test.native"))
            }).unwrap();
            let BuiltinMcpContent::Text{text,..}=&result.content[0] else {panic!()};
            assert!(!text.contains("SECRET_PAYLOAD"));
            let value:Value=serde_json::from_str(text).unwrap();
            assert_eq!(value["status"],if completed==3 {"dispatched_unverified"} else {"partial_unknown"});
            assert_eq!(value["applicationVerified"],false);
            assert_eq!(value["completed"],completed);assert_eq!(value["total"],3);
            assert_eq!(value.get("clipboardChanged"),changed.then_some(&Value::Bool(true)));
            assert_eq!(value.get("clipboardMayHaveChanged"),uncertain.then_some(&Value::Bool(true)));
            assert_eq!(value["clipboardPolicy"],"replace");assert!(value["clipboardWarning"].is_string());
            assert_eq!(check.progress.as_ref().unwrap().load(Ordering::SeqCst),completed);
            assert_eq!(calls,completed+usize::from(fail_at.is_some()));
            if completed<3 {assert_eq!(value["failedIndex"],completed);assert_eq!(result.is_error,Some(true));}
        }
    }
    #[cfg(target_os="linux")]
    #[test] fn cancellation_before_paste_does_not_claim_clipboard_effects() {
        let _revocation=TEST_REVOCATION_LOCK.blocking_lock();
        let check=Check {progress:None,target:None,cancel:Arc::new(AtomicBool::new(true)),dropped:Arc::new(AtomicBool::new(false)),epoch:STOP.load(Ordering::SeqCst),started:Instant::now()};
        let result=batch_with(json!({"target":"window","actions":[{"action":"paste","text":"secret","clipboard_policy":"replace"}]}),&check,|_,_,_|panic!("cancelled paste must not execute")).unwrap();
        let BuiltinMcpContent::Text{text,..}=&result.content[0] else {panic!()};
        let value:Value=serde_json::from_str(text).unwrap();
        assert_eq!(value["completed"],0);assert_eq!(value["status"],"partial_unknown");
        assert!(value.get("clipboardChanged").is_none());assert!(value.get("clipboardMayHaveChanged").is_none());
    }
    #[test] fn timeout_metadata_preserves_clipboard_uncertainty_without_payloads() {
        for (total,completed,changed,uncertain) in [(None,0,false,true),(Some(3),1,true,true),(Some(3),2,true,false),(Some(3),0,false,false)] {
            let result=input_timeout(total,completed,changed,uncertain);
            assert_eq!(result.is_error,Some(true));
            let BuiltinMcpContent::Text{text,..}=&result.content[0] else {panic!()};
            let value:Value=serde_json::from_str(text).unwrap();
            assert_eq!(value["status"],"partial_unknown");assert_eq!(value["applicationVerified"],false);
            assert_eq!(value.get("clipboardChanged"),changed.then_some(&Value::Bool(true)));
            assert_eq!(value.get("clipboardMayHaveChanged"),uncertain.then_some(&Value::Bool(true)));
            assert!(value.get("actions").is_none());assert!(value.get("text").is_none());
            if total.is_some() {assert_eq!(value["completed"],completed);assert_eq!(value["total"],total.unwrap());}
            if changed || uncertain {assert_eq!(value["clipboardPolicy"],"replace");}
        }
    }
    #[test] fn literal_text_rejects_controls_before_any_batch_action() {
        for c in (0u8..=31).chain(127..=159) {
            let value=format!("safe{}tail",char::from(c));
            assert!(validate(&Action::Text{text:value.clone()}).is_err());
            let batch:Batch=serde_json::from_value(json!({"target":"window","actions":[{"action":"key","key":"space"},{"action":"text","text":value}]})).unwrap();
            assert!(validate_batch(&batch).is_err());
        }
        assert!(validate(&Action::Text{text:"e\u{301} \u{200d} العربية".into()}).is_ok());
        #[cfg(target_os="linux")]
        assert!(validate(&Action::Text{text:"🦀".into()}).is_err());
    }
    #[test] fn paste_is_explicit_targeted_and_preflighted() {
        let payload=json!({"action":"paste","text":"🦀\n\tمرحبا","clipboard_policy":"replace"});
        let action:Action=serde_json::from_value(payload.clone()).unwrap();
        #[cfg(target_os="linux")] assert!(validate(&action).is_ok());
        #[cfg(not(target_os="linux"))] assert!(validate(&action).is_err());
        let mut missing=payload.clone();missing.as_object_mut().unwrap().remove("clipboard_policy");
        assert!(serde_json::from_value::<Action>(missing).is_err());
        for patch in [json!({"clipboard_policy":"preserve"}),json!({"text":"a\u{0000}b"}),json!({"text":"a".repeat(513)}),json!({"paste_shortcut":"meta-v"}),json!({"modifiers":["alt"]})] {
            let mut bad=payload.clone();bad.as_object_mut().unwrap().extend(patch.as_object().unwrap().clone());
            let batch:Batch=serde_json::from_value(json!({"target":"window","actions":[{"action":"key","key":"space"},bad]})).unwrap();
            assert!(validate_batch(&batch).is_err());
        }
        let mut no_target=payload.clone();assert!(validate_input_scope("input",&mut no_target).is_err());
        let mut desktop=payload.clone();desktop["scope"]=json!("desktop");assert!(validate_input_scope("input",&mut desktop).is_err());
        let mut targeted=payload;targeted["target"]=json!("window");assert!(validate_input_scope("input",&mut targeted).is_ok());
        assert_eq!(PasteShortcut::ControlV.keys(),vec![Key::Control,Key::Unicode('v')]);
        assert_eq!(PasteShortcut::ControlShiftV.keys(),vec![Key::Control,Key::Shift,Key::Unicode('v')]);
        let batch:Batch=serde_json::from_value(json!({"target":"window","actions":[{"action":"text","text":"a".repeat(300)},{"action":"paste","text":"b".repeat(300),"clipboard_policy":"replace"}]})).unwrap();
        assert!(validate_batch(&batch).is_err());
    }
    #[test] fn native_success_is_dispatch_not_application_verification() {
        let result=dispatched("wayland.wlr_virtual_pointer");
        let BuiltinMcpContent::Text{text,..}=&result.content[0] else {panic!()};
        let value:Value=serde_json::from_str(text).unwrap();
        assert_eq!(value["status"],"dispatched_unverified");
        assert_eq!(value["applicationVerified"],false);
        assert!(value.get("delivered").is_none());
    }
    #[test] fn batches_publish_input_schema_and_new_tools_require_session() {
        let tools=ComputerUse.tools();
        let batch=tools.iter().find(|t|t.name=="batch").unwrap();
        assert_eq!(batch.input_schema["properties"]["actions"]["maxItems"],16);
        assert!(batch.input_schema["properties"]["actions"]["items"]["properties"]["action"].is_object());
        assert_eq!(batch.input_schema["required"],json!(["actions","target"]));
        let items=&batch.input_schema["properties"]["actions"]["items"]["properties"];
        assert!(items.get("target").is_none() && items.get("scope").is_none());
        let input=tools.iter().find(|t|t.name=="input").unwrap();
        assert_eq!(input.input_schema["oneOf"][0]["required"],json!(["target"]));
        assert_eq!(input.input_schema["properties"]["scope"]["enum"],json!(["desktop"]));
        assert!(input.input_schema["properties"]["action"]["enum"].as_array().unwrap().contains(&json!("paste")));
        assert_eq!(input.input_schema["properties"]["clipboard_policy"]["enum"],json!(["replace"]));
        assert_eq!(input.input_schema["allOf"][0]["then"]["required"],json!(["text","clipboard_policy"]));
        assert_eq!(batch.input_schema["properties"]["actions"]["items"]["allOf"],input.input_schema["allOf"]);
        assert!(!input.input_schema["oneOf"][1]["properties"]["action"]["enum"].as_array().unwrap().contains(&json!("paste")));
    }
    #[test] fn arguments_are_strict_and_bounded() {
        assert!(serde_json::from_value::<Action>(json!({"action":"text","text":"ok","command":"bad"})).is_err());
        assert!(validate(&Action::Text { text: "a".repeat(513) }).is_err());
        assert!(validate(&Action::Scroll { amount: i32::MIN, horizontal: false }).is_err());
        assert!(validate(&Action::Key { key:"enter".into(),modifiers:vec!["control".into(),"control".into()] }).is_err());
        assert!(named_key("shell").is_err());
    }
    #[test] fn stale_frames_topology_changes_and_out_of_bounds_are_rejected() {
        let display = Display { id:"screen".into(),x:-1920,y:0,width:1920,height:1080 };
        let displays = vec![display.clone()];
        let mut frame = Frame { target:None, id:"frame".into(),display,width:1600,height:900,created:Instant::now(),epoch:7 };
        assert!(validate_frame(&frame,"frame",0,0,&displays,7).is_ok());
        assert!(validate_frame(&frame,"wrong",0,0,&displays,7).is_err());
        assert!(validate_frame(&frame,"frame",0,0,&displays,8).is_err());
        assert!(validate_frame(&frame,"frame",1600,0,&displays,7).is_err());
        assert!(validate_frame(&frame,"frame",0,0,&[],7).is_err());
        frame.created = Instant::now() - Duration::from_secs(31);
        assert!(validate_frame(&frame,"frame",0,0,&displays,7).is_err());
    }
    #[test] fn direct_capture_and_input_are_denied() {
        for tool in ["screenshot", "input", "batch", "windows", "focus", "wait"] { assert!(ComputerUse.call_tool(Path::new("/"),tool,json!({})).is_err()); }
        assert!(!ComputerUse.enabled_by_default());
    }
    #[tokio::test] async fn revocation_during_native_target_probe_cannot_admit_next_input() {
        let _revocation=TEST_REVOCATION_LOCK.lock().await;
        for revoke in [false,true] {
            let check=Check { progress:None,target:None,cancel:Arc::new(AtomicBool::new(false)),dropped:Arc::new(AtomicBool::new(false)),epoch:STOP.load(Ordering::SeqCst),started:Instant::now() };
            assert!(check.check_probe(|| {
                if revoke { stop(); } else { check.cancel.store(true,Ordering::SeqCst); }
                Ok(())
            }).is_err());
        }
    }
    #[test] fn dropping_call_cancels_worker() {
        let dropped = Arc::new(AtomicBool::new(false));
        let check = Check { progress:None, target:None, cancel:Arc::new(AtomicBool::new(false)), dropped:dropped.clone(),epoch:STOP.load(Ordering::SeqCst),started:Instant::now() };
        assert!(!check.dropped.load(Ordering::SeqCst));
        drop(CancelOnDrop(dropped));
        assert!(check.check().is_err());
    }
}
