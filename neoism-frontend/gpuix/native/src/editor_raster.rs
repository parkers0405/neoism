//! Cached original-font glyph rows. Scroll, selections and diagnostics remain
//! independent geometry; a row's texture changes only when its glyphs change.
use neoism_ui::primitives::{paint::Paint, paint_record::RecordingPainter};
use serde_json::{json, Value};
use std::{collections::{HashMap, HashSet, VecDeque}, sync::{Arc, atomic::{AtomicU64, Ordering}}};
use neoism_paint::text::{DrawOpts, Text};

const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_ROWS: usize = 1024;
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub struct RowRun { pub text: String, pub x: f32, pub width: f32, pub token: String }
#[derive(Clone)]
struct Raster { value: Value, pixels: Arc<Vec<u8>> }
#[derive(Default)]
pub struct EditorRaster {
    settings: Value,
    text: Option<Text>,
    cache: HashMap<String, Raster>,
    order: VecDeque<String>,
    fallback: HashSet<String>,
    fallback_order: VecDeque<String>,
    bytes: usize,
    // Keep all returned pointers alive through both this frame and the next.
    visible: Vec<Raster>,
    previous: Vec<Raster>,
}
impl EditorRaster {
    pub fn clone_settings(&self) -> Self {
        let mut out = Self::default();
        if self.text.is_some() { out.configure(&self.settings); }
        out
    }
    pub fn configure(&mut self, settings: &Value) {
        let settings=json!({"enabled":settings["enabled"].as_bool().unwrap_or(true),"fontFamily":settings["fontFamily"],"fontWeight":settings["fontWeight"],"fontSize":settings["fontSize"],"colors":settings["colors"]});
        if self.settings==settings && self.text.is_some() { return; }
        if settings["enabled"]==false { self.text=None;self.settings=settings;self.cache.clear();self.order.clear();self.fallback.clear();self.fallback_order.clear();self.bytes=0;return; }
        let painter=RecordingPainter::new(settings["fontFamily"].as_str().unwrap_or("Cascadia Code NF"),settings["fontWeight"].as_u64().unwrap_or(400) as u16,vec![]);
        let mut text=Text::new(painter.font_library()); text.init_cpu(); text.set_scale_factor(2.0);
        self.text=Some(text);self.settings=settings;
        self.cache.clear();self.order.clear();self.fallback.clear();self.fallback_order.clear();self.bytes=0;
    }
    pub fn enabled(&self)->bool { self.text.is_some() }
    pub fn begin_frame(&mut self) { self.previous=std::mem::take(&mut self.visible); }
    pub fn stats(&self)->Value { json!({"rows":self.cache.len(),"bytes":self.bytes,"visible":self.visible.len(),"fallbackRows":self.fallback.len()}) }
    fn remember_fallback(&mut self,key:String) {
        if self.fallback.insert(key.clone()){self.fallback_order.push_back(key);}
        while self.fallback_order.len()>MAX_ROWS {if let Some(key)=self.fallback_order.pop_front(){self.fallback.remove(&key);}}
    }
    pub fn row(&mut self, runs: Vec<RowRun>, row_height:f32, scroll_x:f32)->Option<Value> {
        if self.text.is_none() { return None; }
        // Selection/diagnostic boundaries can split one syntax run. Joining
        // those splits preserves the same row identity as the unselected text.
        let mut merged:Vec<(String,f32,f32,[u8;4],String)>=Vec::new();
        for run in runs {
            let color=parse_color(self.settings["colors"][&run.token].as_str().or(self.settings["colors"]["Plain"].as_str()).unwrap_or("#e8e8e8"));
            let x=run.x;
            if let Some(last)=merged.last_mut().filter(|last| last.4==run.token && (last.1+last.2-x).abs()<0.01) {
                last.0.push_str(&run.text);last.2+=run.width;
            }else{merged.push((run.text,x,run.width,color,run.token));}
        }
        let key=format!("{}:{:?}",row_height.to_bits(),merged.iter().map(|(text,x,_,color,token)|(text,x.to_bits(),color,token)).collect::<Vec<_>>());
        if self.fallback.contains(&key){return None;}
        let raster=if let Some(item)=self.cache.get(&key) { Some(item.clone()) }else{
            let size=self.settings["fontSize"].as_f64().unwrap_or(14.0).clamp(7.0,84.0) as f32;
            let text=self.text.as_mut().unwrap();text.clear();
            let y=((row_height-size*1.2)*0.5).max(0.0);
            for (run,x,_,color,_) in &merged { text.draw(*x,y,run,&DrawOpts{font_size:size,color:*color,..Default::default()}); }
            if text.instances().is_empty() { return Some(Value::Null); }
            let mut bounds=[f32::MAX,f32::MAX,f32::MIN,f32::MIN];
            for glyph in text.instances(){let scale=if glyph.raster_scale>0.0{glyph.raster_scale}else{1.0};let x=glyph.pos[0]+glyph.bearings[0] as f32*scale;let y=glyph.pos[1]+glyph.bearings[1] as f32*scale;bounds[0]=bounds[0].min(x);bounds[1]=bounds[1].min(y);bounds[2]=bounds[2].max(x+glyph.glyph_size[0] as f32*scale);bounds[3]=bounds[3].max(y+glyph.glyph_size[1] as f32*scale);}
            let width=bounds[2].ceil()-bounds[0].floor();let height=bounds[3].ceil()-bounds[1].floor();
            if width>8192.0||height>8192.0||width*height>16.0*1024.0*1024.0{self.remember_fallback(key);return None;}
            let Some(image)=text.host_run_raster(0) else{self.remember_fallback(key);return None;};
            // A long unwrapped row may exceed the host GPU texture limit.
            // Keep the legacy glyph path for that row instead of losing text.
            if image.width>8192 || image.height>8192 { self.remember_fallback(key);return None; }
            Some({
                let pixels=Arc::new(image.rgba);
                let item=Raster{value:json!({"id":NEXT_ID.fetch_add(1,Ordering::Relaxed),"ptr":pixels.as_ptr() as usize,"len":pixels.len(),"width":image.width,"height":image.height,"scale":2,"x":image.left/2.0,"y":image.top/2.0}),pixels};
                self.bytes+=item.pixels.len();self.cache.insert(key.clone(),item.clone());self.order.push_back(key);
                while self.bytes>MAX_BYTES || self.cache.len()>MAX_ROWS { let Some(old)=self.order.pop_front() else{break;};if let Some(old)=self.cache.remove(&old){self.bytes-=old.pixels.len();} }
                item
            })
        };
        match raster { None=>Some(Value::Null),Some(item)=>{
            let mut value=item.value.clone();value["x"]=json!(value["x"].as_f64().unwrap_or(0.0)-scroll_x as f64);
            self.visible.push(item);Some(value)
        }}
    }
}
fn parse_color(value:&str)->[u8;4] {
    let value=value.strip_prefix('#').unwrap_or(value);
    if value.len()!=6 && value.len()!=8 {return [232,232,232,255];}
    let mut out=[0,0,0,255];for (i,part) in value.as_bytes().chunks_exact(2).enumerate(){out[i]=std::str::from_utf8(part).ok().and_then(|s|u8::from_str_radix(s,16).ok()).unwrap_or(255);}out
}
