//! Original FileTree typography flattened once per row into a retained image.
use neoism_ui::{panels::file_tree::{ROW_HEIGHT,FONT_SIZE,ICON_FONT_SIZE,INDENT_PX,ICON_GAP,GitStatus,FileTree,TreeEntry,NodeKind},primitives::{paint::Paint,paint_record::{RecordingPainter,PaintCommand},text::{draw_icon_centered_with_occlusion,truncate_to_fit}}};
use serde_json::{json,Value};
use std::{cell::RefCell,collections::{HashMap,VecDeque},sync::{Arc,atomic::{AtomicU64,Ordering}}};
use sugarloaf::text::DrawOpts;
struct Raster{value:Value,pixels:Arc<Vec<u8>>}
struct Host{paint:RecordingPainter,cache:HashMap<String,Raster>,order:VecDeque<String>,bytes:usize}
thread_local!{static HOSTS:RefCell<HashMap<u64,Host>>=RefCell::new(HashMap::new());}
static NEXT_HOST:AtomicU64=AtomicU64::new(0xa000_0000);
static NEXT_RASTER:AtomicU64=AtomicU64::new(1);
fn color(v:&Value,key:&str)->[u8;4]{let s=v[key].as_str().unwrap_or("#eeeeee").trim_start_matches('#');let mut out=[238,238,238,255];if s.len()==6||s.len()==8{for(i,pair)in s.as_bytes().chunks_exact(2).enumerate(){out[i]=std::str::from_utf8(pair).ok().and_then(|s|u8::from_str_radix(s,16).ok()).unwrap_or(255);}}out}
fn git_status(value:&str)->GitStatus{match value{ "Modified"=>GitStatus::Modified,"Staged"|"StagedModified"=>GitStatus::StagedModified,"Mixed"=>GitStatus::Mixed,"Added"=>GitStatus::Added,"Deleted"=>GitStatus::Deleted,"Renamed"=>GitStatus::Renamed,"Untracked"=>GitStatus::Untracked,"Conflict"|"Conflicted"=>GitStatus::Conflict,_=>GitStatus::None}}
fn git_projection(host:&mut Host,v:&Value)->Result<Value,String>{
 use neoism_protocol::{git::GitRepoStatus,host_path::HostPath};
 let snapshot:GitRepoStatus=serde_json::from_value(v["snapshot"].clone()).map_err(|error|error.to_string())?;
 let root=HostPath::new(&snapshot.workspace_root);let mut tree=FileTree::new(snapshot.workspace_root.clone().into());
 let mut paths=std::collections::BTreeMap::<String,bool>::new();
 if let Some(repo)=&snapshot.repo_root{let repo=HostPath::new(repo);for file in &snapshot.files{let absolute=repo.join(&file.path);let Some(relative)=root.relative(absolute.as_str()).filter(|path|!path.is_empty())else{continue;};let mut prefix=relative.as_str();let mut directory=false;loop{paths.entry(prefix.to_owned()).and_modify(|old|*old|=directory).or_insert(directory);let Some((parent,_))=prefix.rsplit_once('/')else{break;};prefix=parent;directory=true;}}}
 tree.host_set_entries(paths.iter().map(|(relative,directory)|TreeEntry{label:relative.clone(),depth:0,kind:if *directory{NodeKind::Dir{open:false}}else{NodeKind::File},path:Some(root.join(relative).as_str().into()),git_status:GitStatus::None,virtual_kind:None}).collect());
 tree.apply_host_git_snapshot(&snapshot);
 let theme=neoism_ui::primitives::ide_theme::IdeTheme::by_name(v["theme"].as_str().unwrap_or("pastel_dark"));let scale=v["scale"].as_f64().unwrap_or(1.0).clamp(0.5,3.0)as f32;
 let rows:Vec<_>=tree.entries().iter().filter(|entry|entry.git_status!=GitStatus::None).map(|entry|{let marker=entry.git_status.marker();let marker_width=marker.map(|marker|host.paint.text_mut().measure(marker,&DrawOpts{font_size:FONT_SIZE*scale*0.9,bold:true,..Default::default()})).unwrap_or(0.0);json!({"path":entry.label,"directory":matches!(entry.kind,NodeKind::Dir{..}),"status":format!("{:?}",entry.git_status),"marker":marker,"color":entry.git_status.color(&theme),"markerWidth":marker_width,"trailingWidth":if marker.is_some(){marker_width+8.0*scale}else{0.0}})}).collect();
 Ok(json!(rows))
}
fn draw(host:&mut Host,v:&Value)->Value{
 let scale=v["scale"].as_f64().unwrap_or(1.0).clamp(0.5,3.0) as f32;
 let width=v["width"].as_f64().unwrap_or(0.0).clamp(0.0,4096.0) as f32;
 let name=v["name"].as_str().unwrap_or("");let icon=v["icon"].as_str().unwrap_or("");let chevron=v["chevron"].as_str().unwrap_or("");
 let key=json!([scale,width,name,icon,chevron,v["labelColor"],v["iconColor"],v["chevronColor"],v["gitStatus"],v["directory"],v["theme"],v["trailingWidth"]]).to_string();
 if let Some(raster)=host.cache.get(&key){return raster.value.clone();}
 host.paint.begin_frame();let row=ROW_HEIGHT*scale;let font=FONT_SIZE*scale;let indent=INDENT_PX*scale;let icon_size=ICON_FONT_SIZE*scale;
 let label_x=indent+icon_size+ICON_GAP*scale;
 let status=git_status(v["gitStatus"].as_str().unwrap_or(""));let theme=neoism_ui::primitives::ide_theme::IdeTheme::by_name(v["theme"].as_str().unwrap_or("pastel_dark"));
 let marker=status.marker();let git_opts=DrawOpts{font_size:font*0.9,color:status.color(&theme),bold:true,..Default::default()};
 let marker_width=marker.map(|marker|host.paint.text_mut().measure(marker,&git_opts)).unwrap_or(0.0);let git_gap=if marker.is_some(){8.0*scale}else{0.0};let trailing=v["trailingWidth"].as_f64().unwrap_or(0.0).max(0.0)as f32;
 let label_color=if status==GitStatus::None{color(v,"labelColor")}else{status.color(&theme)};
 let icon_color=if status!=GitStatus::None&&v["directory"].as_bool().unwrap_or(false){status.color(&theme)}else{color(v,"iconColor")};
 let label_opts=DrawOpts{font_size:font,color:label_color,..Default::default()};
 let label=truncate_to_fit(name,(width-label_x-4.0*scale-marker_width-git_gap-trailing).max(0.0),&mut host.paint,&label_opts);
 if !chevron.is_empty(){draw_icon_centered_with_occlusion(&mut host.paint,0.0,[0.0,0.0,(indent-4.0*scale).max(1.0),row],chevron,&DrawOpts{font_size:(indent-6.0*scale).max(6.0*scale),color:color(v,"chevronColor"),..Default::default()},&[],true);}
 draw_icon_centered_with_occlusion(&mut host.paint,indent,[indent,0.0,icon_size,row],icon,&DrawOpts{font_size:icon_size,color:icon_color,..Default::default()},&[],true);
 host.paint.text_mut().draw(label_x,(row-font)/2.0,&label,&label_opts);
 let marker_x=(width-4.0*scale-marker_width).max(0.0);if let Some(marker)=marker{host.paint.text_mut().draw(marker_x,(row-font)/2.0,marker,&git_opts);}
 let w=(width*2.0).ceil() as usize;let h=(row*2.0).ceil() as usize;if w==0||h==0{return Value::Null;}
 let mut canvas=vec![[0.0f32;4];w*h];
 for command in host.paint.commands(){if let PaintCommand::Text{x,y,raster:Some(r),..}=command{
  let left=((x+r.dx)*2.0).round() as i32;let top=((y+r.dy)*2.0).round() as i32;let source=r.pixels();
  for sy in 0..r.height as usize{let dy=top+sy as i32;if dy<0||dy>=h as i32{continue;}for sx in 0..r.width as usize{let dx=left+sx as i32;if dx<0||dx>=w as i32{continue;}let at=(sy*r.width as usize+sx)*4;let alpha=source[at+3] as f32/255.0;if alpha==0.0{continue;}let dest=&mut canvas[dy as usize*w+dx as usize];for c in 0..3{dest[c]=source[at+c] as f32/255.0*alpha+dest[c]*(1.0-alpha);}dest[3]=alpha+dest[3]*(1.0-alpha);}}
 }}
 let mut pixels=Vec::with_capacity(w*h*4);for p in canvas{for c in p.iter().take(3){pixels.push(if p[3]>0.0{(c/p[3]*255.0).round().clamp(0.0,255.0) as u8}else{0});}pixels.push((p[3]*255.0).round().clamp(0.0,255.0) as u8);}
 let pixels=Arc::new(pixels);let value=json!({"raster":{"id":NEXT_RASTER.fetch_add(1,Ordering::Relaxed),"ptr":pixels.as_ptr() as usize,"len":pixels.len(),"width":w,"height":h,"scale":2},"label":label,"labelX":label_x,"labelY":(row-font)/2.0,"iconX":indent,"rowHeight":row,"gitMarker":marker,"gitColor":status.color(&theme),"gitMarkerX":marker_x,"gitMarkerWidth":marker_width,"gitTrailingWidth":marker_width+git_gap,"labelColor":label_color,"iconColor":icon_color});
 host.bytes+=pixels.len();host.cache.insert(key.clone(),Raster{value:value.clone(),pixels});host.order.push_back(key);
 while host.bytes>64*1024*1024||host.cache.len()>1024{let Some(key)=host.order.pop_front()else{break;};if let Some(old)=host.cache.remove(&key){host.bytes-=old.pixels.len();}}
 value
}
pub fn dispatch(v:&Value)->Option<Result<Value,String>>{
 let op=v["op"].as_str()?;if !op.starts_with("treeglyphs."){return None;}
 Some(HOSTS.with(|store|{let mut hosts=store.borrow_mut();
  if op=="treeglyphs.new"{let id=NEXT_HOST.fetch_add(1,Ordering::Relaxed);let extras=v["fonts"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect();let paint=RecordingPainter::new(v["family"].as_str().unwrap_or("Cascadia Code NF"),v["weight"].as_u64().unwrap_or(400) as u16,extras);hosts.insert(id,Host{paint,cache:HashMap::new(),order:VecDeque::new(),bytes:0});return Ok(json!({"id":id}));}
  let id=v["id"].as_u64().ok_or("Missing tree glyph host")?;
  if op=="treeglyphs.dispose"{hosts.remove(&id);return Ok(json!({}));}
  let host=hosts.get_mut(&id).ok_or("Unknown tree glyph host")?;
  match op{"treeglyphs.git-project"=>git_projection(host,v),"treeglyphs.row"=>Ok(draw(host,v)),"treeglyphs.stats"=>Ok(json!({"rows":host.cache.len(),"bytes":host.bytes})),_=>Err("Unknown tree glyph operation".into())}
 }))
}
