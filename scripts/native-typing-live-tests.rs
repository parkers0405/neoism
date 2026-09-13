//! Opt-in real GTK / vanilla Minecraft acceptance through the production typing facade.
use anyhow::{ensure, Context, Result};
use enigo::Key;
use serde_json::Value;
use std::{fs, process::Command, time::{Duration, Instant}};
use super::{linux_text::KeyboardSession, typing::{self, Method, ClipboardPolicy, ClipboardPermit}};
fn hypr(query:&str)->Result<Value> {
    let signature=std::env::var("HYPRLAND_INSTANCE_SIGNATURE")?;
    ensure!(!signature.is_empty() && !signature.contains('/'));
    let out=Command::new("hyprctl").args(["-i",&signature,"-j",query]).output()?;
    ensure!(out.status.success()); Ok(serde_json::from_slice(&out.stdout)?)
}
fn is_mc()->bool {std::env::var("NEOISM_NATIVE_APP").as_deref()==Ok("minecraft")}
fn guard() -> Result<()> {
    ensure!(std::env::var("NEOISM_NATIVE_LIVE").as_deref()==Ok("1"));
    ensure!(std::env::var("XDG_RUNTIME_DIR")?==std::env::var("NEOISM_NATIVE_RUNTIME")?);
    ensure!(!std::path::Path::new("/mnt/wayland").exists());
    ensure!(!std::path::Path::new("/dev/input").exists());
    let w=hypr("activewindow")?;
    ensure!(w["pid"].as_u64()==Some(std::env::var("NEOISM_NATIVE_PID")?.parse()?),"Wrong foreground: {w}");
    if !is_mc(){ensure!(w["xwayland"]==false);}
    else {
        let r=report()?;
        ensure!(r["hasWorld"]==false,"World was opened; abort");
        let screen=r["screen"].as_str().unwrap_or("");
        let preparing=screen.ends_with("GenericMessageScreen") && r["widgets"].as_array().is_some_and(|a|a.iter().any(|w|w["label"]=="Preparing for world creation..."));
        ensure!(preparing || ["AccessibilityOnboardingScreen","TitleScreen","SelectWorldScreen","CreateWorldScreen"].iter().any(|s|screen.ends_with(s)),"Unapproved Minecraft screen: {screen}");
    }
    Ok(())
}
fn report()->Result<Value> {Ok(serde_json::from_slice(&fs::read(std::env::var("NEOISM_NATIVE_REPORT")?)?)?)}
fn until(label:&str,predicate:impl Fn(&Value)->bool)->Result<Value> {
    let start=Instant::now();
    loop {
        let r=report()?;
        if predicate(&r){return Ok(r)}
        ensure!(start.elapsed()<Duration::from_secs(15),"Timeout {label}: {r}");
        std::thread::sleep(Duration::from_millis(100));
    }
}
fn wait(expected:&str)->Result<Value> {
    until(expected,|r|r["focused"]==true && if is_mc(){r["widgets"].as_array().is_some_and(|a|a.iter().any(|w|w["isFocused"]==true && w["value"]==expected))}else{r["entryFocused"]==true && r["value"]==expected})
}
fn keys(s:&mut KeyboardSession,chord:&[Key])->Result<()> {let p=s.plan_keys(chord)?;s.execute(&p,&mut guard,&mut |_|{})}
fn text(s:&mut KeyboardSession,t:&str,policy:ClipboardPolicy,permit:Option<ClipboardPermit>)->Result<()> {
    let p=typing::prepare_text(s,t,Method::Auto,policy,&[Key::Control,Key::Unicode('v')],false,permit,&mut guard)?;
    typing::execute_text(s,&p,&mut guard,&mut |e|{if e.phase=="complete"{println!("FACADE {}",serde_json::to_string(e).unwrap());}})
}
fn rejected(s:&KeyboardSession)->Result<()> {
    let before=report()?;
    for (t,policy) in [("🦀DO NOT EMIT A PREFIX",ClipboardPolicy::Forbid),("🦀DO NOT EMIT A PREFIX",ClipboardPolicy::Replace)] {
        ensure!(typing::prepare_text(s,t,Method::Auto,policy,&[Key::Control,Key::Unicode('v')],false,None,&mut guard).is_err(),"Unsupported/unauthorized whole request accepted");
    }
    std::thread::sleep(Duration::from_millis(400));let after=report()?;
    if is_mc(){ensure!(before["widgets"]==after["widgets"],"Rejected request changed EditBox");}
    else {ensure!(before["value"]==after["value"] && before["events"]==after["events"],"Rejected request emitted GTK input");}
    println!("PASS facade unsupported Unicode before prefix + missing clipboard permit: no observed mutation");Ok(())
}
fn clear(s:&mut KeyboardSession)->Result<()> {keys(s,&[Key::Control,Key::Unicode('a')])?;keys(s,&[Key::Backspace])?;wait("")?;Ok(())}
fn screenshot(label:&str)->Result<()> {
    guard()?;let run=std::env::var("NEOISM_NATIVE_RUN")?;
    let status=Command::new("timeout").args(["10","grim","-o","NATIVE-TEST",&format!("{run}/{label}.png")]).status()?;
    ensure!(status.success(),"Private screenshot failed: {label}");Ok(())
}
fn click_widget(w:&Value,r:&Value,label:&str)->Result<()> {
    guard()?; screenshot(label)?;
    let window=hypr("activewindow")?;
    let monitor=hypr("monitors")?;
    ensure!(monitor.as_array().is_some_and(|m|m.len()==1) && monitor[0]["name"]=="NATIVE-TEST" && monitor[0]["scale"]==1.0);
    let n=|v:&Value|v.as_f64().context("widget geometry");
    let x=n(&window["at"][0])?+(n(&w["getX"])?+n(&w["getWidth"])?/2.)*n(&window["size"][0])?/n(&r["getGuiScaledWidth"])?;
    let y=n(&window["at"][1])?+(n(&w["getY"])?+n(&w["getHeight"])?/2.)*n(&window["size"][1])?/n(&r["getGuiScaledHeight"])?;
    ensure!(w["visible"]==true && w["isActive"]==true);
    println!("OBSERVED CLICK {label}: widget={w} point={x},{y} window={window}");
    let display=super::Display{id:"NATIVE-TEST".into(),x:0,y:0,width:1280,height:800};
    let point=super::linux_pointer::Point{x:x.round() as u32,y:y.round() as u32,width:1280,height:800};
    super::linux_pointer::send(&display,super::linux_pointer::Command::Click(point,0x110),guard)
}
#[test]
#[ignore="requires isolated native launcher"]
fn native_gtk_existing_layout_roundtrip()->Result<()> {
    wait("")?; let mut s=KeyboardSession::open(&mut guard)?;
    rejected(&s)?;
    let t="Synapse Communications: AbC xyz 0123456789 !@#$%^&*()_+-=[]{};:'\",.<>/?\\|`~";
    text(&mut s,t,ClipboardPolicy::Forbid,None)?;
    let ascii=wait(t); println!("{} GTK exact ASCII {t:?}",if ascii.is_ok(){"PASS"}else{"FAIL"});
    clear(&mut s)?;
    ensure!(std::env::var("NEOISM_NATIVE_CLIPBOARD_POLICY").as_deref()==Ok("allow"),"Fixture clipboard opt-in required");
    let unicode="🦀 Native clipboard: Grüße 日本語 e\u{301} END";
    text(&mut s,unicode,ClipboardPolicy::Replace,Some(ClipboardPermit::granted()))?;wait(unicode)?;
    println!("PASS facade PRIVATE clipboard exact Unicode");screenshot("gtk-final")?;s.finish()?;ascii?;Ok(())
}
#[test]
#[ignore="requires isolated vanilla 26.2 launcher with read-only observer"]
fn native_minecraft_editbox_roundtrip()->Result<()> {
    // Only whitelist menu navigation. Never click the final Create New World button.
    for i in 0..5 {
        let r=until("menu ready",|r|r["overlay"]==false && !r["screen"].as_str().unwrap_or("").ends_with("GenericMessageScreen") && r["widgets"].as_array().is_some_and(|a|!a.is_empty()))?;
        guard()?;
        let screen=r["screen"].as_str().context("screen")?;
        if screen.ends_with("CreateWorldScreen") {break;}
        let label=if screen.ends_with("AccessibilityOnboardingScreen"){"Continue"}
            else if screen.ends_with("TitleScreen"){"Singleplayer"}
            else if screen.ends_with("SelectWorldScreen"){"Create New World"}
            else {anyhow::bail!("Unexpected screen {screen}")};
        let widget=r["widgets"].as_array().context("widgets")?.iter().find(|w|w["label"]==label).with_context(||format!("Missing menu label {label}: {r}"))?;
        click_widget(widget,&r,&format!("menu-{i}"))?;
        until("screen transition",|after|after["screen"]!=r["screen"])?;
    }
    let r=until("world NAME editor",|r|r["screen"].as_str().is_some_and(|s|s.ends_with("CreateWorldScreen")) && r["widgets"].as_array().is_some_and(|a|a.iter().any(|w|w.get("value").is_some())))?;
    let edits=r["widgets"].as_array().unwrap().iter().filter(|w|w.get("value").is_some()).collect::<Vec<_>>();
    ensure!(edits.len()==1,"Ambiguous editor; never guess: {r}");
    click_widget(edits[0],&r,"world-name-before")?;
    let mut s=KeyboardSession::open(&mut guard)?;
    clear(&mut s)?; rejected(&s)?;
    for (i,t) in ["/balance Mr_Settle 42","AbC xyz 0123456789","!@#$%^&*()_+-=[]{}",";:'\",.<>/?\\|`~"].iter().enumerate() {
        ensure!(t.chars().count()<=32);
        clear(&mut s)?;text(&mut s,t,ClipboardPolicy::Forbid,None)?;
        let ack=wait(t)?;screenshot(&format!("editbox-{i}"))?;
        println!("PASS Minecraft 26.2 real EditBox exact {t:?}; observer={ack}");
    }
    ensure!(report()?["hasWorld"]==false);
    s.finish()?;
    println!("PASS no world opened/created; stopping at name editor");Ok(())
}
