//! Canonical Agent input geometry, footer paint, slash planning and effects.
use neoism_ui::{panels::agent_pane::{command_controller,view::{self,composer_geometry::ComposerGeometry,fx::{self,AgentFxKind},layout::{self,AgentPaneInput},user_input::{self,host_footer::{self,FooterSnapshot},AgentStreamingStatus}}},primitives::{ide_theme::IdeTheme,paint_record::RecordingPainter}};
use serde_json::{json,Value};
use std::{cell::RefCell,collections::HashMap,time::Instant};
struct Host {paint:RecordingPainter,effects:RecordingPainter,fx:Option<(AgentFxKind,Instant,bool)>,chip_loading:bool,chip_hydration:neoism_ui::panels::agent_pane::state::ConfigChipHydration,chip_label:Option<String>,chip_changed:Option<Instant>}
thread_local! {static HOSTS:RefCell<HashMap<u64,Host>>=RefCell::new(HashMap::new());static NEXT:RefCell<u64>=RefCell::new(0xb000_0000);}
fn n(v:&Value,k:&str,d:f32)->f32{v[k].as_f64().map(|n|n as f32).unwrap_or(d)}
fn text<'a>(v:&'a Value,k:&str)->&'a str{v[k].as_str().unwrap_or("")}
fn status(value:&str)->AgentStreamingStatus {match value.to_lowercase().as_str(){"thinking"=>AgentStreamingStatus::Thinking,"working"=>AgentStreamingStatus::Working,"generating"=>AgentStreamingStatus::Generating,"compacting"=>AgentStreamingStatus::Compacting,"waiting-subagents"|"waitingsubagents"=>AgentStreamingStatus::WaitingSubagents,"background-tasks"|"backgroundtasks"=>AgentStreamingStatus::BackgroundTasks,"retrying"=>AgentStreamingStatus::Retrying,_=>AgentStreamingStatus::Idle}}
fn effect(value:&str)->Result<AgentFxKind,String>{Ok(match value.to_lowercase().as_str(){"piss"=>AgentFxKind::Piss,"cuss"=>AgentFxKind::Cuss,"glitch"=>AgentFxKind::Glitch,"disco"=>AgentFxKind::Disco,"gangfight"=>AgentFxKind::GangFight,"praise"=>AgentFxKind::Praise,_=>return Err("Unknown Agent effect".into())})}
struct Input<'a>{text:&'a str,help:bool,footer:bool,images:usize,rows:usize}
impl AgentPaneInput for Input<'_>{fn input(&self)->&str{self.text}fn input_help_visible(&self)->bool{self.help}fn input_footer_visible(&self)->bool{self.footer}fn background_task_details_expanded(&self)->bool{false}fn input_image_count(&self)->usize{self.images}fn input_visual_row_count(&self)->Option<usize>{Some(self.rows)}}
pub fn dispatch(v:&Value)->Option<Result<Value,String>>{
 let op=v["op"].as_str()?;if !op.starts_with("agentcomposer."){return None;}
 Some((||{
  if op=="agentcomposer.slash-options"{return Ok(Value::Array(command_controller::slash_options().into_iter().map(|row|json!({"title":row.title,"description":row.description,"footer":row.footer,"value":row.value})).collect()));}
  if op=="agentcomposer.defaults"{
   use neoism_ui::panels::agent_pane::api_mapping::{config_defaults_from_json,default_model_from_providers_json};
   let mut defaults=config_defaults_from_json(&v["config"]);
   if defaults.model.is_none(){defaults.model=default_model_from_providers_json(&v["providers"]);}
   return Ok(json!({"agent":defaults.agent,"model":defaults.model,"thinking":defaults.thinking}));
  }
  if op=="agentcomposer.normalize-thinking"{return Ok(json!(neoism_ui::panels::agent_pane::api_mapping::normalize_thinking(text(v,"value"))));}
  if op=="agentcomposer.caret-rect"{return Ok(json!(user_input::prompt_caret_rect_for_host(n(v,"x",0.0),n(v,"y",0.0),n(v,"remainingWidth",0.0),n(v,"fontSize",16.0),n(v,"scale",1.0))));}
  if op=="agentcomposer.plan"{return serde_json::to_value(command_controller::plan_slash_command(text(v,"text"))).map_err(|e|e.to_string());}
  HOSTS.with(|hosts|{let mut hosts=hosts.borrow_mut();
   if op=="agentcomposer.new"{
    let id=NEXT.with(|next|{let mut next=next.borrow_mut();*next+=1;*next});let family=v["fontFamily"].as_str().unwrap_or("Cascadia Code NF");let weight=v["fontWeight"].as_u64().unwrap_or(400)as u16;
    hosts.insert(id,Host{paint:RecordingPainter::new(family,weight,vec!["Symbols Nerd Font Mono".into()]),effects:RecordingPainter::new(family,weight,vec!["Symbols Nerd Font Mono".into()]),fx:None,chip_loading:false,chip_hydration:Default::default(),chip_label:None,chip_changed:None});return Ok(json!({"id":id}));
   }
   let id=v["id"].as_u64().ok_or("Missing Agent composer id")?;
   if op=="agentcomposer.dispose"{hosts.remove(&id);return Ok(Value::Null);}
   let host=hosts.get_mut(&id).ok_or("Unknown Agent composer")?;
   let scale=n(v,"scale",1.0).clamp(0.5,3.0);let width=n(v,"width",800.0).max(0.0);let height=n(v,"height",600.0).max(0.0);let theme=IdeTheme::by_name(v["theme"].as_str().unwrap_or("pastel_dark"));
   if op=="agentcomposer.chip-frame"{
    let label=text(v,"label");let kind=text(v,"kind");let loading=v["loading"].as_bool().unwrap_or(false);
    if loading&&!host.chip_loading{host.chip_hydration.begin();}
    if !loading&&host.chip_loading{host.chip_hydration.complete();}
    host.chip_loading=loading;
    if host.chip_label.as_deref().is_some_and(|previous|previous!=label)&&kind=="agent"{host.chip_changed=Some(Instant::now());}
    host.chip_label=Some(label.to_owned());
    let reduced=v["reducedMotion"].as_bool().unwrap_or(false);
    let transition=if reduced{None}else{host.chip_hydration.transition()};
    let elapsed=if reduced{None}else{host.chip_changed.map(|start|start.elapsed().as_secs_f32()*1000.0).filter(|ms|*ms<320.0)};
    host.paint.begin_frame();let font_size=n(v,"fontSize",14.0*scale);
    let width=user_input::render_status_chip_label_for_host(&mut host.paint,label,kind,font_size,&theme,transition,elapsed);
    return Ok(json!({"commands":host.paint.commands(),"width":width,"height":font_size*1.3,"animating":transition.is_some()||elapsed.is_some(),"phase":match transition{Some(neoism_ui::panels::agent_pane::state::ConfigChipTransition::Loading{..})=>"loading",Some(_)=>"settling",None if elapsed.is_some()=>"changing",_=>"idle"}}));
   }
   if op=="agentcomposer.fx.start"{host.fx=Some((effect(text(v,"kind"))?,Instant::now(),false));return Ok(Value::Null);}
   if op=="agentcomposer.fx.frame"{
    host.effects.begin_frame();let mut prompt=None;let mut animating=false;
    if let Some((kind,start,sent))=host.fx.as_mut(){let elapsed=v["elapsed"].as_f64().map(|n|n as f32).unwrap_or_else(||start.elapsed().as_secs_f32());animating=elapsed<fx::total_seconds(*kind);if elapsed>=fx::prompt_at(*kind)&&!*sent{prompt=Some(fx::followup_prompt(*kind));*sent=true;}if animating{let input_y=n(v,"inputY",height*0.46);let rect=fx::scene_rect([0.0,0.0,width,height],[0.0,input_y,width,0.0],scale);fx::render(*kind,&mut host.effects,rect,elapsed,scale,&theme);}}
    if !animating{host.fx=None;}
    return Ok(json!({"commands":host.effects.commands(),"prompt":prompt,"animating":animating}));
   }
   if op!="agentcomposer.frame"{return Err(format!("Unknown Agent composer operation {op}"));}
   let home=v["home"].as_bool().unwrap_or(true);let help=v["help"].as_bool().unwrap_or(true);let cwd=v["cwd"].as_str().filter(|s|!s.is_empty()).map(str::to_owned);let branch=v["branch"].as_str().filter(|s|!s.is_empty()).map(str::to_owned);
   let system=v["system"].as_str().filter(|s|!s.is_empty()).map(|label|neoism_ui::panels::agent_pane::composer_interactions::ComposerSystemControl{label:label.into(),selectable:v["systemSelectable"].as_bool().unwrap_or(false)});
   let footer_visible=help||cwd.is_some()||system.is_some();let images=v["images"].as_u64().unwrap_or(0)as usize;let input_text=text(v,"text");
   let card_width=if home{layout::home_input_width([0.0,0.0,width,height],scale)}else{layout::chat_column([0.0,0.0,width,height],scale).1};
   let rows=user_input::measure_prompt_rows_for_host(&mut host.paint,input_text,card_width,scale).len();
   let input=Input{text:input_text,help,footer:footer_visible,images,rows};let rect=if home{layout::home_input_rect(&input,[0.0,0.0,width,height],scale)}else{layout::chat_input_rect(&input,[0.0,0.0,width,height],scale)};
   let geometry=ComposerGeometry::new([0.0,0.0,rect[2],rect[3]],scale,n(v,"deviceScale",1.0),footer_visible);
   let mut snapshot=FooterSnapshot{help_visible:help,no_providers:v["noProviders"].as_bool().unwrap_or(false),streaming:status(text(v,"state")),system,interactions:Default::default()};
   host.paint.begin_frame();let checkout=view::AgentCheckoutContext::new(cwd.map(|cwd|neoism_ui::panels::agent_pane::attachment_policy::compact_directory_label(&cwd)),branch);
   if let Some(tray)=geometry.tray{host_footer::render(&mut host.paint,&mut snapshot,tray,width,&theme,scale,home,!input_text.trim().is_empty(),v["busy"].as_bool().unwrap_or(false),n(v,"seconds",0.0),Some(&checkout));}
   let controls=host_footer::controls(geometry.card,scale,home,!input_text.trim().is_empty(),v["busy"].as_bool().unwrap_or(false),images);
   let logo=view::home::wordmark_rect([0.0,0.0,width,height],rect,scale);
   Ok(json!({"inputRect":rect,"logoRect":logo,"cardRect":geometry.card,"trayRect":geometry.tray,"cardRadius":geometry.card_radius,"joinRect":geometry.tray_join_rect((neoism_ui::panels::file_tree::FRAME_STROKE*0.5*scale).max(1.0)),"trayRadius":geometry.tray_radius,"footerVisible":footer_visible,"controls":controls,"visualRows":rows,"commands":host.paint.commands(),"animating":footer_visible&&snapshot.streaming!=AgentStreamingStatus::Idle,"cwdRects":snapshot.interactions.cwd_rects,"systemRects":snapshot.interactions.system_rects}))
  })
 })())
}
