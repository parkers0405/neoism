//! Original terminal splash menu geometry and paint, without a Sugarloaf window.
use neoism_ui::{panels::{terminal_splash,splash_overlay::{SplashOverlay,SplashInjection}},primitives::{paint_record::RecordingPainter,ide_theme::IdeTheme}};
use serde_json::{json,Value};
use std::{cell::RefCell,collections::HashMap};
struct Host{overlay:SplashOverlay,paint:RecordingPainter}
thread_local!{static HOSTS:RefCell<HashMap<u64,Host>>=RefCell::new(HashMap::new());static NEXT:RefCell<u64>=RefCell::new(0xc000_0000);}
fn n(v:&Value,key:&str,fallback:f32)->f32{v[key].as_f64().map(|n|n as f32).unwrap_or(fallback)}
pub fn dispatch(v:&Value)->Option<Result<Value,String>>{
 let op=v["op"].as_str()?;if !op.starts_with("splash."){return None;}
 Some(HOSTS.with(|hosts|{let mut hosts=hosts.borrow_mut();
  if op=="splash.new"{let id=NEXT.with(|next|{let mut next=next.borrow_mut();*next+=1;*next});hosts.insert(id,Host{overlay:SplashOverlay::new(),paint:RecordingPainter::new(v["fontFamily"].as_str().unwrap_or("Cascadia Code NF"),v["fontWeight"].as_u64().unwrap_or(400)as u16,vec!["Symbols Nerd Font Mono".into()])});return Ok(json!({"id":id}));}
  let id=v["id"].as_u64().ok_or("Missing splash id")?;if op=="splash.dispose"{hosts.remove(&id);return Ok(Value::Null);}
  let host=hosts.get_mut(&id).ok_or("Unknown splash")?;
  if op=="splash.hit"{return Ok(json!({"action":host.overlay.menu_hit(n(v,"x",-1.0),n(v,"y",-1.0)).map(|action|action.as_str())}));}
  if op!="splash.frame"{return Err(format!("Unknown splash operation {op}"));}
  let width=n(v,"width",800.0).max(0.0);let height=n(v,"height",600.0).max(0.0);let cell_h=n(v,"cellHeight",18.2).max(1.0);let rows=v["rows"].as_u64().unwrap_or((height/cell_h).floor()as u64)as usize;let cols=v["cols"].as_u64().unwrap_or(100)as usize;
  host.paint.begin_frame();
  let Some((padding,layout))=terminal_splash::splash_bytes(cols,rows)else{return Ok(json!({"commands":[],"rows":[],"wordmark":[0,0,0,0],"padding":[]}));};
  let injection=SplashInjection{wordmark_row:layout.wordmark_row(),wordmark_cells_h:layout.wordmark_rows,gap_cells_h:layout.gap_rows,menu_cells_h:layout.menu_rows};
  let band_top=injection.wordmark_row as f32*cell_h;let band_h=injection.wordmark_cells_h as f32*cell_h;let logo_w=(band_h*0.96*terminal_splash::WORDMARK_ASPECT).min(width*0.60);let logo_h=logo_w/terminal_splash::WORDMARK_ASPECT;let logo=[(width-logo_w)*0.5,band_top+(band_h-logo_h)*0.5,logo_w,logo_h];
  host.overlay.set_mouse(if v["pointer"].as_bool().unwrap_or(false){Some((n(v,"x",-1.0),n(v,"y",-1.0)))}else{None});
  let elapsed=n(v,"dismissedMs",0.0);let t=(elapsed/420.0).clamp(0.0,1.0);let eased=t*t*(3.0-2.0*t);let scale=1.0-(1.0-0.84)*eased;
  if t<1.0{host.overlay.render_menu_host(&mut host.paint,&injection,(0.0,0.0),(width,height),cell_h,&IdeTheme::by_name(v["theme"].as_str().unwrap_or("pastel_dark")),n(v,"scale",1.0),logo_w*scale,t,eased,1.0-eased,&[]);}
  Ok(json!({"commands":host.paint.commands(),"rows":if t<1.0{host.overlay.host_menu_rows().into_iter().map(|(action,label,key,rect)|json!({"action":action,"label":label,"key":key,"rect":rect})).collect::<Vec<_>>()}else{vec![]},"wordmark":[logo[0]+logo[2]*(1.0-scale)*0.5,logo[1]+logo[3]*(1.0-scale)*0.5-band_h*0.35*eased,logo[2]*scale,logo[3]*scale],"alpha":1.0-eased,"animating":t>0.0&&t<1.0,"padding":padding.as_bytes()}))
 }))
}
