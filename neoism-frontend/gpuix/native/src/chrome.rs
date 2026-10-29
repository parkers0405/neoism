//! Original chrome state and paint. No parallel layout or invented menu catalog.
use neoism_ui::{panels::{chrome_topbar::{ChromeTopBar, ServerIndicatorStatus},status_line::{StatusInfo,StatusLine}},primitives::{IdeTheme,paint_record::RecordingPainter}};
use serde_json::{json,Value};
use std::{cell::RefCell,collections::HashMap};

enum Kind { Top(ChromeTopBar), Status(StatusLine) }
struct Host { kind:Kind,paint:RecordingPainter,info:Value }
thread_local! { static HOSTS:RefCell<HashMap<u64,Host>>=RefCell::new(HashMap::new());static NEXT:RefCell<u64>=RefCell::new(0x9000_0000); }
fn n(v:&Value,key:&str,fallback:f32)->f32{v[key].as_f64().map(|n|n as f32).unwrap_or(fallback)}
fn render(host:&mut Host,v:&Value)->Result<Value,String>{
    let width=n(v,"width",800.0).max(0.0);let scale=n(v,"scale",1.0).clamp(0.5,3.0);
    let theme=IdeTheme::by_name(v["theme"].as_str().unwrap_or("pastel_dark"));
    host.paint.begin_frame();
    match &mut host.kind {
        Kind::Top(top)=>{
            top.set_scale(scale);top.set_visible(true);
            if let Some(peers)=v["peers"].as_array(){
                top.set_peers(peers.iter().filter_map(|p|Some(neoism_ui::editor::crdt::PresenceAvatarPeer{
                    peer_id:p["peer_id"].as_str()?.to_owned(),display_name:p["display_name"].as_str()?.to_owned(),
                    client_os:p["client_os"].as_str().map(str::to_owned),
                    color:[p["color"][0].as_u64()?.min(255)as u8,p["color"][1].as_u64()?.min(255)as u8,p["color"][2].as_u64()?.min(255)as u8],
                    rainbow:p["rainbow"].as_bool().unwrap_or(false),
                })).collect());
            }
            if let Some(s)=v["sidebar"].as_str(){top.set_panel_open(s=="files");top.set_notes_open(s=="notes");top.set_conversations_open(s=="conversations");}
            if v["sidebar"].is_null(){top.set_panel_open(false);top.set_notes_open(false);top.set_conversations_open(false);}
            top.set_search_open(v["search"].as_bool().unwrap_or(false));
            top.set_right_button_visible(v["agent"].as_bool().unwrap_or(false));
            top.set_right_panel_open(v["details"].as_bool().unwrap_or(false));
            top.set_server_status(match v["connection"].as_str(){Some("connected")=>ServerIndicatorStatus::Online,Some("connecting"|"authenticating"|"waiting")=>ServerIndicatorStatus::Connecting,Some("auth-rejected"|"disconnected")=>ServerIndicatorStatus::Offline,_=>ServerIndicatorStatus::Unknown});
            if v["op"].as_str()==Some("chrome.frame") && v["menu"].as_bool()==Some(false){top.close_menu();}
            let frame=top.paint_host(&mut host.paint,width,n(v,"height",900.0),&theme);
            let action=top.take_action().map(|a|format!("{a:?}"));
            Ok(json!({"commands":host.paint.commands(),"regions":frame.regions,"menuOpen":frame.menu_open,"menuRect":frame.menu_rect,"animating":frame.animating,"height":top.height(),"action":action}))
        }
        Kind::Status(line)=>{
            line.set_visible(true);line.set_scale(scale);
            if let Some(info)=v.get("info"){if &host.info!=info{line.set_info(serde_json::from_value::<StatusInfo>(info.clone()).map_err(|e|e.to_string())?);host.info=info.clone();}}
            if let Some(hover)=v["branchHover"].as_bool(){line.set_git_branch_hovered(hover);}
            line.set_split_toggle(v["splitEnabled"].as_bool().unwrap_or(false),v["splitHidden"].as_bool().unwrap_or(false));
            line.render_with_ide_theme(&mut host.paint,0.0,0.0,width,&theme);
            let mut hits=Vec::new();
            // Keep diagnostic/LSP anchors and all hit classification inside the original panel.
            for (name,pill) in [("error",neoism_ui::panels::status_line::DiagnosticPill::Error),("warn",neoism_ui::panels::status_line::DiagnosticPill::Warn)] {if let Some((x,y))=line.diagnostic_pill_anchor(pill){hits.push(json!({"kind":name,"x":x,"y":y}));}}
            Ok(json!({"commands":host.paint.commands(),"height":line.scaled_height(),"animating":line.is_animating(),"anchors":hits}))
        }
    }
}
pub fn dispatch(v:&Value)->Option<Result<Value,String>>{
    let op=v["op"].as_str()?;if !op.starts_with("chrome."){return None;}
    Some(HOSTS.with(|store|{
        let mut store=store.borrow_mut();
        if op=="chrome.new"{
            let id=NEXT.with(|next|{let mut n=next.borrow_mut();*n+=1;*n});
            let kind=if v["kind"].as_str()==Some("status"){Kind::Status(StatusLine::new())}else{Kind::Top(ChromeTopBar::new())};
            let extras=v["fonts"].as_array().map(|a|a.iter().filter_map(|s|s.as_str().map(str::to_owned)).collect()).unwrap_or_default();
            let paint=RecordingPainter::new(v["family"].as_str().unwrap_or("Cascadia Code NF"),v["weight"].as_u64().unwrap_or(400) as u16,extras);
            store.insert(id,Host{kind,paint,info:Value::Null});return Ok(json!({"id":id}));
        }
        let id=v["id"].as_u64().ok_or("Missing chrome id")?;
        if op=="chrome.dispose"{store.remove(&id);return Ok(json!({}));}
        let host=store.get_mut(&id).ok_or("Unknown chrome id")?;
        if let Kind::Top(top)=&mut host.kind{
            match op {"chrome.pointer"=>{top.pointer_move(n(v,"x",-1.0),n(v,"y",-1.0));},"chrome.click"=>{top.pointer_down(n(v,"x",-1.0),n(v,"y",-1.0));},_=>{}}
        }
        if op=="chrome.hit"{if let Kind::Status(line)=&mut host.kind{return Ok(json!({"action":line.click_action_at(n(v,"x",-1.0),n(v,"y",-1.0)),"branch":line.git_branch_at(n(v,"x",-1.0),n(v,"y",-1.0))}));}}
        render(host,v)
    }))
}
