//! Pane-owned, bounded literal UI runtime. No network or outbound-command capability.
use super::*;
use std::{cell::RefCell, ops::Range, sync::Arc};
use serde_json::json;

// Opaque IDs are unique across panes/engines as well as source revisions.
// This counter owns no UI state and cannot grow storage with closed views.
fn next_id()->u64 {
    static NEXT:std::sync::atomic::AtomicU64=std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1,std::sync::atomic::Ordering::Relaxed)
}

#[derive(Clone, Debug)]
pub(super) struct NodeMeasure {
    pub start:f32, pub own_bottom:f32, pub bottom:f32, pub x:f32, pub width:f32, pub items:Range<usize>,
    pub tabs:Vec<[f32;4]>, pub control_width:f32,
}
#[derive(Clone, Debug)]
pub(super) struct Prepared { pub spec:Spec, pub nodes:HashMap<String,NodeMeasure>, pub scale:f32, pub source:String }

#[derive(Clone, Debug)]
struct LocalControl { node:String, tab:Option<usize>, rect:[f32;4], kind:&'static str, label:String, value:Value, options:Vec<String>, placeholder:Option<String>, multiline:bool, disabled:bool, selected:bool, collapsible:bool, points:Vec<(f32,f32)> }
#[derive(Clone)]
struct Target { instance:String, token:u64, control:LocalControl }
struct Instance { token:u64, source:String, namespace:String, session:String, prepared:Arc<Prepared>, values:HashMap<String,Value>, used:u64 }
#[derive(Default)]
pub struct Runtime {
    session:String, instances:HashMap<String,Instance>, visible:HashMap<String,Target>,
    controls:Vec<Value>, clock:u64,
    viewport:[f32;4],
}

fn prop<'a>(n:&'a Element,k:&str)-> &'a str { n.props.get(k).and_then(Value::as_str).unwrap_or("") }
fn flag(n:&Element,k:&str)->bool { n.props.get(k).and_then(Value::as_bool)==Some(true) }
fn initial(n:&Element)->Value { match n.kind.as_str() {
    "Tabs"=>json!(n.props.get("defaultTab").and_then(Value::as_u64).unwrap_or(0)),
    "Collapsible"=>json!(flag(n,"defaultOpen")), "Checkbox"=>json!(flag(n,"defaultChecked")),
    "Select"=>n.props.get("defaultValue").cloned().unwrap_or_else(||n.props["options"][0].clone()),
    "Input"=>json!(prop(n,"defaultValue")), "LineGraph"=>json!(0), "Diagram"=>diagram::Transform::default().value(), _=>json!(""),
} }
fn value(i:&Instance,id:&str)->Value { i.values.get(id).cloned().unwrap_or_else(||initial(&i.prepared.spec.elements[id])) }
fn instance_key(session:&str,namespace:&str,index:usize)->String { json!([session,namespace,index]).to_string() }

impl Runtime {
    /// Must run before each paint; invalidates all previous hit targets.
    pub fn begin_frame(&mut self,session:&str,viewport:[f32;4])->bool {
        let switched=self.session!=session;
        if switched {
            self.session=session.to_owned();
        }
        self.viewport=viewport; self.visible.clear(); self.controls.clear(); self.clock=self.clock.wrapping_add(1);
        switched
    }
    pub fn metadata(&self)->Value {json!({"controls":self.controls})}
    /// Observe whole-message replacements even when the message is offscreen.
    /// Unchanged literal fences retain values; removed/edited sources cannot
    /// regain old state merely by scrolling back into view.
    pub fn observe_source(&mut self,session:&str,namespace:&str,text:&str) {
        let expired:HashSet<_>=self.instances.iter().filter(|(_,i)|i.session==session && i.namespace==namespace && !text.contains(&i.source)).map(|(k,_)|k.clone()).collect();
        self.expire(&expired);
    }
    pub fn retain_messages(&mut self,session:&str,messages:&HashSet<String>) {
        let expired=self.instances.iter().filter(|(_,i)|i.session==session && !messages.contains(&i.namespace)).map(|(k,_)|k.clone()).collect();self.expire(&expired);
    }
    fn expire(&mut self,keys:&HashSet<String>) {
        self.instances.retain(|key,_|!keys.contains(key));
        let stale:HashSet<_>=self.visible.iter().filter(|(_,t)|keys.contains(&t.instance)).map(|(id,_)|id.clone()).collect();
        self.visible.retain(|id,_|!stale.contains(id));self.controls.retain(|v|v["id"].as_str().is_none_or(|id|!stale.contains(id)));
    }
    pub(crate) fn observe_blocks(&mut self,namespace:&str,blocks:&[AssistantMarkdownBlock]) {
        let expired:HashSet<_>=self.instances.iter().filter(|(_,i)| i.session==self.session && i.namespace==namespace).filter_map(|(key,i)| {
            let live=blocks.iter().enumerate().any(|(index,b)|instance_key(&self.session,namespace,index)==*key && matches!(b,AssistantMarkdownBlock::JsonRender(l) if l.prepared.as_ref().is_some_and(|p|p.source==i.source)));
            (!live).then_some(key.clone())
        }).collect();self.expire(&expired);
    }
    fn ensure(&mut self,l:&NativeLayout,namespace:&str,index:usize)->String {
        let key=instance_key(&self.session,namespace,index);
        let p=l.prepared.as_ref().unwrap();
        let replace=self.instances.get(&key).is_none_or(|i| i.source!=p.source);
        if replace {
            if self.instances.contains_key(&key) {self.expire(&HashSet::from([key.clone()]));}
            if self.instances.len()>=128 { if let Some(old)=self.instances.iter().min_by_key(|(_,i)|i.used).map(|(k,_)|k.clone()) {
                self.expire(&HashSet::from([old]));
            }}
            self.instances.insert(key.clone(),Instance{token:next_id(),source:p.source.clone(),namespace:namespace.to_owned(),session:self.session.clone(),prepared:p.clone(),values:HashMap::new(),used:self.clock});
        }
        let i=self.instances.get_mut(&key).unwrap(); i.used=self.clock; i.prepared=p.clone();
        key
    }
    /// Only local changes to live paint-registered controls are accepted.
    pub fn event(&mut self,id:&str,kind:&str,v:Option<&Value>)->Result<(),String> {
        let t=self.visible.get(id).cloned().ok_or("Stale or invisible control")?;
        if t.control.disabled {return Err("Control disabled".into());}
        let i=self.instances.get_mut(&t.instance).ok_or("Expired instance")?;
        if i.token!=t.token {return Err("Expired source".into());}
        let c=&t.control;
        if kind=="activate" && c.kind=="button" && v.is_some() {return Err("Button activation does not accept a value".into());}
        let n=&i.prepared.spec.elements[&c.node];
        match (kind,c.kind) {
            ("activate","diagram")=>{
                let next=diagram::Transform::from_value(&value(i,&c.node)).event(v)?;
                i.values.insert(c.node.clone(),next.value());
            }
            ("change","input"|"select")=>{
                let s=v.and_then(Value::as_str).filter(|s|bounded(s)).ok_or("Invalid string value")?;
                if c.kind=="select" && !c.options.iter().any(|o|o==s) {return Err("Invalid option".into());}
                i.values.insert(c.node.clone(),json!(s));
            }
            ("change","checkbox")=>{ let b=v.and_then(Value::as_bool).ok_or("Expected boolean")?; i.values.insert(c.node.clone(),json!(b)); }
            ("activate","chart")=>{
                let index=match v {None=>value(i,&c.node).as_u64().unwrap_or(0),Some(v)=>v.as_u64().ok_or("Expected point index")?};
                if index>=c.points.len() as u64 {return Err("Point out of range".into());}
                i.values.insert(c.node.clone(),json!(index));
            }
            ("activate","button")=>match n.kind.as_str() {
                "Tabs"=>{i.values.insert(c.node.clone(),json!(c.tab.ok_or("Missing tab")?));}
                "Collapsible"=>{i.values.insert(c.node.clone(),json!(!value(i,&c.node).as_bool().unwrap_or(false)));}
                _=>return Err("Not an activatable control".into()),
            },
            _=>return Err("Invalid event kind or value".into()),
        }
        // Invalidate immediately: hidden descendants must not remain callable
        // between this event and the host's next render.
        self.visible.clear();self.controls.clear(); Ok(())
    }
    pub fn pointer_activate(&mut self,x:f32,y:f32)->bool {
        let found=self.visible.iter().find(|(_,t)| matches!(t.control.kind,"button"|"chart") && contains(t.control.rect,x,y)).map(|(id,t)|{
            let v=if t.control.kind=="chart" {Some(json!(nearest(&t.control.points,x-t.control.rect[0],y-t.control.rect[1])))} else {None}; (id.clone(),v)
        });
        if let Some((id,v))=found {let _=self.event(&id,"activate",v.as_ref());true} else {false}
    }
    pub fn pointer_hint(&self,x:f32,y:f32)->bool {
        self.visible.values().any(|t| !t.control.disabled && matches!(t.control.kind,"button"|"chart") && contains(t.control.rect,x,y))
    }
}
fn contains(r:[f32;4],x:f32,y:f32)->bool {x>=r[0]&&y>=r[1]&&x<=r[0]+r[2]&&y<=r[1]+r[3]}
fn nearest(points:&[(f32,f32)],x:f32,y:f32)->usize {points.iter().enumerate().min_by(|(_,a),(_,b)| ((a.0-x).powi(2)+(a.1-y).powi(2)).total_cmp(&((b.0-x).powi(2)+(b.1-y).powi(2)))).map_or(0,|(i,_)|i)}
fn shifted(item:&Item,dy:f32)->Item {let mut i=item.clone(); match &mut i {Item::Diagram(c)=>c.rect[1]+=dy,Item::Rect{rect,..}=>rect[1]+=dy,Item::Text{y,..}=>*y+=dy,Item::Shape{points,..}=>for p in points {p.1+=dy;}} i}
fn subtree_text(p:&Prepared,id:&str,out:&mut String) {
    let n=&p.spec.elements[id];
    fn literal(v:&Value,out:&mut String) {match v {Value::String(s)=>{out.push_str(s);out.push('\n');},Value::Array(a)=>for v in a {literal(v,out)},Value::Object(o)=>for v in o.values(){literal(v,out)},_=>{}}}
    for (k,v) in &n.props {if k!="prompt" {literal(v,out);}}
    for ch in n.children.iter().flatten() {subtree_text(p,ch,out);}
}
fn reflow(l:&NativeLayout,i:&Instance)->(NativeLayout,Vec<LocalControl>) {
    fn walk(l:&NativeLayout,i:&Instance,id:&str,y:&mut f32,out:&mut NativeLayout,controls:&mut Vec<LocalControl>) {
        let p=&i.prepared; let n=&p.spec.elements[id];let m=&p.nodes[id];let s=p.scale;
        let start=*y;let dy=start-m.start;
        out.items.extend(l.items[m.items.clone()].iter().map(|item|{
            let mut item=shifted(item,dy);
            if let Item::Diagram(c)=&mut item {c.transform=diagram::Transform::from_value(&value(i,id));}
            item
        }));
        *y+=m.own_bottom-m.start;
        let val=value(i,id);
        let kind=match n.kind.as_str(){"Input"|"Filter"=>Some("input"),"Select"=>Some("select"),"Checkbox"=>Some("checkbox"),"Collapsible"=>Some("button"),"LineGraph"=>Some("chart"),_=>None};
        let mut add=|kind:&'static str,label:String,rect:[f32;4],tab:Option<usize>,points:Vec<(f32,f32)>| {
            let options=if kind=="chart" {n.props["data"].as_array().unwrap().iter().map(|v|v["label"].as_str().unwrap().to_owned()).collect()}else{n.props.get("options").and_then(Value::as_array).map(|a|a.iter().map(|v|v.as_str().unwrap().to_owned()).collect()).unwrap_or_default()};
            controls.push(LocalControl{node:id.to_owned(),tab,rect,kind,label,value:val.clone(),options,placeholder:n.props.get("placeholder").and_then(Value::as_str).map(str::to_owned),multiline:flag(n,"multiline"),disabled:flag(n,"disabled"),selected:kind=="chart" && i.values.contains_key(id),collapsible:n.kind=="Collapsible",points});
        };
        if let Some(kind)=kind {
            let label=if matches!(n.kind.as_str(),"Collapsible"|"LineGraph") {prop(n,"title").to_owned()}else{prop(n,"label").to_owned()};
            let h=if n.kind=="Input" && flag(n,"multiline") {80.0*s}else if matches!(kind,"button"|"checkbox") {36.0*s}else{32.0*s};
            let rect=if kind=="chart" {[m.x,start,m.width,(*y-start).max(s)]}else{[m.x,(*y-h).max(start),m.control_width,h]};
            let points=if kind=="chart" {
                // Preserve exact prepared point geometry (the chart's small
                // foreground rectangles), not an approximation of the curve.
                l.items[m.items.clone()].iter().filter_map(|it|match it {Item::Rect{rect,ink:Ink::Foreground|Ink::Custom(_),..}=>Some((rect[0]+rect[2]*0.5-m.x,rect[1]+rect[3]*0.5-m.start)),_=>None}).collect()
            }else{vec![]};
            add(kind,label,rect,None,points);
        }
        if n.kind=="Diagram" {
            let rect=l.items[m.items.clone()].iter().find_map(|it|if let Item::Diagram(c)=it {let mut r=c.rect;r[1]+=dy;Some(r)}else{None}).unwrap();
            add("diagram",n.props.get("title").and_then(Value::as_str).unwrap_or("Diagram").to_owned(),rect,None,vec![]);
        }
        if n.kind=="Tabs" {
            let labels=n.props["labels"].as_array().unwrap();
            // Tab cells share a premeasured row height, including literal
            // wrapping, so hit regions never overlap their sibling labels.
            for (tab,label) in labels.iter().enumerate() {
                let mut rect=m.tabs[tab];rect[1]+=dy;
                add("button",label.as_str().unwrap().to_owned(),rect,Some(tab),vec![]);
            }
        }
        let mut shown=0;
        if let Some(children)=&n.children {for (index,ch) in children.iter().enumerate() {
            if n.kind=="Tabs" && val.as_u64()!=Some(index as u64) {continue;}
            if n.kind=="Collapsible" && val!=true {continue;}
            if n.kind=="Filter" {let query=val.as_str().unwrap_or("").to_lowercase();let mut content=String::new();subtree_text(p,ch,&mut content);if !content.to_lowercase().contains(&query){continue;}}
            if shown>0 || *y>start { *y+=20.0*s; }
            walk(l,i,ch,y,out,controls);
            shown+=1;
        }}
        let _=m.bottom;
    }
    let mut out=NativeLayout{height:0.0,width:l.width,items:vec![],prepared:None};let mut controls=vec![];let mut y=0.0;
    walk(l,i,&i.prepared.spec.root,&mut y,&mut out,&mut controls);
    out.height=y.max(8.0*i.prepared.scale);(out,controls)
}

#[cfg(test)]
mod tests;

pub fn height(l:&NativeLayout,r:Option<&RefCell<Runtime>>,namespace:&str,index:usize)->f32 {
    let Some(r)=r else{return l.height};let mut r=r.borrow_mut();let key=r.ensure(l,namespace,index);reflow(l,&r.instances[&key]).0.height
}

#[allow(clippy::too_many_arguments)]
pub fn render_interactive(paint:&mut dyn crate::primitives::paint::Paint,l:&NativeLayout,r:Option<&RefCell<Runtime>>,namespace:&str,index:usize,x:f32,y:f32,theme:&IdeTheme,s:f32,clip:[f32;4],occlusion:&[[f32;4]],mouse:Option<(f32,f32)>) {
    let Some(r)=r else {super::render(paint,l,x,y,theme,s,clip,occlusion);return;};
    // Release runtime borrow before painting: text/paint callbacks may inspect
    // the pane. All measurements and paint use this same projection.
    let (mut projected,controls,key,token,viewport)={let mut r=r.borrow_mut();let key=r.ensure(l,namespace,index);let i=&r.instances[&key];let (p,c)=reflow(l,i);(p,c,key,i.token,r.viewport)};
    for mut c in controls {
        let local=c.rect;c.rect[0]+=x;c.rect[1]+=y;
        let usable=|bounds:[f32;4]|c.rect[0]>=bounds[0]&&c.rect[1]>=bounds[1]&&c.rect[0]+c.rect[2]<=bounds[0]+bounds[2]&&c.rect[1]+c.rect[3]<=bounds[1]+bounds[3]&&c.rect[2]>0.0&&c.rect[3]>0.0;
        let live=usable(clip)&&usable(viewport)&&!occlusion.iter().any(|o|intersect_rect(c.rect,*o).is_some());
        if c.kind!="chart" && c.kind!="diagram" {
            let selected=c.tab.is_some_and(|t|c.value.as_u64()==Some(t as u64));
            if c.tab.is_some() {
                for item in &mut projected.items {
                    if let Item::Text{x,y,ink,..}=item {
                        if *x>=local[0] && *x<local[0]+local[2] && *y>=local[1] && *y<local[1]+local[3] {
                            *ink=if selected {Ink::Foreground}else{Ink::Muted};
                        }
                    }
                }
                if selected {projected.items.push(Item::Rect{rect:[local[0],local[1]+local[3]-2.0*s,local[2],2.0*s],ink:Ink::Foreground,radius:0.0});}
            }else{
            if matches!(c.kind,"input"|"select") {
                projected.items.push(Item::Rect{rect:local,ink:Ink::Border,radius:6.0*s});
                projected.items.push(Item::Rect{rect:[local[0]+s,local[1]+s,(local[2]-2.0*s).max(0.0),(local[3]-2.0*s).max(0.0)],ink:Ink::Surface,radius:5.0*s});
            }
            let shown=match c.kind {"button" if c.collapsible=>format!("{} {}",if c.value==true {"▾"}else{"▸"},c.label),"button"=>c.label.clone(),"checkbox"=>format!("{} {}",if c.value==true {"☑"}else{"☐"},c.label),_=>c.value.as_str().filter(|v|!v.is_empty()).map(str::to_owned).or_else(||c.placeholder.clone()).unwrap_or_default()};
            let inset=if c.collapsible || c.kind=="checkbox" {0.0}else{10.0*s};
            let top=if c.multiline {10.0*s}else{(local[3]-13.0*s)*0.5};
            projected.items.push(Item::Text{x:local[0]+inset,y:local[1]+top,width:(local[2]-inset).max(s),text:shown,size:13.0,bold:c.kind=="button",ink:if c.disabled {Ink::Muted}else{Ink::Foreground}});
            }
        } else if c.kind=="chart" {
            let hover=if live {mouse.filter(|&(mx,my)|contains(c.rect,mx,my)).map(|(mx,my)|nearest(&c.points,mx-c.rect[0],my-c.rect[1]))}else{None};
            let selected=c.value.as_u64().unwrap_or(0) as usize;
            if let Some(&(px,py))=hover.or_else(||c.selected.then_some(selected)).and_then(|idx|c.points.get(idx)) {
                projected.items.push(Item::Rect{rect:[local[0]+px-5.0*s,local[1]+py-5.0*s,10.0*s,10.0*s],ink:Ink::Success,radius:5.0*s});
                if let Some(h)=hover {
                    let r=r.borrow();let data=&r.instances[&key].prepared.spec.elements[&c.node].props["data"][h];
                    let text=format!("{}: {}",data["label"].as_str().unwrap(),data["value"]);
                    let width=(220.0*s).min(local[2]);let height=28.0*s;
                    let tx=(local[0]+px-width*0.5).clamp(local[0],local[0]+local[2]-width);
                    let ty=(local[1]+py-height-10.0*s).clamp(local[1],(local[1]+local[3]-height).max(local[1]));
                    projected.items.push(Item::Rect{rect:[tx,ty,width,height],ink:Ink::Surface,radius:5.0*s});
                    projected.items.push(Item::Text{x:tx+5.0*s,y:ty+5.0*s,width:(width-10.0*s).max(s),text,size:12.0,bold:true,ink:Ink::Foreground});
                }
            }
        }
        if live {
            let id=format!("json-{token}-{}-{}",c.node,c.tab.map_or_else(||"control".into(),|t|t.to_string()));
            let record=json!({"id":id,"kind":c.kind,"rect":c.rect,"label":c.label,"value":c.value,"options":c.options,"placeholder":c.placeholder,"disabled":c.disabled,"multiline":c.multiline,"selected":c.selected||c.tab.is_some_and(|t|c.value.as_u64()==Some(t as u64))});
            // Omit optional values rather than emitting nulls to host controls.
            let mut record=record;record.as_object_mut().unwrap().retain(|_,v|!v.is_null());
            let mut r=r.borrow_mut();
            if let Some(previous)=r.controls.iter_mut().find(|v|v["id"]==id) {*previous=record;}else{r.controls.push(record);}
            r.visible.insert(id,Target{instance:key.clone(),token,control:c});
        }
    }
    super::render(paint,&projected,x,y,theme,s,clip,occlusion);
}