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
            top.set_databases_available(true);
            // OS controls use window logical pixels; the panel scales its insets internally.
            top.set_left_safe_inset(n(v,"leftSafeInset",0.0)/scale);
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
            // Opt-in per frame (also pass rail on pointer/click requests).
            let rail=v["rail"].as_bool().unwrap_or(false);
            let viewport_height=n(v,"height",900.0).max(0.0);
            let frame=if rail {top.paint_rail_host(&mut host.paint,width,viewport_height,&theme)} else {top.paint_host(&mut host.paint,width,viewport_height,&theme)};
            let action=top.take_action().map(|a|format!("{a:?}"));
            let mut result=json!({"commands":host.paint.commands(),"regions":frame.regions,"menuOpen":frame.menu_open,"menuRect":frame.menu_rect,"animating":frame.animating,"height":if rail {viewport_height} else {top.height()},"action":action});
            if rail {result["width"]=json!(top.rail_width());}
            Ok(result)
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
            if let Some(rect)=line.lsp_pill_rect(){hits.push(json!({"kind":"lsp","x":rect.x,"y":rect.y,"width":rect.w,"height":rect.h}));}
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rail_slots_menu_actions_and_top_path_geometry() {
        let id = dispatch(&json!({"op":"chrome.new"})).unwrap().unwrap()["id"].clone();
        for scale in [1.0, 1.5, 2.0] {
            for agent in [false, true] {
                let params = json!({"op":"chrome.frame","id":id,"rail":true,"scale":scale,
                    "width":800,"height":900,"agent":agent,"menu":false,
                    "peers":[{"peer_id":"p","display_name":"Peer","color":[10,20,30]}]});
                let frame = dispatch(&params).unwrap().unwrap();
                assert_eq!(frame["width"], 52.0 * scale);
                assert_eq!(frame["height"], 900.0);
                let regions = frame["regions"].as_array().unwrap();
                let ids = ["topbar-menu", "toggle-files", "toggle-notes", "toggle-conversations", "open-finder", "open-servers", "presence:p"];
                assert_eq!(regions.len(), ids.len());
                for (index, expected) in ids.iter().enumerate() {
                    assert_eq!(regions[index]["id"], *expected);
                    assert_eq!(regions[index]["rect"], json!([0.0, (6.0 + 48.0 * index as f64) * scale, 52.0 * scale, 48.0 * scale]));
                }
                // Slot edges, outside the 40px visual, still dispatch actions.
                let mut click = params.clone();
                click["op"] = json!("chrome.click");
                click["x"] = json!(1.0);
                click["y"] = json!(55.0 * scale);
                assert_eq!(dispatch(&click).unwrap().unwrap()["action"], "ToggleFileTree");
                click["y"] = json!(7.0 * scale);
                let opened = dispatch(&click).unwrap().unwrap();
                assert_eq!(opened["menuOpen"], true);
                let menu = opened["menuRect"].as_array().unwrap();
                assert_eq!(menu[0], 52.0 * scale);
                assert!(menu[0].as_f64().unwrap() + menu[2].as_f64().unwrap() <= 800.0);
                assert!(menu[1].as_f64().unwrap() + menu[3].as_f64().unwrap() <= 900.0);
                let settings = opened["regions"].as_array().unwrap().iter().find(|r| r["id"] == "menu:OpenSettings").unwrap();
                click["x"] = json!(settings["rect"][0].as_f64().unwrap() + 10.0);
                click["y"] = json!(settings["rect"][1].as_f64().unwrap() + 10.0);
                assert_eq!(dispatch(&click).unwrap().unwrap()["action"], "OpenSettings");
            }
        }
        dispatch(&json!({"op":"chrome.frame","id":id,"rail":true,"width":220,"height":166})).unwrap().unwrap();
        let clamped = dispatch(&json!({"op":"chrome.click","id":id,"rail":true,"width":220,"height":166,"x":1,"y":7})).unwrap().unwrap();
        let menu = clamped["menuRect"].as_array().unwrap();
        assert_eq!(clamped["menuOpen"], true);
        assert!(menu[0].as_f64().unwrap() >= 0.0);
        assert!(menu[1].as_f64().unwrap() >= 0.0);
        assert!(menu[0].as_f64().unwrap() + menu[2].as_f64().unwrap() <= 220.0);
        assert!(menu[1].as_f64().unwrap() + menu[3].as_f64().unwrap() <= 166.0);
        let settings = clamped["regions"].as_array().unwrap().iter().find(|r| r["id"] == "menu:OpenSettings").unwrap();
        let clicked = dispatch(&json!({"op":"chrome.click","id":id,"rail":true,"width":220,"height":166,
            "x":settings["rect"][0].as_f64().unwrap() + 1.0,
            "y":settings["rect"][1].as_f64().unwrap() + 10.0})).unwrap().unwrap();
        assert_eq!(clicked["action"], "OpenSettings");
        let top = dispatch(&json!({"op":"chrome.frame","id":id,"width":800,"height":900,"menu":false})).unwrap().unwrap();
        assert!(top.get("width").is_none());
        assert_eq!(top["height"], 30.0);
        assert_eq!(top["regions"][0]["rect"], json!([8.0, 4.0, 22.0, 22.0]));
        dispatch(&json!({"op":"chrome.dispose","id":id})).unwrap().unwrap();
    }

    #[test]
    fn database_menu_region_click_and_sidebar_selection_do_not_synthesize_actions() {
        let id = dispatch(&json!({"op":"chrome.new"})).unwrap().unwrap()["id"].clone();
        let frame = dispatch(&json!({"op":"chrome.frame","id":id,"width":800,"sidebar":"databases"})).unwrap().unwrap();
        assert!(!frame["regions"].as_array().unwrap().iter().any(|r| r["id"] == "toggle-databases"));
        assert!(frame["action"].is_null());
        let menu = frame["regions"].as_array().unwrap().iter().find(|r| r["id"] == "topbar-menu").unwrap();
        let bounds = menu["rect"].as_array().unwrap();
        let opened = dispatch(&json!({
            "op":"chrome.click","id":id,"width":800,
            "x":bounds[0].as_f64().unwrap() + bounds[2].as_f64().unwrap() / 2.0,
            "y":bounds[1].as_f64().unwrap() + bounds[3].as_f64().unwrap() / 2.0,
        })).unwrap().unwrap();
        let region = opened["regions"].as_array().unwrap().iter()
            .find(|r| r["id"] == "menu:OpenDatabases").unwrap();
        assert_eq!(region["label"], "Database");
        assert_eq!(region["menu"], true);
        let bounds = region["rect"].as_array().unwrap();
        let clicked = dispatch(&json!({
            "op":"chrome.click","id":id,"width":800,"sidebar":"databases",
            "x":bounds[0].as_f64().unwrap() + bounds[2].as_f64().unwrap() / 2.0,
            "y":bounds[1].as_f64().unwrap() + bounds[3].as_f64().unwrap() / 2.0,
        })).unwrap().unwrap();
        assert_eq!(clicked["action"], "OpenDatabases");
        assert_eq!(clicked["menuOpen"], false);
        for sidebar in [json!("notes"), json!("files"), json!("conversations"), json!(null), json!("databases")] {
            let frame = dispatch(&json!({"op":"chrome.frame","id":id,"width":800,"sidebar":sidebar})).unwrap().unwrap();
            assert!(frame["action"].is_null());
        }
        dispatch(&json!({"op":"chrome.dispose","id":id})).unwrap().unwrap();
    }
}
