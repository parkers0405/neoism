//! Original NEOISM artwork, hover geometry, startup policy and fifteen click effects.
use neoism_ui::{panels::agent_pane::{state::NeoismWordmarkState,view::wordmark::{self,WordmarkState}},primitives::{paint_record::RecordingPainter,look}};
use serde_json::{json,Value};
use std::{cell::RefCell,collections::HashMap,sync::{Arc,atomic::{AtomicU64,Ordering}},time::Instant};
#[derive(Clone)]
struct Image{value:Value,_pixels:Arc<Vec<u8>>}
struct Host{state:NeoismWordmarkState,paint:RecordingPainter,created:Instant,image:Option<Image>,previous:Option<Image>,colors:Vec<u32>,kind:String,reduced:bool}
thread_local!{static HOSTS:RefCell<HashMap<u64,Host>>=RefCell::new(HashMap::new());static IMAGES:RefCell<std::collections::VecDeque<(Vec<u32>,Image)>>=RefCell::new(std::collections::VecDeque::new());}
static NEXT:AtomicU64=AtomicU64::new(0xd000_0000);
static IMAGE:AtomicU64=AtomicU64::new(1);
fn number(v:&Value,k:&str,d:f32)->f32{v[k].as_f64().filter(|n|n.is_finite()).map(|n|n as f32).unwrap_or(d)}
fn color(v:&str)->u32{u32::from_str_radix(v.trim_start_matches('#').get(..6).unwrap_or("e8e8e8"),16).unwrap_or(0xe8e8e8)}
pub fn dispatch(v:&Value)->Option<Result<Value,String>>{
 let op=v["op"].as_str()?;if !op.starts_with("wordmark."){return None;}
 Some(HOSTS.with(|hosts|{let mut hosts=hosts.borrow_mut();
  if op=="wordmark.new"{let id=NEXT.fetch_add(1,Ordering::Relaxed);hosts.insert(id,Host{state:NeoismWordmarkState::default(),paint:RecordingPainter::new("Cascadia Code NF",400,vec![]),created:Instant::now(),image:None,previous:None,colors:vec![],kind:v["kind"].as_str().unwrap_or("agent").into(),reduced:false});return Ok(json!({"id":id}));}
  let id=v["id"].as_u64().ok_or("Missing wordmark host")?;
  if op=="wordmark.dispose"{hosts.remove(&id);return Ok(Value::Null);}
  let host=hosts.get_mut(&id).ok_or("Unknown wordmark host")?;
  if op=="wordmark.replay"{if !host.reduced{host.state.animation.replay();}return Ok(Value::Null);}
  if op=="wordmark.cancel"{host.state.animation.cancel();return Ok(Value::Null);}
  if op!="wordmark.frame"{return Err("Unknown wordmark operation".into());}
  let width=number(v,"width",0.0).max(0.0);let height=number(v,"height",0.0).max(0.0);
  if width<=0.0||height<=0.0{return Ok(json!({"letters":[],"commands":[],"animating":false,"showOriginal":true}));}
  host.reduced=v["reducedMotion"].as_bool().unwrap_or(false);
  host.state.auto_animate=v["autoAnimate"].as_bool().unwrap_or(false)&&!host.reduced;
  host.state.play_startup_animation();if host.reduced{host.state.animation.cancel();}
  let mut colors:Vec<u32>=v["colors"].as_array().into_iter().flatten().filter_map(Value::as_str).map(color).collect();
  if colors.is_empty(){colors=look::wordmark_colors_or(color(v["fg"].as_str().unwrap_or("#e8e8e8")));}
  if host.image.is_none()||host.colors!=colors{
   let image=IMAGES.with(|cache|{let mut cache=cache.borrow_mut();
    if let Some(index)=cache.iter().position(|(key,_)|*key==colors){let item=cache.remove(index).unwrap();let image=item.1.clone();cache.push_back(item);return Ok(image);}
    let (w,h,pixels)=wordmark::wordmark_rgba(&colors).ok_or("Unable to decode original wordmark")?;
    let pixels=Arc::new(pixels);let value=json!({"id":IMAGE.fetch_add(1,Ordering::Relaxed),"ptr":pixels.as_ptr() as usize,"len":pixels.len(),"width":w,"height":h});
    let image=Image{value,_pixels:pixels};cache.push_back((colors.clone(),image.clone()));while cache.len()>8{cache.pop_front();}Ok::<_,String>(image)
   })?;
   host.previous=host.image.take();host.image=Some(image);host.colors=colors.clone();
  }
  let elapsed=number(v,"seconds",host.created.elapsed().as_secs_f32());
  let dt=host.state.frame_delta_seconds();let mouse=v["mouse"].as_array().filter(|p|p.len()==2).and_then(|p|Some((p[0].as_f64()? as f32,p[1].as_f64()? as f32)));
  let frame=host.state.animation.frame();let mut letters=Vec::new();host.paint.begin_frame();
  let clip=v["clip"].as_array().filter(|a|a.len()==4).and_then(|a|Some([a[0].as_f64()? as f32,a[1].as_f64()? as f32,a[2].as_f64()? as f32,a[3].as_f64()? as f32])).unwrap_or([-width,-height*4.0,width*3.0,height*9.0]);
  for i in 0..6{
   let target=mouse.is_some_and(|(x,y)|x>=i as f32*width/6.0&&x<=(i+1)as f32*width/6.0&&y>=0.0&&y<=height)as u8 as f32;
   let hover=host.state.hover_mut();hover[i]=if host.reduced{0.0}else{hover[i]+(target-hover[i])*(1.0-(-14.0*dt).exp())};let hover=hover[i].clamp(0.0,1.0);
   let rect=if host.reduced{[i as f32*width/6.0,0.0,width/6.0,height]}else{wordmark::letter_rect([0.0,0.0,width,height],i,hover,elapsed)};
   let glow=if host.kind=="splash"{let breath=((elapsed/4.6*std::f32::consts::TAU).sin()*0.5+0.5).clamp(0.0,1.0);1.0+0.05*(0.05+0.13*breath+hover*0.4)}else{1.04+hover*0.05};
   let [x,y,w,h]=rect;letters.push(json!({"rect":rect,"glow":[x+w*(1.0-glow)*0.5,y+h*(1.0-glow)*0.5,w*glow,h*glow]}));
   if let Some(frame)=frame.as_ref(){let c=colors[i%colors.len()];frame.draw_letter(&mut host.paint,i,rect,[((c>>16)&255)as f32/255.0,((c>>8)&255)as f32/255.0,(c&255)as f32/255.0,1.0],clip,&[],0.0,0);}
  }
  Ok(json!({"image":host.image.as_ref().map(|image|&image.value),"letters":letters,"commands":host.paint.commands(),"showOriginal":frame.as_ref().is_none_or(|frame|frame.show_original()),"effect":frame.as_ref().map(|frame|format!("{:?}",frame.effect)),"progress":frame.map(|frame|frame.progress),"animating":!host.reduced,"clickAnimating":host.state.animation.is_active()}))
 }))
}
