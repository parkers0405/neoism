//! Built-in, opt-in desktop control. No shell commands and no separate MCP process.
//! All observation/input goes through the ordinary session MCP permission path.
use std::{path::Path, sync::{Arc, Mutex, atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering}}, time::{Duration, Instant}};
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

#[path = "computer_use/capture.rs"]
mod capture;
#[path = "computer_use/latency.rs"]
mod latency;
#[path = "computer_use/platform.rs"]
mod platform;
#[path = "computer_use/typing.rs"]
mod typing;
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
tokio::task_local! { static CLIPBOARD_AUTHORIZED: bool; }
pub(crate) async fn with_clipboard_authorization<F: std::future::Future>(authorized: bool, future: F) -> F::Output {
    CLIPBOARD_AUTHORIZED.scope(authorized, future).await
}
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
    #[serde(rename="type", alias="text")]
    Text { text: String, #[serde(default)] method: typing::Method, #[serde(default)] clipboard_policy: typing::ClipboardPolicy, #[serde(default)] paste_shortcut: PasteShortcut },
    Paste { text: String, clipboard_policy: String, #[serde(default)] paste_shortcut: PasteShortcut },
    #[serde(alias="press_key")]
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

#[cfg(test)]
fn clipboard_effects(value:&mut Value,changed:bool,uncertain:bool) {
    // Absence is intentional: never turn unknown ownership into a false claim.
    if changed {value["clipboardChanged"]=json!(true);}
    if uncertain {value["clipboardMayHaveChanged"]=json!(true);}
    if changed || uncertain {
        value["clipboardPolicy"]=json!("replace");
        value["clipboardWarning"]=json!("Clipboard was or may have been replaced; history may retain text. Previous content is not read or restored. Observe before any deliberate retry; no automatic retry.");
    }
}
#[cfg(test)]
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
fn attach_timings(result:&mut BuiltinMcpCallResult, timing:latency::Snapshot) {
    let stages=timing.stages.into_iter().map(|(name,(calls,micros))|(name.to_owned(),json!({"calls":calls,"totalUs":micros}))).collect::<serde_json::Map<_,_>>();
    let timing=json!({"queueMs":timing.queue.as_secs_f64()*1000.0,"workerMs":timing.worker.as_secs_f64()*1000.0,"totalMs":(timing.queue+timing.worker).as_secs_f64()*1000.0,"stages":stages,"counters":timing.counters,"note":"Native authorized-call timing; excludes model/permission wait and response transport. Stages overlap; no application-consumption acknowledgement."});
    for content in &mut result.content {
        if let BuiltinMcpContent::Text {text,..}=content {
            if let Ok(Value::Object(mut value))=serde_json::from_str(text) {
                value.insert("timings".into(),timing);
                *text=Value::Object(value).to_string();
                break;
            }
        }
    }
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
            ("screenshot", "Capture one display as PNG with a short-lived frame token. Images may contain private information. Pass image-pixel coordinates and that token to move/click.", json!({"type":"object","properties":{"settle_ms":{"type":"integer","minimum":0,"maximum":3000,"default":0,"description":"Immediate capture by default (0). Positive values opt into bounded visual settling, not application readiness."},"display":{"type":"string"},"target":{"type":"string","description":"Optional foreground window token; binds image to this target for pointer input"}},"required":["display"],"additionalProperties":false})),
            ("input", "Control the HOST desktop (not a remote browser). One bounded action; no persistent key/button holds. Use a fresh screenshot frame for move/click/drag; drag uses to_x/to_y for its endpoint. Text is literal, max 512 Unicode characters, with no C0/C1 controls (including newline, CR, Tab); use explicit key actions for Return/Tab. Key names: enter, tab, escape, backspace, delete, space, up/down/left/right, home/end, page_up/page_down, or one ASCII letter/digit (case-insensitive; use shift modifier). Linux also accepts a single ASCII punctuation key present in the unmodified layout, such as / or -. For shifted punctuation, use the unshifted key with shift (for example shift+- for underscore on US layouts). Use text for other supported literal characters. Modifiers (case-insensitive): control/ctrl/Control_L, alt/option/Alt_L, shift/Shift_L, meta/super/Super_L/win/windows/logo/cmd/command. Modifier names can also be tapped as keys. Key aliases include Return, Esc, ArrowLeft/Right/Up/Down, PageUp/PgUp and PageDown/PgDn. Scroll amount -20..20; positive is down/right. Cancellation can leave partial text.", json!({"type":"object","properties":{"target":{"type":"string","description":"Optional windows token; require unchanged foreground window. Not a sandbox."},"action":{"type":"string","enum":["move","click","drag","scroll","text","key"]},"frame":{"type":"string"},"x":{"type":"integer","minimum":0},"y":{"type":"integer","minimum":0},"to_x":{"type":"integer","minimum":0},"to_y":{"type":"integer","minimum":0},"button":{"type":"string","enum":["left","right","middle"]},"amount":{"type":"integer","minimum":-20,"maximum":20},"horizontal":{"type":"boolean"},"text":{"type":"string","maxLength":512,"description":"Literal Unicode only; no C0/C1 control characters including newline, CR or Tab. Use explicit key actions for Return/Tab."},"key":{"type":"string","description":"Case-insensitive named key or ASCII letter/digit; Linux also accepts unmodified ASCII punctuation. For underscore on US layouts use key '-' with shift. Super_L/super/meta tap the system modifier; Return=enter, Esc=escape. Use action:text only where literal text is supported."},"modifiers":{"type":"array","items":{"type":"string","description":"Case-insensitive: control/ctrl/Control_L, alt/option/Alt_L, shift/Shift_L, meta/super/Super_L/win/windows/logo/cmd/command."},"maxItems":4}},"required":["action"],"additionalProperties":false})),
            ("windows", "List native windows and snapshot target tokens. Re-list invalidates old tokens. Bounds are not screenshot pixel coordinates. Foreground targeting is not isolation.", json!({"type":"object","properties":{},"additionalProperties":false})),
            ("focus", "Request focus of one listed target window; confirm foreground or fail. Invalidates screenshot coordinates. Does not guarantee exclusive input routing.", json!({"type":"object","properties":{"target":{"type":"string"}},"required":["target"],"additionalProperties":false})),
            ("batch", "Run 1..16 already-decided input actions serially, stopping on first failure/cancellation. Optional final display screenshot is returned as model image with frame token. Do not batch blind semantic decisions: observe between decisions. Total text <=512 characters. Failed action may be partially delivered.", json!({"type":"object","properties":{"settle_ms":{"type":"integer","minimum":0,"maximum":3000,"default":0,"description":"Final screenshot defaults to immediate capture (0); positive values opt into visual settling, not application readiness."},"actions":{"type":"array","minItems":1,"maxItems":16,"items":{"type":"object","description":"Same action object as input; no nested batches"}},"target":{"type":"string"},"screenshot":{"type":"string","description":"Display ID for final screenshot"}},"required":["actions"],"additionalProperties":false})),
            ("wait", "Bounded observation of one listed window becoming foreground or closing. No typing, clicks, browser semantics, or implicit retries of actions. Polls at 100ms; native calls can overrun.", json!({"type":"object","properties":{"target":{"type":"string"},"condition":{"type":"string","enum":["foreground","closed"]},"timeout_ms":{"type":"integer","minimum":100,"maximum":3000}},"required":["target","condition","timeout_ms"],"additionalProperties":false})),
            ("stop", "Cancel current computer action and invalidate screenshot coordinates. Does not wait for the desktop lock.", json!({"type":"object","properties":{},"additionalProperties":false})),
        ].into_iter().map(|(name, description, input_schema)| BuiltinMcpTool {
            name: name.into(), description: Some(description.into()), input_schema,
            annotations: Some(json!({"readOnlyHint": name == "capabilities" || name == "screenshot" || name == "windows" || name == "wait", "destructiveHint": name == "input" || name == "batch" || name == "focus", "openWorldHint": true})),
        }).collect();
        let input=tools.iter_mut().find(|t|t.name=="input").unwrap();
        input.description.as_mut().unwrap().push_str(" Canonical action:type (text alias) chooses ONE whole-string method before any input. method:auto|keyboard|native|paste defaults auto; clipboard_policy:forbid|replace defaults forbid. Linux uses the existing keyboard layout or, only with replace consent, clipboard paste. No remapping or runtime fallback. auto/keyboard/native reject C0/C1 including LF/Tab; forced method:paste deliberately permits LF/Tab. Explicit legacy paste permits LF/Tab deliberately, requires replace, and may execute commands. Paste is Linux-only. Native dispatch is not application verification.");
        input.input_schema["properties"]["action"]["enum"]=json!(["move","click","drag","scroll","type","text","key","press_key","paste"]);
        input.input_schema["properties"]["method"]=json!({"type":"string","enum":["auto","keyboard","native","paste"],"default":"auto"});
        input.input_schema["properties"]["clipboard_policy"]=json!({"type":"string","enum":["forbid","replace"],"default":"forbid"});
        input.input_schema["properties"]["paste_shortcut"]=json!({"type":"string","enum":["control-v","control-shift-v"],"default":"control-v"});
        input.input_schema["properties"]["text"]["description"]=json!("Max 512 characters total. auto/keyboard/native reject all C0/C1 controls; forced paste and legacy paste permit LF/Tab and require replace.");
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
                        {"required":["scope"],"not":{"required":["target"]},"properties":{"action":{"enum":["key","press_key","move","click","drag","scroll"]}}}
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
            let effects=Arc::new(Mutex::new(input_pipeline::Effects::default()));
            let worker_effects=effects.clone();
            let clipboard_authorized=CLIPBOARD_AUTHORIZED.try_with(|v|*v).unwrap_or(false);
            let input_requested=tool=="input";
            let tool = tool.to_owned();
            #[cfg(test)]
            let test_backend = TEST_BACKEND.try_with(Arc::clone).ok();
            let queued=Instant::now();
            let result = tokio::time::timeout(DEADLINE, tokio::task::spawn_blocking(move || {
                let timing=latency::Scope::start(queued.elapsed());
                let result=run_worker(epoch, cancel, dropped, |check| {
                    let check=Check { progress:Some(worker_progress), effects:worker_effects, clipboard_authorized, ..check.clone() };
                    let check=&check;
                    preflight_request(&tool,&arguments,check)?;
                    #[cfg(test)]
                    if let Some(backend) = test_backend { return backend(&tool, arguments); }
                    execute(&tool, arguments, check)
                });
                (result,timing.finish())
            })).await;
            drop(guard);
            match result {
                Ok(Ok((result,timing))) => {
                    let mut result=match result {
                        Ok(result)=>result,
                        Err(error) if input_requested || batch_total.is_some()=>effects.lock().unwrap_or_else(std::sync::PoisonError::into_inner).report(batch_total,Some(format!("{error:#}"))),
                        Err(error)=>return Err(ServiceError::new(format!("{error:#}"))),
                    };
                    attach_timings(&mut result,timing);
                    Ok(result)
                },
                Ok(Err(e)) if input_requested || batch_total.is_some() => Ok(effects.lock().unwrap_or_else(std::sync::PoisonError::into_inner).report(batch_total,Some(format!("Computer worker failed: {e}")))),
                Ok(Err(e)) => Err(ServiceError::new(format!("Computer worker failed: {e}"))),
                Err(_) if input_requested || batch_total.is_some() => Ok(effects.lock().unwrap_or_else(std::sync::PoisonError::into_inner).report(batch_total,Some("Computer input timed out; further input cancelled. Observe before deliberate retry; no automatic retry.".into()))),
                Err(_) => Err(ServiceError::new("Computer operation timed out; further input cancelled. Native calls cannot be preempted.")),
            }
        })
    }
}
struct CancelOnDrop(Arc<AtomicBool>);
impl Drop for CancelOnDrop { fn drop(&mut self) { self.0.store(true, Ordering::SeqCst); } }
#[derive(Clone)]
struct Check { progress:Option<Arc<AtomicUsize>>, effects: Arc<Mutex<input_pipeline::Effects>>, clipboard_authorized:bool, target: Option<windows::Window>, cancel: Arc<AtomicBool>, dropped: Arc<AtomicBool>, epoch: u64, started: Instant }
impl Check {
    fn fast(&self) -> anyhow::Result<()> {
        latency::count("cancellation_checks",1);
        ensure!(!self.cancel.load(Ordering::SeqCst) && !self.dropped.load(Ordering::SeqCst) && self.epoch == STOP.load(Ordering::SeqCst) && self.started.elapsed() < DEADLINE, "Computer operation stopped; partial input may have been delivered");
        Ok(())
    }
    fn check(&self) -> anyhow::Result<()> {
        self.check_probe(|| latency::measure("target_validation",|| {
            if let Some(target)=&self.target { windows::validate(target,true)?; }
            Ok(())
        }))
    }
    fn check_probe(&self, probe:impl FnOnce()->anyhow::Result<()>) -> anyhow::Result<()> {
        self.fast()?;
        probe()?;
        // A blocking native probe cannot admit an event after revocation.
        self.fast()
    }
}
fn run_worker<T>(epoch: u64, cancel: Arc<AtomicBool>, dropped: Arc<AtomicBool>, backend: impl FnOnce(&Check) -> anyhow::Result<T>) -> anyhow::Result<T> {
    // Never reload STOP here: an admission can have been revoked before this
    // blocking worker was scheduled. Tests use this same worker entry point.
    let check = Check { progress:None, effects:Arc::default(),clipboard_authorized:false, target:None, epoch, cancel, dropped, started: Instant::now() };
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
// Shared request validation lies above the native test seam and below admission.
fn preflight_request(tool:&str,arguments:&Value,check:&Check)->anyhow::Result<()> {
    if tool=="input" || tool=="batch" {
        let count=if tool=="batch" {arguments["actions"].as_array().map_or(0,Vec::len).min(16)} else {1};
        check.effects.lock().unwrap_or_else(std::sync::PoisonError::into_inner).records=(0..count).map(input_pipeline::unplanned).collect();
    }
    match tool {
        "input"=>{
            let mut value=arguments.clone();validate_input_scope(tool,&mut value)?;
            value.as_object_mut().unwrap().remove("target");
            let action:Action=serde_json::from_value(value)?;validate(&action)?;
        },
        "batch"=>{
            let batch=serde_json::from_value::<Batch>(arguments.clone())?;
            if let Err(error)=validate_batch(&batch) {
                let failed=batch.actions.iter().position(|v|serde_json::from_value::<Action>(v.clone()).map_err(anyhow::Error::from).and_then(|a|validate(&a)).is_err());
                check.effects.lock().unwrap_or_else(std::sync::PoisonError::into_inner).current=failed;
                return Err(error);
            }
        },
        _=>return Ok(()),
    }
    fn clipboard(value:&Value)->bool {value["action"]=="paste" || value["method"]=="paste" || value["clipboard_policy"]=="replace" || value["actions"].as_array().is_some_and(|a|a.iter().any(clipboard))}
    ensure!(!clipboard(arguments) || check.clipboard_authorized,"Clipboard replacement requires explicit human computer_clipboard permission");
    Ok(())
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
    let scoped = Check { progress:check.progress.clone(), effects:check.effects.clone(), clipboard_authorized:check.clipboard_authorized, cancel:check.cancel.clone(), dropped:check.dropped.clone(),epoch:check.epoch,started:check.started,target };
    let check = if scoped.target.is_some() { &scoped } else { check };
    check.check()?;
    match tool {
        "screenshot" => {
            #[derive(Deserialize)] #[serde(deny_unknown_fields)] struct Args { display: String, #[serde(default="default_settle_ms")] settle_ms:u64 }
            let args: Args = serde_json::from_value(arguments)?;
            #[cfg(target_os="linux")]
            let mut capture_session=latency::measure("capture_setup",capture::ScopedLinuxCaptureSession::new)?;
            #[cfg(target_os="linux")]
            let displays=capture_session.displays();
            #[cfg(not(target_os="linux"))]
            let displays=latency::measure("display_probe",platform::displays)?;
            let display = displays.into_iter().find(|d| d.id == args.display).context("Unknown display; call capabilities again")?;
            ensure!(u64::from(display.width) * u64::from(display.height) <= 64_000_000, "Display exceeds capture limit");
            ensure!(args.settle_ms<=3000,"settle_ms must be 0..3000");
            #[cfg(target_os="linux")]
            let capture_image=||latency::measure("capture",||capture_session.capture(&display));
            #[cfg(not(target_os="linux"))]
            let capture_image=||latency::measure("capture",||platform::capture(&display));
            let (image,settling)=capture_settled_with_checks(args.settle_ms,||check.check(),||check.fast(),capture_image)?;
            #[cfg(not(target_os="linux"))]
            ensure!(platform::displays()?.contains(&display),"Display topology changed during capture; observe again");
            check.check()?;
            let encoded=capture::encode(image)?;
            latency::record("resize",encoded.timings.resize);
            latency::record("png",encoded.timings.png);
            latency::count("png_lossless_retries",u64::from(encoded.timings.lossless_retry));
            latency::count("png_bytes",encoded.bytes.len() as u64);
            let current_target=check.target.as_ref().map(|target|windows::validate(target,true)).transpose()?;
            let frame = Frame { target:current_target, id: format!("{:032x}", rand::random::<u128>()), display, width: encoded.width, height: encoded.height, created: Instant::now(), epoch: check.epoch };
            check.check()?;
            let mut result = text(json!({"frame":frame.id,"display":frame.display,"imageWidth":frame.width,"imageHeight":frame.height,"coordinates":"image pixels, origin top-left; pass frame unchanged","expiresAfterSeconds":30,"targetWindow":frame.target,"settling":settling}));
            *FRAME.lock().map_err(|_| anyhow::anyhow!("Frame state poisoned"))? = Some(frame);
            let data=latency::measure("base64",||STANDARD.encode(encoded.bytes));
            result.content.push(BuiltinMcpContent::Image { data, mime_type: "image/png".into(), annotations: None });
            Ok(result)
        }
        "input" => {
            let action: Action = serde_json::from_value(arguments)?;
            let displays=if needs_pointer(&action) {latency::measure("display_probe",platform::displays)?} else {Vec::new()};
            check.check()?;
            input_pipeline::run(vec![action], &displays, check, false)
        }
        _ => bail!("Unknown computer tool {tool}"),
    }
}
fn needs_pointer(action:&Action)->bool {
    matches!(action,Action::Move{..}|Action::Click{..}|Action::Drag{..}|Action::Scroll{..})
}
fn pointer_action(action: Action, displays: &[Display], check: &Check) -> anyhow::Result<BuiltinMcpCallResult> {
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
                Action::Paste {..}|Action::Text {..}|Action::Key {..}=>unreachable!("Typing and shortcuts execute only through prepared plans"),
            }
            check.check()?;
            Ok(dispatched(platform::NAME))
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
        ensure!(matches!(object.get("action").and_then(Value::as_str),Some("key"|"press_key"|"move"|"click"|"drag"|"scroll")),"Desktop scope forbids text and paste; list/focus a window and pass its target");
    } else {
        ensure!(target.and_then(Value::as_str).is_some_and(|t|!t.is_empty()),"Input requires an explicit window target even after focus. Foreground is not inherited. For deliberate global shortcuts/pointer actions only, use scope:desktop");
    }
    Ok(())
}

fn default_settle_ms()->u64 { 0 }
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
#[cfg(test)]
fn capture_settled(ms:u64,check:impl FnMut()->anyhow::Result<()>,capture:impl FnMut()->anyhow::Result<image_rs::DynamicImage>)->anyhow::Result<(image_rs::DynamicImage,Value)> {
    let check=std::cell::RefCell::new(check);
    capture_settled_with_checks(ms,||(*check.borrow_mut())(),||(*check.borrow_mut())(),capture)
}
fn capture_settled_with_checks(ms:u64,mut check:impl FnMut()->anyhow::Result<()>,mut waiting:impl FnMut()->anyhow::Result<()>,mut capture:impl FnMut()->anyhow::Result<image_rs::DynamicImage>)->anyhow::Result<(image_rs::DynamicImage,Value)> {
    ensure!(ms<=3000,"settle_ms must be 0..3000");
    let start=Instant::now();
    if ms==0 {
        check()?;
        let image=capture()?;
        check()?;
        return Ok((image,json!({"mode":"immediate","settled":false,"timedOut":false,"elapsedMs":start.elapsed().as_millis(),"captures":1,"samples":0,"note":"Immediate capture; no settling wait or application-readiness acknowledgement."})));
    }
    let mut state=Settling::default();
    let mut captures=0;
    loop {
        check()?;
        let image=capture()?;
        check()?;
        captures+=1;
        let sample=latency::measure("settle_sample",||image.resize_exact(64,64,image_rs::imageops::FilterType::Triangle).into_rgb8().into_raw());
        let elapsed=start.elapsed();
        let stable=state.observe(elapsed,sample);
        if stable || elapsed>=Duration::from_millis(ms) {
            return Ok((image,json!({"settled":ms!=0 && stable,"timedOut":ms!=0 && !stable,"elapsedMs":elapsed.as_millis(),"captures":captures,"note":"Visual stability only; not application readiness"})));
        }
        // Check cancellation at <=25ms while waiting between 100ms probes.
        let until=(start.elapsed()+Duration::from_millis(100)).min(Duration::from_millis(ms));
        while start.elapsed()<until { waiting()?; std::thread::sleep(Duration::from_millis(25).min(until.saturating_sub(start.elapsed()))); }
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
        if let Action::Text { text, .. }|Action::Paste {text,..}=action { chars+=text.chars().count(); }
    }
    ensure!(chars<=MAX_TEXT,"Batch text total exceeds 512 characters");
    Ok(())
}
// Generic loop keeps cancellation/partial-progress tests off the real desktop.
#[cfg(test)]
fn sequence(count:usize, mut step:impl FnMut(usize)->anyhow::Result<()>)->(usize,Option<String>) {
    for index in 0..count {
        if let Err(error)=step(index) { return (index,Some(format!("{error:#}"))); }
    }
    (count,None)
}
fn batch(arguments:Value,check:&Check)->anyhow::Result<BuiltinMcpCallResult> {
    let args:Batch=serde_json::from_value(arguments)?;
    validate_batch(&args)?;
    let target=windows::resolve(args.target.as_deref().unwrap())?;
    let scoped=Check {target:Some(target), ..check.clone()};
    scoped.check()?;
    let actions=args.actions.into_iter().map(serde_json::from_value).collect::<Result<Vec<Action>,_>>()?;
    let displays=if actions.iter().any(needs_pointer) {latency::measure("display_probe",platform::displays)?} else {Vec::new()};
    after_input_capture(||input_pipeline::run(actions,&displays,&scoped,true),|| {
        args.screenshot.map(|display|scoped.check().and_then(|_|execute_locked("screenshot",json!({"display":display,"target":args.target,"settle_ms":args.settle_ms}),&scoped)))
    })
}
fn after_input_capture(input:impl FnOnce()->anyhow::Result<BuiltinMcpCallResult>,capture:impl FnOnce()->Option<anyhow::Result<BuiltinMcpCallResult>>)->anyhow::Result<BuiltinMcpCallResult> {
    // The input pipeline includes explicit keyboard finalization, on success
    // and failure. Observation must never race ahead of that cleanup.
    let mut result=input()?;
    if let Some(image)=capture() {
        match image {
            Ok(image)=>result.content.extend(image.content),
            Err(error)=>{result.content.extend(text(json!({"screenshotError":format!("{error:#}")})).content);result.is_error=Some(true);}
        }
    }
    Ok(result)
}
#[cfg(test)]
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
        Action::Text { text, method, clipboard_policy, .. } => {
            #[cfg(target_os="linux")]
            ensure!(*method!=typing::Method::Native,"Native Unicode injection is unsupported on Linux");
            #[cfg(not(target_os="linux"))]
            ensure!(*method!=typing::Method::Keyboard,"Forced layout keyboard typing is unsupported on this platform");
            if *method==typing::Method::Paste {
                ensure!(*clipboard_policy==typing::ClipboardPolicy::Replace,"Paste requires explicit clipboard_policy:replace");
                ensure!(text.chars().count()<=MAX_TEXT && !text.contains('\0'),"Paste must contain at most 512 Unicode characters and no NUL");
                #[cfg(not(target_os="linux"))] bail!("Clipboard paste is unsupported on this platform");
                return Ok(());
            }
            ensure!(text.chars().count() <= MAX_TEXT && !text.chars().any(char::is_control), "Text must contain at most 512 characters and no C0/C1 control characters; use explicit key actions for Return/Tab");

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
        #[cfg(target_os = "linux")]
        s if s.len() == 1 && s.as_bytes()[0].is_ascii_punctuation() => Key::Unicode(s.chars().next().unwrap()),
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
    fn input_finalization_precedes_any_result_capture() {
        let finished=std::cell::Cell::new(false);
        let result=after_input_capture(||{
            finished.set(true);
            let mut result=text(json!({"completed":1,"cleanupFailure":"test failure"}));
            result.is_error=Some(true);Ok(result)
        },||{
            assert!(finished.get(),"capture ran before finalized input outcome");
            Some(Ok(text(json!({"frame":"test"}))))
        }).unwrap();
        assert_eq!(result.is_error,Some(true));assert_eq!(result.content.len(),2);
        assert!(after_input_capture(||bail!("cancelled before execution"),||panic!("capture after failed admission")).is_err());
    }
    #[test]
    fn immediate_capture_skips_sampling_and_waits() {
        let scope=latency::Scope::start(Duration::ZERO);
        let checks=std::cell::Cell::new(0);
        let captures=std::cell::Cell::new(0);
        assert_eq!(default_settle_ms(),0);
        let (_,info)=capture_settled_with_checks(0,||{checks.set(checks.get()+1);Ok(())},
            ||panic!("immediate capture must not wait"),||{captures.set(captures.get()+1);Ok(image_rs::DynamicImage::new_rgb8(2,3))}).unwrap();
        assert_eq!(captures.get(),1);assert_eq!(checks.get(),2);
        assert_eq!(info["mode"],"immediate");assert_eq!(info["samples"],0);assert_eq!(info["timedOut"],false);
        assert!(!scope.finish().stages.contains_key("settle_sample"));
    }
    #[test]
    fn keyboard_only_actions_do_not_need_capture_topology() {
        for value in [json!({"action":"type","text":"hello"}),json!({"action":"key","key":"a"}),json!({"action":"paste","text":"hello","clipboard_policy":"replace"})] {
            assert!(!needs_pointer(&serde_json::from_value(value).unwrap()));
        }
        assert!(needs_pointer(&Action::Scroll{amount:1,horizontal:false}));
    }
    #[test]
    fn native_timing_metadata_does_not_change_dispatch_status() {
        let scope=latency::Scope::start(Duration::from_millis(2));
        latency::count("window_ipc_requests",1);
        let mut result=dispatched("keyboard");
        attach_timings(&mut result,scope.finish());
        let BuiltinMcpContent::Text{text,..}=&result.content[0] else {panic!("JSON result")};
        let value:Value=serde_json::from_str(text).unwrap();
        assert_eq!(value["status"],"dispatched_unverified");
        assert_eq!(value["applicationVerified"],false);
        assert_eq!(value["timings"]["counters"]["window_ipc_requests"],1);
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_punctuation_keys_use_the_original_layout() {
        for ch in ['/', '-', '=', '[', ']', '\\', ';', '\'', ',', '.', '`'] {
            assert_eq!(named_key(&ch.to_string()).unwrap(), Key::Unicode(ch));
            validate(&Action::Key {key:ch.to_string(),modifiers:Vec::new()}).unwrap();
        }
        let context=xkbcommon::xkb::Context::new(xkbcommon::xkb::CONTEXT_NO_FLAGS);
        let map=xkbcommon::xkb::Keymap::new_from_names(&context,"","","us","",None,xkbcommon::xkb::KEYMAP_COMPILE_NO_FLAGS).unwrap();
        let slash=shortcuts::resolve_in_map(&map,0,&[named_key("/").unwrap()]).unwrap();
        assert_eq!(slash,vec![61]);
        let codes=shortcuts::resolve_in_map(&map,0,&[Key::Shift,named_key("-").unwrap()]).unwrap();
        let mut state=xkbcommon::xkb::State::new(&map);
        for code in &codes {state.update_key(xkbcommon::xkb::Keycode::new(u32::from(*code)),xkbcommon::xkb::KeyDirection::Down);}
        assert_eq!(state.key_get_utf8(xkbcommon::xkb::Keycode::new(u32::from(codes[1]))),"_");
        assert!(shortcuts::resolve_in_map(&map,0,&[named_key("_").unwrap()]).is_err());
        assert!(named_key("é").is_err());
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
            let failed=ComputerUse.call_tool_authorized_async(Path::new("/"),"input",json!({"action":"text","text":"mock","target":"fixture"}),true,Arc::new(AtomicBool::new(false)),Some(epoch)).await;
            assert_eq!(failed.unwrap().is_error,Some(true));
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
            let check=Check { progress:None,effects:Arc::default(),clipboard_authorized:false,target:None,cancel:Arc::new(AtomicBool::new(false)),dropped:Arc::new(AtomicBool::new(false)),epoch:STOP.load(Ordering::SeqCst),started:Instant::now() };
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
            let check=Check { progress:Some(Arc::new(AtomicUsize::new(0))),effects:Arc::default(),clipboard_authorized:false,target:None,cancel:Arc::new(AtomicBool::new(false)),dropped:Arc::new(AtomicBool::new(false)),epoch:STOP.load(Ordering::SeqCst),started:Instant::now() };
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
        let check=Check { progress:None,effects:Arc::default(),clipboard_authorized:false,target:None,cancel:Arc::new(AtomicBool::new(false)),dropped:Arc::new(AtomicBool::new(false)),epoch:STOP.load(Ordering::SeqCst),started:Instant::now() };
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
            let check=Check {progress:Some(Arc::new(AtomicUsize::new(0))),effects:Arc::default(),clipboard_authorized:false,target:None,cancel:Arc::new(AtomicBool::new(false)),dropped:Arc::new(AtomicBool::new(false)),epoch:STOP.load(Ordering::SeqCst),started:Instant::now()};
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
        let check=Check {progress:None,effects:Arc::default(),clipboard_authorized:false,target:None,cancel:Arc::new(AtomicBool::new(true)),dropped:Arc::new(AtomicBool::new(false)),epoch:STOP.load(Ordering::SeqCst),started:Instant::now()};
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
            assert!(validate(&Action::Text{text:value.clone(),method:Default::default(),clipboard_policy:Default::default(),paste_shortcut:Default::default()}).is_err());
            let batch:Batch=serde_json::from_value(json!({"target":"window","actions":[{"action":"key","key":"space"},{"action":"text","text":value}]})).unwrap();
            assert!(validate_batch(&batch).is_err());
        }
        assert!(validate(&Action::Text{text:"e\u{301} \u{200d} العربية".into(),method:Default::default(),clipboard_policy:Default::default(),paste_shortcut:Default::default()}).is_ok());
        #[cfg(target_os="linux")]
        assert!(validate(&Action::Text{text:"🦀".into(),method:Default::default(),clipboard_policy:Default::default(),paste_shortcut:Default::default()}).is_ok());
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
        assert_eq!(input.input_schema["properties"]["clipboard_policy"]["enum"],json!(["forbid","replace"]));
        assert_eq!(input.input_schema["allOf"][0]["then"]["required"],json!(["text","clipboard_policy"]));
        assert_eq!(batch.input_schema["properties"]["actions"]["items"]["allOf"],input.input_schema["allOf"]);
        assert!(!input.input_schema["oneOf"][1]["properties"]["action"]["enum"].as_array().unwrap().contains(&json!("paste")));
    }
    #[test] fn arguments_are_strict_and_bounded() {
        assert!(serde_json::from_value::<Action>(json!({"action":"text","text":"ok","command":"bad"})).is_err());
        assert!(validate(&Action::Text { text: "a".repeat(513),method:Default::default(),clipboard_policy:Default::default(),paste_shortcut:Default::default() }).is_err());
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
            let check=Check { progress:None,effects:Arc::default(),clipboard_authorized:false,target:None,cancel:Arc::new(AtomicBool::new(false)),dropped:Arc::new(AtomicBool::new(false)),epoch:STOP.load(Ordering::SeqCst),started:Instant::now() };
            assert!(check.check_probe(|| {
                if revoke { stop(); } else { check.cancel.store(true,Ordering::SeqCst); }
                Ok(())
            }).is_err());
        }
    }
    #[test] fn dropping_call_cancels_worker() {
        let dropped = Arc::new(AtomicBool::new(false));
        let check = Check { progress:None, effects:Arc::default(),clipboard_authorized:false, target:None, cancel:Arc::new(AtomicBool::new(false)), dropped:dropped.clone(),epoch:STOP.load(Ordering::SeqCst),started:Instant::now() };
        assert!(!check.dropped.load(Ordering::SeqCst));
        drop(CancelOnDrop(dropped));
        assert!(check.check().is_err());
    }
}

mod input_pipeline {
use super::*;
use super::typing::{Method,ClipboardPolicy};
#[cfg(not(target_os="linux"))]
use super::typing::select;
#[derive(Default)]
pub(super) struct Effects {
    pub records: Vec<Value>,
    pub completed: usize,
    pub current: Option<usize>,
    pub started: bool,
    pub cleanup_phase: bool,
    pub cleanup_failure: Option<String>,
}
pub(super) fn unplanned(index:usize)->Value {json!({"index":index,"method":null,"phase":"preflight","status":"not_dispatched","completedNativeUnits":0,"currentUnitUncertain":false,"clipboard":"unchanged","cleanupFailure":null,"applicationVerified":false})}
impl Effects {
    pub fn report(&self, total: Option<usize>, error: Option<String>) -> BuiltinMcpCallResult {
        let possible=self.records.iter().any(|r|r["currentUnitUncertain"]==true || r["completedNativeUnits"].as_u64().unwrap_or(0)>0 || matches!(r["clipboard"].as_str(),Some("changed"|"may_changed")));
        let mut records=self.records.clone();
        if error.is_some() {if let Some(record)=records.get_mut(self.current.unwrap_or(self.completed)) {
            record["status"]=json!(if record["currentUnitUncertain"]==true || record["completedNativeUnits"].as_u64().unwrap_or(0)>0 || matches!(record["clipboard"].as_str(),Some("changed"|"may_changed")) {"partial_unknown"} else {"failed"});
        }}
        let mut value=json!({"status":if error.is_some() {if possible {"partial_unknown"} else {"failed"}} else {"dispatched_unverified"},
            "applicationVerified":false,"actions":records,"error":error,"partialActionPossible":error.is_some() && possible,
            "phase":if self.started {"execution"} else {"preflight"},
            "completedNativeUnitsMeaning":"dispatched, not inserted; no application acknowledgement",
            "clipboard":"unchanged","clipboardChanged":false,"clipboardMayHaveChanged":false});
        if self.records.iter().any(|r|r["clipboard"]=="changed") {value["clipboard"]=json!("changed");value["clipboardChanged"]=json!(true);}
        if self.records.iter().any(|r|r["clipboard"]=="may_changed") {value["clipboardMayHaveChanged"]=json!(true); if value["clipboard"]!="changed" {value["clipboard"]=json!("may_changed");}}
        if let Some(total)=total {value["completed"]=json!(self.completed);value["total"]=json!(total);value["failedIndex"]=json!(if error.is_some(){self.current.filter(|i|*i<total).or((self.completed<total).then_some(self.completed))}else{None});}
        if total.is_none() {if let Some(record)=self.records.first() {for field in ["method","phase","completedNativeUnits","currentUnitUncertain","cleanupFailure","totalNativeUnits","nativeUnitKind"] {value[field]=record[field].clone();}}}
        if self.cleanup_phase {value["phase"]=json!("cleanup");value["cleanupPending"]=json!(self.cleanup_failure.is_none());}
        if let Some(error)=&self.cleanup_failure {value["cleanupFailure"]=json!(error);}
        let mut result=text(value); result.is_error=error.map(|_|true); result
    }
}
fn update(check:&Check, index:usize, f:impl FnOnce(&mut Value)) {
    let mut effects=check.effects.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    f(&mut effects.records[index]);
}

enum Prepared {
    #[cfg(target_os="linux")]
    Keyboard(linux_text::KeyPlan),
    #[cfg(target_os="linux")]
    Text(typing::PreparedText),
    #[cfg(not(target_os="linux"))]
    Native(platform::NativePlan),
    #[cfg(not(target_os="linux"))]
    Keys(Vec<shortcuts::NativeKey>),
    Pointer(Action),
}
impl Prepared {
    fn method(&self)->&'static str {match self {
        #[cfg(target_os="linux")] Self::Keyboard(_)=>"keyboard",
        #[cfg(target_os="linux")] Self::Text(plan)=>plan.method(),
        #[cfg(not(target_os="linux"))] Self::Native(_)=>"native",
        #[cfg(not(target_os="linux"))] Self::Keys(_)=>"keyboard",
        Self::Pointer(_)=>"pointer",
    }}
}

pub(super) fn run(actions:Vec<Action>,displays:&[Display],check:&Check,batch:bool)->anyhow::Result<BuiltinMcpCallResult> {
    let total=actions.len();
    check.effects.lock().unwrap_or_else(std::sync::PoisonError::into_inner).records=(0..total).map(unplanned).collect();
    let result=native_boundary(|| {
        for action in &actions {validate(action)?;}
        check.check()?;
        #[cfg(target_os="linux")]
        let mut keyboard=if actions.iter().any(|a|matches!(a,Action::Text{..}|Action::Key{..}|Action::Paste{..})) {
            Some(linux_text::KeyboardSession::open(&mut ||check.fast())?)
        } else {None};
        #[cfg(not(target_os="linux"))]
        let mut input=Enigo::new(&Settings {open_prompt_to_get_permissions:false,release_keys_when_dropped:false,..Settings::default()})?;
        let outcome=native_boundary(|| {
        let prepared=latency::measure("planning",||->anyhow::Result<Vec<Prepared>> {
        let mut prepared=Vec::with_capacity(total);
        for (index,action) in actions.into_iter().enumerate() {
            check.effects.lock().unwrap_or_else(std::sync::PoisonError::into_inner).current=Some(index);
            check.check()?;
            let plan=match action {
                Action::Text {text,method,clipboard_policy,paste_shortcut}=> {
                    #[cfg(target_os="linux")]
                    {
                        Prepared::Text(typing::prepare_text(keyboard.as_ref().unwrap(),&text,method,clipboard_policy,&paste_shortcut.keys(),false,check.clipboard_authorized.then(typing::ClipboardPermit::granted),&mut ||check.fast())?)
                    }
                    #[cfg(not(target_os="linux"))]
                    {let _=paste_shortcut;select(method,clipboard_policy,false,false)?;Prepared::Native(platform::prepare_native(&text)?)}
                },
                Action::Paste{text,paste_shortcut,..}=> {
                    #[cfg(target_os="linux")]
                    {Prepared::Text(typing::prepare_text(keyboard.as_ref().unwrap(),&text,Method::Paste,ClipboardPolicy::Replace,&paste_shortcut.keys(),true,check.clipboard_authorized.then(typing::ClipboardPermit::granted),&mut ||check.fast())?)}
                    #[cfg(not(target_os="linux"))]
                    {let _=(text,paste_shortcut);bail!("Clipboard paste is unsupported on this platform")}
                },
                Action::Key{key,modifiers}=>{
                    let mut keys=modifiers.iter().map(|m|modifier_key(m)).collect::<anyhow::Result<Vec<_>>>()?;
                    keys.push(named_key(&key)?);
                    #[cfg(target_os="linux")]
                    {Prepared::Keyboard(keyboard.as_ref().unwrap().plan_keys(&keys)?)}
                    #[cfg(not(target_os="linux"))]
                    {Prepared::Keys(shortcuts::resolve(&keys)?)}
                },
                pointer=>{preflight_pointer(&pointer,displays,check)?;Prepared::Pointer(pointer)},
            };
            let index=prepared.len();
            let mut record=json!({"index":index,"method":plan.method(),"phase":"prepared","status":"not_dispatched","completedNativeUnits":0,"currentUnitUncertain":false,"clipboard":"unchanged","cleanupFailure":null,"applicationVerified":false});
            match &plan {
                #[cfg(target_os="linux")] Prepared::Text(p)=>{record["totalNativeUnits"]=json!(p.unit_count());record["nativeUnitKind"]=json!(p.unit_kind());},
                #[cfg(target_os="linux")] Prepared::Keyboard(p)=>{record["totalNativeUnits"]=json!(p.unit_count());record["nativeUnitKind"]=json!(p.units_kind);},
                #[cfg(not(target_os="linux"))] Prepared::Native(p)=>{record["totalNativeUnits"]=json!(p.total_units());record["nativeUnitKind"]=json!(match p.unit_kind(){platform::UnitKind::Utf16CodeUnits=>"utf16_code_units",platform::UnitKind::UnicodeScalars=>"unicode_scalars"});},
                _=>{},
            }
            check.effects.lock().unwrap_or_else(std::sync::PoisonError::into_inner).records[index]=record;
            prepared.push(plan);
        }
        Ok(prepared)
        })?;
        for (index,plan) in prepared.into_iter().enumerate() {
            check.effects.lock().unwrap_or_else(std::sync::PoisonError::into_inner).current=Some(index);
            check.check()?;
            {let mut e=check.effects.lock().unwrap_or_else(std::sync::PoisonError::into_inner);e.current=Some(index);e.started=true;}
            update(check,index,|r|{r["phase"]=json!("dispatch");r["currentUnitUncertain"]=json!(true);});
            let mut progress=|units|update(check,index,|r|r["completedNativeUnits"]=json!(r["completedNativeUnits"].as_u64().unwrap_or(0)+units as u64));
            let execution=(||->anyhow::Result<()> {match plan {
                #[cfg(target_os="linux")]
                Prepared::Keyboard(plan)=>keyboard.as_mut().unwrap().execute_with_checks(&plan,&mut linux_text::Checks {wait:&mut ||check.fast(),full:&mut ||check.check()},&mut progress)?,
                #[cfg(target_os="linux")]
                Prepared::Text(plan)=>typing::execute_text_with_wait(keyboard.as_mut().unwrap(),&plan,&mut ||check.check(),&mut ||check.fast(),&mut |effects| {
                    update(check,index,|record| {
                        let value=serde_json::to_value(effects).expect("text effects serialize");
                        for (key,value) in value.as_object().unwrap() {record[key]=value.clone();}
                    });
                })?,
                #[cfg(not(target_os="linux"))]
                Prepared::Native(plan)=>{platform::execute_native(&plan,&mut ||check.check(),&mut progress)?;},
                #[cfg(not(target_os="linux"))]
                Prepared::Keys(keys)=>{shortcuts::send(&mut input,&keys,||check.check())?;progress(1);},
                Prepared::Pointer(action)=>{pointer_action(action,displays,check)?;progress(1);},
            };Ok(())})();
            if let Err(error)=execution {
                update(check,index,|r|{
                    r["status"]=json!("partial_unknown");r["error"]=json!(format!("{error:#}"));
                    #[cfg(target_os="linux")]
                    if let Some(facts)=linux_text::failure_facts(&error) {
                        r["completedNativeUnits"]=json!(facts.completed_units);r["currentUnitUncertain"]=json!(facts.current_unit_uncertain);
                        if facts.cleanup_failed {r["cleanupFailure"]=json!(format!("{error:#}"));}
                    }
                    #[cfg(not(target_os="linux"))]
                    if let Some(facts)=error.downcast_ref::<platform::NativeError>() {
                        r["currentUnitUncertain"]=json!(facts.uncertain);
                        if facts.cleanup_failed {r["cleanupFailure"]=json!(format!("{error:#}"));}
                    }
                });
                invalidate_frame();return Err(error);
            }
            update(check,index,|r|{r["phase"]=json!("complete");r["status"]=json!("dispatched_unverified");r["currentUnitUncertain"]=json!(false);});
            check.effects.lock().unwrap_or_else(std::sync::PoisonError::into_inner).completed=index+1;
            if let Some(progress)=&check.progress {progress.store(index+1,Ordering::SeqCst);}
        }
        Ok(())
        });
        #[cfg(target_os="linux")]
        if let Some(session)=keyboard.as_mut() {
            return finish_outcome(&check.effects,total,outcome,||session.finish());
        }
        outcome
    });
    Ok(check.effects.lock().unwrap_or_else(std::sync::PoisonError::into_inner).report(batch.then_some(total),result.err().map(|e|format!("{e:#}"))))
}

// Finalize regardless of primary success/failure, with the native context still
// alive outside the dispatch unwind boundary. Drop is only a last-resort backup.
#[cfg(any(test,target_os="linux"))]
fn finish_outcome(effects:&Mutex<Effects>,total:usize,outcome:anyhow::Result<()>,finish:impl FnOnce()->anyhow::Result<()>)->anyhow::Result<()> {
    {
        let mut effects=effects.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        effects.cleanup_phase=true;
        if effects.completed==total {effects.current=None;}
    }
    match native_boundary(finish) {
        Err(error)=>{
            effects.lock().unwrap_or_else(std::sync::PoisonError::into_inner).cleanup_failure=Some(format!("{error:#}"));
            outcome.and(Err(error))
        },
        Ok(())=>{
            effects.lock().unwrap_or_else(std::sync::PoisonError::into_inner).cleanup_phase=false;
            outcome
        },
    }
}

fn preflight_pointer(action:&Action,displays:&[Display],check:&Check)->anyhow::Result<()> {
    let display=match action {
        Action::Move{frame,x,y}|Action::Click{frame,x,y,..}=>coordinate_frame(frame,*x,*y,displays,check)?.display,
        Action::Drag{frame,x,y,to_x,to_y}=>{let start=coordinate_frame(frame,*x,*y,displays,check)?;coordinate_frame(frame,*to_x,*to_y,displays,check)?;start.display},
        Action::Scroll{..}=>{#[cfg(target_os="linux")] ensure!(displays.len()==1,"Wayland pointer control requires exactly one output");displays.first().context("No display")?.clone()},
        _=>unreachable!(),
    };
    #[cfg(target_os="linux")]
    linux_pointer::preflight(&display,||check.check())?;
    #[cfg(not(target_os="linux"))]
    let _=display;
    Ok(())
}


#[cfg(test)] mod effect_tests {
    use super::*;
    fn payload(result:BuiltinMcpCallResult)->Value {let BuiltinMcpContent::Text{text,..}=&result.content[0] else {panic!()};serde_json::from_str(text).unwrap()}
    #[test] fn timeout_reports_actual_selected_method_progress_and_clipboard() {
        let mut effects=Effects::default();
        let before=payload(effects.report(Some(2),Some("timeout".into())));
        assert_eq!(before["status"],"failed");assert_eq!(before["clipboard"],"unchanged");assert_eq!(before["partialActionPossible"],false);
        effects.started=true;effects.current=Some(1);effects.completed=1;
        effects.records=vec![json!({"method":"keyboard","completedNativeUnits":3,"currentUnitUncertain":false,"clipboard":"unchanged","status":"dispatched_unverified"}),json!({"method":"paste","phase":"clipboard_publication","completedNativeUnits":0,"currentUnitUncertain":false,"clipboard":"may_changed"})];
        let timed=payload(effects.report(Some(2),Some("timeout".into())));
        assert_eq!(timed["completed"],1);assert_eq!(timed["failedIndex"],1);assert_eq!(timed["status"],"partial_unknown");assert_eq!(timed["actions"][1]["method"],"paste");assert_eq!(timed["clipboardMayHaveChanged"],true);assert_eq!(timed["applicationVerified"],false);
    }
    #[test] fn primary_clipboard_error_still_finishes_and_preserves_both_failures() {
        let effects=Mutex::new(Effects{started:true,completed:1,current:Some(1),records:vec![json!({"method":"keyboard","status":"dispatched_unverified","completedNativeUnits":3,"currentUnitUncertain":false,"clipboard":"unchanged"}),json!({"method":"paste","status":"partial_unknown","completedNativeUnits":0,"currentUnitUncertain":false,"clipboard":"may_changed"})],..Default::default()});
        let finished=std::cell::Cell::new(false);
        let primary=finish_outcome(&effects,2,Err(anyhow::anyhow!("clipboard publication failed")),||{finished.set(true);bail!("keyboard destroy ACK failed")}).unwrap_err();
        assert!(finished.get());assert_eq!(primary.to_string(),"clipboard publication failed");
        let report=payload(effects.lock().unwrap().report(Some(2),Some(primary.to_string())));
        assert_eq!(report["completed"],1);assert_eq!(report["failedIndex"],1);assert_eq!(report["actions"][0]["status"],"dispatched_unverified");assert_eq!(report["phase"],"cleanup");assert_eq!(report["cleanupFailure"],"keyboard destroy ACK failed");assert_eq!(report["error"],"clipboard publication failed");
    }
    #[test] fn final_cleanup_failure_preserves_completed_actions_without_out_of_range_index() {
        let effects=Effects{started:true,completed:1,current:None,cleanup_phase:true,cleanup_failure:Some("destroy acknowledgement failed".into()),records:vec![json!({"method":"keyboard","status":"dispatched_unverified","phase":"complete","completedNativeUnits":3,"currentUnitUncertain":false,"clipboard":"unchanged"})]};
        let report=payload(effects.report(Some(1),Some("cleanup failed".into())));
        assert_eq!(report["completed"],1);assert!(report["failedIndex"].is_null());assert_eq!(report["phase"],"cleanup");assert_eq!(report["actions"][0]["status"],"dispatched_unverified");assert!(report["cleanupFailure"].is_string());assert_eq!(report["applicationVerified"],false);
    }
    #[test] fn known_pre_event_runtime_failure_does_not_claim_effects() {
        let effects=Effects{started:true,current:Some(0),records:vec![json!({"method":"keyboard","completedNativeUnits":0,"currentUnitUncertain":false,"clipboard":"unchanged"})],..Default::default()};
        let failed=payload(effects.report(None,Some("layout changed before event".into())));
        assert_eq!(failed["status"],"failed");assert_eq!(failed["currentUnitUncertain"],false);assert_eq!(failed["partialActionPossible"],false);
    }
}

}
