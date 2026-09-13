//! Opt-in real native Chromium regression. Lives under computer_use alongside
//! linux_text/linux_pointer/linux_clipboard. The launcher isolates every endpoint.
//! Fixture JS observes only; keyboard, pointer and paste use production modules.
use anyhow::{ensure, Context, Result};
use enigo::Key;
use serde_json::Value;
use std::{fs, process::Command, time::{Duration, Instant}};

fn guard() -> Result<()> {
    ensure!(std::env::var("NEOISM_LIVE_BROWSER_TEST").as_deref() == Ok("1"));
    ensure!(std::env::var("XDG_RUNTIME_DIR")? == std::env::var("NEOISM_BROWSER_RUNTIME")?);
    ensure!(!std::path::Path::new("/mnt/wayland").exists());
    ensure!(!std::path::Path::new("/dev/input").exists());
    let window = hypr("activewindow")?;
    let pid: i64 = std::env::var("NEOISM_BROWSER_PID")?.parse()?;
    ensure!(window["pid"].as_i64() == Some(pid), "Private browser not foreground: {window}");
    ensure!(window["xwayland"] == false, "Not native Wayland: {window}");
    ensure!(window["class"].as_str() == Some("neoism-browser-regression"), "Wrong app: {window}");
    Ok(())
}
fn hypr(query: &str) -> Result<Value> {
    let signature = std::env::var("HYPRLAND_INSTANCE_SIGNATURE")?;
    ensure!(!signature.is_empty() && !signature.contains('/'));
    let output = Command::new("hyprctl").args(["-i", &signature, "-j", query]).output()?;
    ensure!(output.status.success());
    Ok(serde_json::from_slice(&output.stdout)?)
}
fn report() -> Result<Value> {
    Ok(serde_json::from_slice(&fs::read(std::env::var("NEOISM_BROWSER_REPORT")?)?)?)
}
fn wait_for(label: &str, predicate: impl Fn(&Value)->bool) -> Result<Value> {
    let start = Instant::now();
    loop {
        if let Ok(value) = report() {
            if predicate(&value) {
                println!("ACK {label}: active={} values={} events={}", value["activeElement"], value["values"], value["events"].as_array().map_or(0, Vec::len));
                return Ok(value);
            }
            ensure!(start.elapsed() < Duration::from_secs(8), "FAIL {label}: browser active={} focused={} exact values={} (full events retained in reports.jsonl)", value["activeElement"], value["focused"], value["values"]);
        }
        ensure!(start.elapsed() < Duration::from_secs(8), "No browser ACK for {label}");
        std::thread::sleep(Duration::from_millis(25));
    }
}
fn keys(keys: &[Key]) -> Result<()> { guard()?; super::linux_text::send_keys(keys, guard) }
fn text(text: &str) -> Result<()> { guard()?; super::linux_text::send(text, guard) }
fn value_is(report:&Value, id:&str, value:&str)->bool {
    report["focused"] == true && report["activeElement"] == id && report["values"][id] == value
}
fn fresh(label: &str, before:&Value) -> Result<Value> {
    let seq=before["seq"].as_u64().context("report sequence")?;
    wait_for(label, |r| r["seq"].as_u64().unwrap_or(0) > seq+2)
}
fn click(field: &str) -> Result<()> {
    use super::linux_pointer::{Point, Command as PointerCommand};
    guard()?;
    let before=report()?;
    let output=Command::new("python3").args([
        &std::env::var("NEOISM_BROWSER_LAUNCHER")?, "--observe-pointer",
        &std::env::var("NEOISM_BROWSER_RUN")?, field,
    ]).output()?;
    ensure!(output.status.success(), "Screenshot observation failed: {}", String::from_utf8_lossy(&output.stderr));
    let observed:Value=serde_json::from_slice(&output.stdout)?;
    println!("SCREENSHOT TARGET {field}: {observed}");
    let d=&observed["display"];
    let display=super::Display{id:d["id"].as_str().context("display name")?.into(),
        x:d["x"].as_i64().context("display x")? as i32,y:d["y"].as_i64().context("display y")? as i32,
        width:d["width"].as_u64().context("logical width")? as u32,height:d["height"].as_u64().context("logical height")? as u32};
    let p=&observed["point"];
    let point=Point{x:p["x"].as_u64().context("pixel x")? as u32,y:p["y"].as_u64().context("pixel y")? as u32,
        width:p["width"].as_u64().context("screenshot width")? as u32,height:p["height"].as_u64().context("screenshot height")? as u32};
    guard()?;
    super::linux_pointer::send(&display,PointerCommand::Click(point,0x110),guard)?;
    let serial=before["pointerCount"].as_u64().context("pointer counter")?;
    let after=wait_for("native pointerdown and focus", |r| r["pointerCount"].as_u64().unwrap_or(0)>serial && r["activeElement"]==field)?;
    ensure!(after["values"] == before["values"], "Click unexpectedly changed a field");
    let event=after["events"].as_array().context("events")?.iter().rev().find(|e|e["type"]=="pointerdown").context("pointerdown")?;
    ensure!(event["target"]==field, "Native click hit wrong target: {event}");
    let rect=&before["rects"][field];
    let center=|axis:&str,extent:&str| ->Result<f64> {Ok(rect[axis].as_f64().context("rect axis")?+rect[extent].as_f64().context("rect extent")?*if axis=="y" {0.75} else {0.5})};
    ensure!((event["x"].as_f64().context("clientX")?-center("x","width")?).abs()<3.0
        && (event["y"].as_f64().context("clientY")?-center("y","height")?).abs()<3.0,
        "Screenshot-selected click disagrees with browser CSS coordinates: event={event}, rect={rect}");
    println!("PASS screenshot native click {field}: {event}");
    Ok(())
}
fn clear(field:&str)->Result<()> {
    keys(&[Key::Control,Key::Unicode('a')])?;
    keys(&[Key::Backspace])?;
    // Chromium leaves a layout-only <br> in an emptied contenteditable. Its
    // innerText is "\n" although there are no text characters. Check textContent
    // only for this empty-state ACK; all delivered strings remain exact innerText.
    wait_for("shortcut clear",|r| if field=="editable" {
        r["focused"]==true && r["activeElement"]==field && r["editableTextContent"]==""
    } else {value_is(r,field,"")})?;
    Ok(())
}
fn paste(field:&str,content:&str)->Result<()> {
    guard()?;
    let _owner=super::linux_clipboard::publish(content,&guard)?;
    keys(&[Key::Control,Key::Unicode('v')])?;
    wait_for("explicit native clipboard paste exact",|r|value_is(r,field,content))?;
    Ok(())
}
#[test]
#[ignore = "requires scripts/check-computer-browser-live.py isolated native browser"]
fn hyprland_production_browser_roundtrip() -> Result<()> {
    let expected_scale: f64=std::env::var("NEOISM_BROWSER_SCALE")?.parse()?;
    let monitors=hypr("monitors")?;
    ensure!(monitors.as_array().is_some_and(|m|m.len()==1)
        && monitors[0]["scale"].as_f64().is_some_and(|s|(s-expected_scale).abs()<0.001),
        "Requested output scale not active: {monitors}");
    wait_for("browser-local ready", |r| r["focused"] == true && r["activeElement"] == "input")?;
    keys(&[Key::Control, Key::Unicode('l')])?;
    keys(&[Key::Unicode('x')])?;
    guard()?;
    if let Ok(path) = std::env::var("NEOISM_BROWSER_SCREENSHOT") {
        ensure!(Command::new("grim").arg(path).status()?.success(), "Private screenshot failed");
    }
    let previous=report()?;
    let query="?q=Synapse%20Communications&src=typed_query&f=user";
    let local_url=format!("{}{query}",std::env::var("NEOISM_BROWSER_URL")?);
    // Actual omnibox navigation: no DOM/CDP navigation, and no F6 workaround.
    keys(&[Key::Control,Key::Unicode('l')])?;
    text(&local_url)?;
    keys(&[Key::Return])?;
    let navigated=wait_for("native omnibox URL + Return navigation", |r|
        r["generation"].as_str().is_some() && r["generation"]!=previous["generation"]
        && r["href"]==local_url && r["query"]==query && r["ready"]=="complete"
        && r["focused"]==true && r["activeElement"]=="input")?;
    let requests=fs::read_to_string(std::env::var("NEOISM_BROWSER_NAVIGATION")?)?;
    let target=format!("/{query}");
    let count=requests.lines().filter_map(|line|serde_json::from_str::<Value>(line).ok())
        .filter(|request|request["path"]==target).count();
    ensure!(count==1,"Expected exactly one local server navigation to {target}, observed {count}");
    println!("PASS OMNIBOX NAVIGATION: native Text URL={local_url}; server_requests={count}; generation {} -> {}; exact href={} query={}",
        previous["generation"],navigated["generation"],navigated["href"],navigated["query"]);
    keys(&[Key::Unicode('x')])?;
    wait_for("Action::Key x delivered", |r| value_is(r,"input","x"))?;
    clear("input")?;
    let url="https://example.invalid/neoism?q=Abcdefghijklmnopqrstuvwxyz+42&z=%23#test";
    text(url)?;
    wait_for("Action::Text URL exact delivery", |r| value_is(r,"input",url))?;

    // More than 48 distinct printable scalars: exact chunk ordering, punctuation,
    // combining sequence and RTL are all checked, not normalized or shortened.
    let bmp=format!("{} Grüße — café e\u{301} Ελληνικά 日本語 العربية עברית END",('!'..='~').collect::<String>());
    let full="Full Unicode: Grüße e\u{301} العربية 日本語 🦀 👩\u{200d}💻 🏳️\u{200d}🌈";
    let baseline=report()?["viewport"]["dpr"].as_f64().context("browser DPR")?;
    ensure!((baseline-expected_scale).abs()<0.02, "Fresh browser is not at 100 percent zoom: DPR={baseline}, output scale={expected_scale}");
    for zoom in [100,125] {
        if zoom==125 {
            for _ in 0..2 {
                let before=report()?;
                keys(&[Key::Control,Key::Unicode('=')])?;
                fresh("browser zoom change ACK",&before)?;
            }
            wait_for("125 percent browser zoom",|r|r["viewport"]["dpr"].as_f64().is_some_and(|d|(d-baseline*1.25).abs()<0.02))?;
        }
        println!("MATRIX browser_zoom={zoom} monitor={}",hypr("monitors")?);
        for field in ["input","textarea","editable"] {
            click(field)?;
            clear(field)?;
            text(&bmp)?;
            wait_for("native ASCII BMP combining RTL chunked exact",|r|value_is(r,field,&bmp))?;
            let before=report()?;
            // The supplementary scalar is after >48 unique valid scalars:
            // rejection must happen before the FIRST chunk is injected.
            let invalid=format!("{bmp}🦀");
            let error=text(&invalid).expect_err("supplementary native text must reject before input");
            ensure!(format!("{error:#}").contains("supplementary Unicode"), "Wrong rejection reason: {error:#}");
            println!("EXPECTED supplementary rejection: {error:#}");
            let after=fresh("supplementary rejection observed",&before)?;
            ensure!(after["values"]==before["values"] && after["inputCount"]==before["inputCount"],
                "Rejected text partially changed browser fields: before={} after={}",before["values"],after["values"]);
            clear(field)?;
            let full=if field=="input" {full.to_owned()} else {format!("{full}\nline two\tliteral tab END")};
            paste(field,&full)?;
            clear(field)?;
            keys(&[Key::Unicode('x')])?;
            wait_for("post-paste shortcut and native key",|r|value_is(r,field,"x"))?;
            if field=="textarea" {
                keys(&[Key::Return])?;
                wait_for("explicit Return inserts newline",|r|value_is(r,field,"x\n"))?;
            }
        }
    }
    Ok(())
}
