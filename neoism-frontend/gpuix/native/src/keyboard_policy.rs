//! Canonical desktop keyboard policies and the original chrome trail cursor.
use neoism_ui::{panels::{buffer_tabs::{BufferTabs,BufferTab},trail_cursor::TrailCursor},widgets::island::Island};
use serde_json::{json,Value};
use std::{cell::RefCell,collections::HashMap,time::Instant};
thread_local!{static CURSORS:RefCell<HashMap<u32,(TrailCursor,Instant)>>=RefCell::new(HashMap::new());static NEXT:RefCell<u32>=RefCell::new(0xa0000000);}
pub fn dispatch(v:&Value)->Option<Result<Value,String>>{
 let op=v["op"].as_str()?;if !op.starts_with("keyboard."){return None;}
 let n=|name:&str,default:f32|v[name].as_f64().map(|n|n as f32).unwrap_or(default);
 Some(Ok(match op{
  "keyboard.workspace-index"=>json!(neoism_ui::key_policy::workspace_index_for_alt_digit(v["key"].as_str().unwrap_or("").chars().next().unwrap_or('\0'),v["shift"].as_bool().unwrap_or(false),v["ctrl"].as_bool().unwrap_or(false),v["alt"].as_bool().unwrap_or(false),v["cmd"].as_bool().unwrap_or(false))),
  "keyboard.focus-step"=>{
   let count=v["count"].as_u64().unwrap_or(0).min(10000) as usize;let index=v["index"].as_u64().unwrap_or(0) as usize;let previous=v["previous"].as_bool().unwrap_or(false);
   if v["kind"].as_str()==Some("workspace"){
    let mut island=Island::new([0.0;4],[0.0;4],[0.0;4],false);island.set_focused(true,index,count);island.move_focus_cursor(previous,count);json!(island.focus_cursor(count))
   }else{
    let mut tabs=BufferTabs::<()>::new();tabs.visible=true;tabs.focused=true;tabs.focused_index=index;
    tabs.tabs=(0..count).map(|_|BufferTab{title:String::new(),modified:false,custom_icon:None,path:None,markdown:false,terminal_route_id:None,neoism_agent_route_id:None,chrome_page:None,agent_kind:None}).collect();
    tabs.move_focused(previous);json!(tabs.focused_index())
   }
  },
  "keyboard.ex-target"=>{
   use neoism_ui::editor::scroll_model::{parse_ex_command,MarkdownExCommandPlan};
   let command=v["query"].as_str().unwrap_or("").trim().trim_start_matches(':').trim();
   match parse_ex_command(command).map(|(head,_)|MarkdownExCommandPlan::classify(&head)){
    Some(MarkdownExCommandPlan::JumpToLine(_)|MarkdownExCommandPlan::JumpToLastLine)=>json!({"action":"ExCommand","argument":command}),_=>Value::Null
   }
  },
  "keyboard.cursor.new"=>{let id=NEXT.with(|next|{let mut next=next.borrow_mut();*next+=1;*next});CURSORS.with(|cursors|cursors.borrow_mut().insert(id,(TrailCursor::new(),Instant::now())));json!({"id":id})},
  "keyboard.cursor.seed"=>{
   let rect=v["rect"].as_array().map(|values|std::array::from_fn::<_,4,_>(|index|values.get(index).and_then(Value::as_f64).unwrap_or(0.0) as f32)).unwrap_or([0.0;4]);
   CURSORS.with(|cursors|{if let Some((trail,last))=cursors.borrow_mut().get_mut(&(v["id"].as_u64().unwrap_or(0) as u32)){
    trail.set_cursor_shape(neoism_terminal_core::ansi::CursorShape::Block);trail.set_destination(rect[0],rect[1],rect[2],rect[3]);trail.snap_to_destination(rect[2],rect[3]);*last=Instant::now();
   }});Value::Null
  },
  "keyboard.cursor.dispose"=>{CURSORS.with(|cursors|cursors.borrow_mut().remove(&(v["id"].as_u64().unwrap_or(0) as u32)));Value::Null},
  "keyboard.cursor.frame"=>{
   let rect=[n("x",0.0),n("y",0.0),n("width",0.0),n("height",0.0)];let scale=n("scale",1.0).clamp(0.5,3.0);
   let target=match v["kind"].as_str(){
    Some("rect")=>rect,
    Some("workspace")=>neoism_ui::widgets::island::keyboard_cursor_rect(rect,scale),
    Some("tree")=>neoism_ui::panels::file_tree::keyboard_cursor_rect(rect,n("rowY",rect[1]),n("rowHeight",26.0*scale),scale),
    _=>neoism_ui::panels::buffer_tabs::keyboard_cursor_rect(rect[0],rect[1],rect[3],n("padding",12.0*scale),scale,v["plus"].as_bool().unwrap_or(false)),
   };
   CURSORS.with(|cursors|{let mut cursors=cursors.borrow_mut();let Some((trail,last))=cursors.get_mut(&(v["id"].as_u64().unwrap_or(0) as u32))else{return json!({"error":"Unknown keyboard cursor"});};
    let was_animating=trail.is_animating();trail.set_cursor_shape(neoism_terminal_core::ansi::CursorShape::Block);trail.set_destination(target[0],target[1],target[2],target[3]);let now=Instant::now();trail.animate(target[2],target[3],if was_animating{now.duration_since(*last).as_secs_f32()}else{0.0});*last=now;
    json!({"rect":target,"quad":trail.quad_points(),"animating":trail.is_animating(),"hold":trail.blink_hold_visible(neoism_ui::editor::optimistic::DEFAULT_BLINK_PERIOD_MS)})
   })
  },_=>return Some(Err(format!("Unknown keyboard policy operation {op}")))
 }))
}
