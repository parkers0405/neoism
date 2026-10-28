//! Host adapter for the original repository review, commit and pull-request
//! views. All layout/hit policy stays in GitDiffView; Git IO stays in daemon.
use neoism_protocol::git::{GitCommitDetail,GitServerMessage};
use neoism_ui::{panels::git_diff_view::{GitDiffView,GitDiffViewAction},primitives::{paint::Paint,paint_record::RecordingPainter,IdeTheme},layout::Rect};
use serde_json::{json,Value};
use std::{cell::RefCell,collections::HashMap,path::{Path,PathBuf}};
struct Host{view:GitDiffView,paint:RecordingPainter,width:f32,height:f32,font_size:f32,font_family:String,theme:String,editor:Option<u32>,mouse:Option<[f32;2]>}
thread_local!{static HOSTS:RefCell<HashMap<u64,Host>>=RefCell::new(HashMap::new());static NEXT:RefCell<u64>=RefCell::new(0xe000_0000);}
fn text<'a>(v:&'a Value,key:&str)->&'a str{v[key].as_str().unwrap_or("")}
fn n(v:&Value,key:&str,default:f32)->f32{v[key].as_f64().map_or(default,|n|n as f32)}
fn action(action:GitDiffViewAction)->Value{match action{
 GitDiffViewAction::PullRequest{root,request}=>json!({"kind":"pull-request","root":root,"request":request}),
 GitDiffViewAction::OpenUrl{url}=>json!({"kind":"open-url","url":url}),
 GitDiffViewAction::OpenSource{root,path,line,column}=>json!({"kind":"open-source","root":root,"path":path,"line":line,"column":column}),
 GitDiffViewAction::ChangeBlock{root,path,block,restore}=>json!({"kind":"block","root":root,"path":path,"block":block,"restore":restore}),
 GitDiffViewAction::Stage{root,path,unstage}=>json!({"kind":"stage","root":root,"path":path,"unstage":unstage}),
 GitDiffViewAction::Restore{root,path}=>json!({"kind":"restore","root":root,"path":path}),
 GitDiffViewAction::StageAll{root}=>json!({"kind":"stage-all","root":root}),
 GitDiffViewAction::Commit{root}=>json!({"kind":"commit","root":root})}}
impl Host{
 fn frame(&mut self,engines:&mut super::Engines)->Value{
  self.paint.begin_frame();let theme=IdeTheme::by_name(&self.theme);let font=self.paint.font_id_for_family(&self.font_family).unwrap_or(0);
  let editor=self.editor.and_then(|id|engines.editors.get_mut(&id));
  self.view.render_with_editor(&mut self.paint,Rect::new(0.0,0.0,self.width,self.height),font,self.font_size,&theme,editor.map(|e|&mut e.pane));
  let mut frame=self.view.host_frame();frame["commands"]=json!(self.paint.commands());frame["animating"]=json!(self.view.needs_redraw());
  frame["actions"]=json!(self.view.take_actions().into_iter().map(action).collect::<Vec<_>>());
  frame["copy"]=json!(self.view.take_commit_copy());
  frame["edit"]=self.view.take_edit_request().map_or(Value::Null,|edit|json!({"root":edit.root,"path":edit.path,"line":edit.line,"column":edit.column}));
  frame["pointer"]=json!(self.mouse.is_some_and(|[x,y]|self.view.pointer_cursor(x,y)));frame["textPointer"]=json!(self.mouse.is_some_and(|[x,y]|self.view.text_cursor(x,y)));
  if let Some(editor)=self.editor.and_then(|id|engines.editors.get_mut(&id)){
   let visible=self.view.editing_path().is_some();let rect=editor.pane.cursor_rect;
   let dt=editor.last_frame.elapsed().as_secs_f32();editor.last_frame=std::time::Instant::now();
   if visible{if let Some(r)=rect{let moving=editor.trail.is_animating();editor.trail.set_destination(r[0],r[1],r[2],r[3]);editor.trail.animate(r[2],r[3],if moving{dt}else{0.0});}}
   frame["cursorQuad"]=if visible{json!(editor.trail.quad_points())}else{Value::Null};
   frame["cursorToken"]=json!([editor.pane.buffer.cursor_line,editor.pane.buffer.cursor_col]);
   frame["cursorShape"]=json!(format!("{:?}",editor.pane.buffer.mode));frame["dirty"]=json!(editor.pane.is_dirty());
   frame["animating"]=json!(self.view.needs_redraw()||visible&&editor.trail.is_animating());
   frame["clipboard"]=json!(editor.clipboard.take());frame["hostAction"]=json!(editor.host_action.take());
  }
  frame
 }
 fn reply(&mut self,value:&Value)->Result<(),String>{
  let reply:GitServerMessage=serde_json::from_value(value.clone()).map_err(|e|e.to_string())?;
  match reply{
   GitServerMessage::RepoStatus{snapshot,..}=>{if !self.view.is_read_only(){self.view.set_files(snapshot.files.into_iter().map(super::git_panel::file_change).collect());if let Some(error)=snapshot.error{self.view.set_error(error);}}},
   GitServerMessage::ChangedFiles{files,error,..}=>{if !self.view.is_read_only(){self.view.set_files(files.into_iter().map(super::git_panel::file_change).collect());if let Some(error)=error{self.view.set_error(error);}}},
   GitServerMessage::FileDiffs{diffs}=>{if !self.view.is_read_only(){for diff in diffs{let mut lines=vec![];neoism_ui::panels::git_diff::parse_diff_into_with_headers(diff.patch.as_bytes(),&mut lines);self.view.set_file_lines(Path::new(&diff.path),lines);}}},
   GitServerMessage::BaseFile{path,text}=>self.view.set_base_text(Path::new(&path),text.into()),
   GitServerMessage::CommitDetail{detail}=>{let root=self.view.root.clone();self.view=GitDiffView::from_commit(root,detail);},
   GitServerMessage::PullRequest{reply}=>self.view.apply_pull_request_reply(&reply),
   GitServerMessage::Error{message}=>self.view.set_error(message),_=>{}
  }Ok(())
 }
}
fn make_view(v:&Value)->Result<GitDiffView,String>{let root=PathBuf::from(text(v,"root"));let detail=&v["detail"];Ok(match text(v,"mode"){
 "commit"=>match serde_json::from_value::<GitCommitDetail>(detail.clone()){Ok(detail)=>GitDiffView::from_commit(root,detail),Err(_) =>{let mut view=GitDiffView::new(root,PathBuf::new());view.commit_id=Some(text(v,"revision").to_owned());view.set_files(vec![]);view}},
 "pull-request"=>GitDiffView::from_pull_request(root,serde_json::from_value(detail.clone()).map_err(|e|format!("Invalid pull request detail: {e}"))?),
 "pull-request-inbox"=>GitDiffView::pull_request_inbox(root),
 "pull-request-create"=>GitDiffView::create_pull_request(root,text(detail,"branch").to_owned(),text(detail,"head").to_owned(),text(detail,"defaultBranch").to_owned()),
 _=>GitDiffView::new(root,PathBuf::from(text(v,"path")))})}
pub(super) fn dispatch(v:&Value,engines:&mut super::Engines)->Option<Result<Value,String>>{
 let op=text(v,"op");if !op.starts_with("gitreview."){return None;}
 Some(HOSTS.with(|hosts|{let mut hosts=hosts.borrow_mut();
  if op=="gitreview.new"{let id=NEXT.with(|next|{let mut next=next.borrow_mut();*next+=1;*next});let family=text(v,"fontFamily").to_owned();hosts.insert(id,Host{view:make_view(v)?,paint:RecordingPainter::new(&family,v["fontWeight"].as_u64().unwrap_or(400)as u16,vec![]),width:960.0,height:680.0,font_size:n(v,"fontSize",14.0),font_family:family,theme:text(v,"theme").to_owned(),editor:None,mouse:None});return Ok(json!({"id":id}));}
  let id=v["id"].as_u64().ok_or("Missing Git review ID")?;if op=="gitreview.dispose"{hosts.remove(&id);return Ok(json!(null));}let host=hosts.get_mut(&id).ok_or("Unknown Git review")?;
  match op{
   "gitreview.configure"=>{host.font_family=text(v,"fontFamily").to_owned();host.paint=RecordingPainter::new(&host.font_family,v["fontWeight"].as_u64().unwrap_or(400)as u16,vec![]);host.font_size=n(v,"fontSize",host.font_size);host.theme=text(v,"theme").to_owned();},
   "gitreview.layout"=>{host.width=n(v,"width",host.width).max(1.0);host.height=n(v,"height",host.height).max(1.0);},
   "gitreview.reply"=>host.reply(&v["message"])?,
   "gitreview.focus"=>host.view.set_host_focused(v["focused"].as_bool().unwrap_or(true)),
   "gitreview.loading"=>host.view.set_actions_loading(v["loading"].as_bool().unwrap_or(false)),
   "gitreview.reveal"=>host.view.reveal_file(Path::new(text(v,"path"))),
   "gitreview.attach"=>{let id=v["editorID"].as_u64().ok_or("Missing editor ID")? as u32;let editor=engines.editors.get_mut(&id).ok_or("Unknown source editor")?;
    let expected=host.view.document_path().ok_or("Review has no editable source")?;
    if editor.pane.path!=expected{return Err("Canonical editor belongs to another review source".into());}
    editor.pane.buffer.set_cursor_position(v["line"].as_u64().unwrap_or(0)as usize,v["column"].as_u64().unwrap_or(0)as usize,false);host.editor=Some(id);host.view.set_edit_focus(true);
   },
   "gitreview.scroll"=>{let dx=n(v,"deltaX",0.0);let dy=n(v,"deltaY",0.0);let x=n(v,"x",0.0);let y=n(v,"y",0.0);
    let consumed=host.view.commit_metadata.as_mut().is_some_and(|metadata|metadata.scroll_at(Some((x,y)),dy));if !consumed{host.view.scroll(dy);}if dx!=0.0{host.view.scroll_horizontal_at(x,y,dx);}},
   "gitreview.pointer"=>{let x=n(v,"x",0.0);let y=n(v,"y",0.0);host.mouse=Some([x,y]);match text(v,"kind"){
    "down"=>{host.view.pointer_down(x,y);},
    "move"=>{host.view.pointer_move(x,y);if let Some(editor)=host.editor.and_then(|id|engines.editors.get_mut(&id)){if editor.pane.mouse_selecting{editor.pane.extend_pointer_selection_at(x,y);}}},
    "up"=>{host.view.pointer_up();if let Some(editor)=host.editor.and_then(|id|engines.editors.get_mut(&id)){editor.pane.mouse_selecting=false;}},
    "leave"=>{host.mouse=None;host.view.pointer_leave();},_=>{}
   }},
   "gitreview.select"=>{if let Some(editor)=host.editor.and_then(|id|engines.editors.get_mut(&id)){let(line,col)=editor.pane.geometry.hit_position(&editor.pane.buffer.lines,n(v,"x",0.0),n(v,"y",0.0));editor.pane.begin_pointer_selection(line,col,v["clicks"].as_u64().unwrap_or(1)as usize,v["shift"].as_bool().unwrap_or(false));}},
   "gitreview.key"=>{let key=text(v,"key");let normalized=match key{"up"=>"ArrowUp","down"=>"ArrowDown","left"=>"ArrowLeft","right"=>"ArrowRight","enter"=>"Enter","escape"=>"Escape","tab"=>"Tab","backspace"=>"Backspace","delete"=>"Delete","pageup"=>"PageUp","pagedown"=>"PageDown","home"=>"Home","end"=>"End","space"=>" ",_=>if text(v,"char").is_empty(){key}else{text(v,"char")}};
    if host.view.is_pull_request(){if !host.view.pull_request_key(normalized,v["shift"].as_bool().unwrap_or(false),v["ctrl"].as_bool().unwrap_or(false)||v["cmd"].as_bool().unwrap_or(false)){if !text(v,"char").is_empty(){host.view.pull_request_text_input(text(v,"char"));}else{host.view.key(normalized);}}}
    else if host.view.editing_path().is_some(){if let Some(editor)=host.editor.and_then(|id|engines.editors.get_mut(&id)){editor.key(v);}}
    else{host.view.key(normalized);}
   },
   "gitreview.text"=>{host.view.pull_request_text_input(text(v,"text"));},
   "gitreview.column"=>{host.view.focus_column(v["column"].as_u64().unwrap_or(1)as usize);},
   "gitreview.navigate-file"=>host.view.navigate_file(v["next"].as_bool().unwrap_or(true)),
   "gitreview.frame"=>{},_=>return Err(format!("Unknown Git review operation {op}"))
  }Ok(host.frame(engines))
 }))
}
