//! Static, literal-only json-render v1. Validation and measured layout run only
//! during cached Markdown preparation; painting never parses model output.
use super::*;
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::HashSet;
mod chart;
mod diagram;
mod interactive;
pub use interactive::{Runtime, height, render_interactive};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Spec {
    root: String,
    elements: HashMap<String, Element>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Element {
    #[serde(rename = "type")]
    kind: String,
    props: Map<String, Value>,
    #[serde(default, deserialize_with = "children")]
    children: Option<Vec<String>>,
}

fn children<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Vec<String>>, D::Error> {
    Vec::<String>::deserialize(d).map(Some)
}

fn bounded(s: &str) -> bool { s.chars().count() <= 2000 }

fn valid_color(p:&Map<String,Value>)->bool {
    p.get("color").is_none_or(|v|v.as_str().is_some_and(|s|s.len()==7 && s.starts_with('#') && s.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)))
}
fn custom_color(p:&Map<String,Value>)->Option<u32> {
    u32::from_str_radix(p.get("color")?.as_str()?.strip_prefix('#')?,16).ok()
}

fn string(props: &Map<String, Value>, key: &str, required: bool) -> bool {
    match props.get(key) {
        Some(Value::String(s)) => bounded(s),
        None => !required,
        _ => false,
    }
}

fn parse(source: &str) -> Option<Spec> {
    if source.len() > 32768 { return None; }
    let spec: Spec = serde_json::from_str(source).ok()?;
    if spec.elements.is_empty() || spec.elements.len() > 64 || !bounded(&spec.root) { return None; }
    for (id, node) in &spec.elements {
        if !bounded(id) { return None; }
        let p = &node.props;
        let (allowed, valid): (&[&str], bool) = match node.kind.as_str() {
            "Stack" => (&[], true),
            "Card" => (&["title", "description"], string(p,"title",false) && string(p,"description",false)),
            "Text" => (&["text","color"], string(p,"text",true)),
            "Metric" => (&["label","value","detail","change","changeType","color"], string(p,"label",true) && string(p,"value",true) && string(p,"detail",false) && string(p,"change",false) && p.get("changeType").is_none_or(|v| matches!(v.as_str(), Some("positive"|"negative"|"neutral")))),
            "Badge" => (&["text","tone","color"], string(p,"text",true) && p.get("tone").is_none_or(|v| matches!(v.as_str(), Some("neutral"|"success"|"warning"|"danger")))),
            "Progress" => (&["label","value","color"], string(p,"label",true) && p.get("value").and_then(Value::as_f64).is_some_and(|n| n.is_finite() && (0.0..=100.0).contains(&n))),
            "Separator" => (&[], true),
            "Tabs" => (&["labels","defaultTab"], p.get("labels").and_then(Value::as_array).is_some_and(|a| (2..=8).contains(&a.len()) && a.iter().all(|v| v.as_str().is_some_and(bounded)) && node.children.as_ref().is_some_and(|c| c.len()==a.len()) && p.get("defaultTab").is_none_or(|v| v.as_u64().is_some_and(|i| i<a.len() as u64)))),
            "Collapsible" => (&["title","defaultOpen"], string(p,"title",true) && boolean(p,"defaultOpen")),
            "Filter" => (&["label","placeholder"], string(p,"label",true) && string(p,"placeholder",false)),
            "Input" => (&["label","placeholder","defaultValue","required","multiline"], string(p,"label",true) && string(p,"placeholder",false) && string(p,"defaultValue",false) && boolean(p,"required") && boolean(p,"multiline")),
            "Select" => (&["label","options","defaultValue","required"], string(p,"label",true) && boolean(p,"required") && p.get("options").and_then(Value::as_array).is_some_and(|a| (1..=16).contains(&a.len()) && a.iter().all(|v| v.as_str().is_some_and(bounded)) && a.iter().collect::<HashSet<_>>().len()==a.len() && p.get("defaultValue").is_none_or(|v| a.contains(v)))),
            "Checkbox" => (&["label","defaultChecked","required"], string(p,"label",true) && boolean(p,"defaultChecked") && boolean(p,"required")),
            "LineGraph" => (&["title","data","color"], string(p,"title",false) && p.get("data").and_then(Value::as_array).is_some_and(|data| {
                (2..=32).contains(&data.len()) && data.iter().all(|point| point.as_object().is_some_and(|point| {
                    point.len()==2 && string(point,"label",true) && point.get("value").and_then(Value::as_f64).is_some_and(f64::is_finite)
                }))
            })),
            "Diagram" => (&["title","direction","nodes","edges"], diagram::valid(p)),
            _ => return None,
        };
        if !valid || !valid_color(p) || p.keys().any(|k| !allowed.contains(&k.as_str())) { return None; }
        if node.children.is_some() && !matches!(node.kind.as_str(), "Stack"|"Card"|"Tabs"|"Collapsible"|"Filter") { return None; }
        if node.children.as_ref().is_some_and(|c| c.len() > 64 || c.iter().any(|id| !bounded(id))) { return None; }
    }
    fn visit(spec: &Spec, id: &str, depth: usize, seen: &mut HashSet<String>) -> Option<()> {
        if depth > 8 || !seen.insert(id.to_owned()) { return None; }
        let node = spec.elements.get(id)?;
        for child in node.children.iter().flatten() { visit(spec, child, depth + 1, seen)?; }
        Some(())
    }
    let mut seen = HashSet::new();
    visit(&spec, &spec.root, 1, &mut seen)?;
    (seen.len() == spec.elements.len()).then_some(spec)
}

fn boolean(p: &Map<String,Value>, key: &str) -> bool { p.get(key).is_none_or(Value::is_boolean) }

#[derive(Clone, Copy, Debug)]
enum Ink { Foreground, Muted, Accent, Success, Warning, Danger, Surface, Border, Grid, NeutralFill, SuccessFill, WarningFill, DangerFill, Custom(u32), CustomFill(u32) }
impl Ink {
    fn color(self, t: &IdeTheme) -> u32 {
        // UI neutrals must not inherit low-contrast terminal syntax colors.
        let light = ((t.bg >> 16) & 255) * 299 + ((t.bg >> 8) & 255) * 587 + (t.bg & 255) * 114 > 128_000;
        match (self, light) {
            (Self::Custom(color), _) => color,
            (Self::CustomFill(color), _) => {
                let bg:u32=if light {0xffffff}else{0x080808};
                let blend=|shift:u32|((((color>>shift)&255)+((bg>>shift)&255)*4)/5)<<shift;
                blend(16)|blend(8)|blend(0)
            }
            (Self::Foreground | Self::Accent, false) => 0xfafafa,
            (Self::Foreground | Self::Accent, true) => 0x18181b,
            (Self::Muted, false) => 0x929292, (Self::Muted, true) => 0x52525b,
            (Self::Surface, false) => 0x080808, (Self::Surface, true) => 0xffffff,
            (Self::Border, false) => 0x292929, (Self::Border, true) => 0xe4e4e7,
            (Self::Grid, false) => 0x1b1b1b, (Self::Grid, true) => 0xf0f0f0,
            (Self::NeutralFill, false) => 0x333333, (Self::NeutralFill, true) => 0xf4f4f5,
            (Self::Success, false) => 0x4ade80, (Self::Success, true) => 0x166534,
            (Self::SuccessFill, false) => 0x102c1e, (Self::SuccessFill, true) => 0xdcfce7,
            (Self::Warning, false) => 0xfbbf24, (Self::Warning, true) => 0x92400e,
            (Self::WarningFill, false) => 0x33270e, (Self::WarningFill, true) => 0xfef3c7,
            (Self::Danger, false) => 0xfca5a5, (Self::Danger, true) => 0x991b1b,
            (Self::DangerFill, false) => 0x391b20, (Self::DangerFill, true) => 0xfee2e2,
        }
    }
}

#[derive(Clone, Debug)]
enum Item {
    Diagram(diagram::Canvas),
    Rect { rect: [f32;4], ink: Ink, radius: f32 },
    Text { x: f32, y: f32, width: f32, text: String, size: f32, bold: bool, ink: Ink },
    Shape { points: Vec<(f32, f32)>, ink: Ink, alpha: f32 },
}

#[derive(Clone, Debug)]
pub struct NativeLayout {
    pub(super) height: f32,
    width: f32,
    items: Vec<Item>,
    prepared: Option<std::sync::Arc<interactive::Prepared>>,
}

// Measurement is injected so layout invariants can be tested without a GPU or
// font installation. Text is always literal (never Markdown, HTML, or links).
fn layout(spec: &Spec, width: f32, s: f32, measure: &mut impl FnMut(&str, f32, bool) -> f32) -> NativeLayout {
    fn text(out: &mut NativeLayout, value: &str, x: f32, y: &mut f32, width: f32, size: f32, bold: bool, ink: Ink, s: f32, measure: &mut impl FnMut(&str,f32,bool)->f32) {
        for raw in value.split('\n') {
            let mut line = String::new();
            for g in raw.graphemes(true) {
                let mut next = line.clone(); next.push_str(g);
                if !line.is_empty() && measure(&next,size,bold) > width {
                    out.items.push(Item::Text{x,y:*y,width,text:line,size,bold,ink});
                    *y += (size + 5.0) * s; line = g.to_owned();
                } else { line = next; }
            }
            out.items.push(Item::Text{x,y:*y,width,text:line,size,bold,ink});
            *y += (size + 5.0) * s;
        }
    }
    fn node(spec: &Spec, id: &str, out: &mut NativeLayout, x: f32, y: &mut f32, w: f32, s: f32, measure: &mut impl FnMut(&str,f32,bool)->f32) {
        let n = &spec.elements[id];
        let start = *y;
        let item_start = out.items.len();
        let mut tabs=Vec::new();
        // Groups use typography and spacing, not recursively nested boxes.
        let control_width = w;
        let color=custom_color(&n.props);
        let foreground=color.map(Ink::Custom).unwrap_or(Ink::Foreground);
        let mut field = |key: &str, size, bold, ink, out: &mut NativeLayout, y: &mut f32| {
            if let Some(v) = n.props.get(key).and_then(Value::as_str) {
                text(out,v,x,y,w,size,bold,ink,s,measure);
            }
        };
        match n.kind.as_str() {
            "Card" => {
                field("title",16.0,true,Ink::Foreground,out,y);
                if n.props.contains_key("title") && n.props.contains_key("description") { *y += 4.0*s; }
                field("description",13.0,false,Ink::Muted,out,y);
            }
            "Text" => field("text",14.0,false,foreground,out,y),
            "Metric" => {
                field("label",13.0,false,Ink::Muted,out,y);
                *y += 5.0*s;
                let value=n.props["value"].as_str().unwrap_or_default();
                let value_y=*y;
                text(out,value,x,y,w,26.0,true,foreground,s,measure);
                if let Some(change)=n.props.get("change").and_then(Value::as_str) {
                    let ink=match n.props.get("changeType").and_then(Value::as_str) {
                        Some("positive")=>Ink::Success, Some("negative")=>Ink::Danger, _=>Ink::Muted,
                    };
                    let value_w=measure(value,26.0,true);
                    if !value.contains('\n') && !change.contains('\n') && value_w+measure(change,13.0,false)+8.0*s<=w {
                        let mut change_y=value_y+11.0*s;
                        text(out,change,x+value_w+8.0*s,&mut change_y,w-value_w-8.0*s,13.0,false,ink,s,measure);
                    } else { text(out,change,x,y,w,13.0,false,ink,s,measure); }
                }
                if let Some(detail)=n.props.get("detail").and_then(Value::as_str) {
                    *y+=4.0*s; text(out,detail,x,y,w,12.0,false,Ink::Muted,s,measure);
                }
            }
            "Badge" => {
                let (ink, fill) = match n.props.get("tone").and_then(Value::as_str) {
                    Some("success")=>(Ink::Success, Ink::SuccessFill),
                    Some("warning")=>(Ink::Warning, Ink::WarningFill),
                    Some("danger")=>(Ink::Danger, Ink::DangerFill),
                    _=>(Ink::Foreground, Ink::NeutralFill),
                };
                let (ink,fill)=color.map(|c|(Ink::Custom(c),Ink::CustomFill(c))).unwrap_or((ink,fill));
                let label=n.props["text"].as_str().unwrap_or_default();
                let natural=label.split('\n').map(|line|measure(line,12.0,false)).fold(0.0,f32::max);
                let badge_w=(natural+32.0*s).min(w);
                let inset=(10.0*s).min(badge_w*0.2);
                let dot=(6.0*s).min(badge_w*0.1);
                let gap=(6.0*s).min(badge_w*0.1);
                let text_slot = out.items.len();
                out.items.push(Item::Rect{rect:[x,*y,badge_w,0.0],ink:fill,radius:12.5*s});
                *y += 4.0*s;
                out.items.push(Item::Rect{rect:[x+inset,*y+5.5*s,dot,dot],ink,radius:dot*0.5});
                text(out,label,x+inset+dot+gap,y,(badge_w-2.0*inset-dot-gap).max(s),12.0,false,ink,s,measure);
                *y += 4.0*s;
                // The text helper reserves five pixels below each line; center
                // the glyph block rather than that trailing line-box space.
                for item in &mut out.items[text_slot+2..] {if let Item::Text{y,..}=item {*y+=2.5*s;}}
                if let Item::Rect{rect,..}=&mut out.items[text_slot+1] {rect[1]=start+(*y-start-dot)*0.5;}
                if let Item::Rect{rect,..}=&mut out.items[text_slot] { rect[3]=*y-start; }
            }
            "Progress" => {
                let value = n.props["value"].as_f64().unwrap_or_default();
                let label=format!("{}  {}%",n.props["label"].as_str().unwrap_or_default(),value);
                text(out,&label,x,y,w,13.0,false,Ink::Muted,s,measure);
                *y += 9.0*s;
                out.items.push(Item::Rect{rect:[x,*y,w,7.0*s],ink:Ink::NeutralFill,radius:3.5*s});
                out.items.push(Item::Rect{rect:[x,*y,w*(value as f32/100.0),7.0*s],ink:color.map(Ink::Custom).unwrap_or(Ink::Accent),radius:3.5*s});
                *y += 7.0*s;
            }
            "Separator" => {
                *y+=4.0*s;
                out.items.push(Item::Rect{rect:[x,*y,w,s],ink:Ink::Grid,radius:0.0});
                *y+=5.0*s;
            }
            "Tabs" => {
                let labels=n.props["labels"].as_array().unwrap();
                let top=*y;let cell=w/labels.len() as f32;let inset=(8.0*s).min(cell*0.2);
                for (index,label) in labels.iter().enumerate() {
                    let mut bottom=top+8.0*s;
                    text(out,label.as_str().unwrap(),x+index as f32*cell+inset,&mut bottom,(cell-2.0*inset).max(s),14.0,true,Ink::Foreground,s,measure);
                    *y=(*y).max(bottom+8.0*s);
                    tabs.push([x+index as f32*cell,top,cell,0.0]);
                }
                for rect in &mut tabs {rect[3]=*y-top;}
            }
            "Collapsible" | "Checkbox" => {
                // These labels are painted in the control, not a second heading.
                *y += 36.0*s;
            }
            "Filter" | "Input" | "Select" => {
                field("label",12.0,false,Ink::Muted,out,y);
                *y += if n.kind=="Input" && n.props.get("multiline").and_then(Value::as_bool)==Some(true) { 88.0*s } else { 40.0*s };
            }
            "LineGraph" => {
                field("title",14.0,true,Ink::Foreground,out,y);
                if n.props.contains_key("title") { *y+=12.0*s; }
                *y+=chart::layout(out,n.props["data"].as_array().unwrap(),x,*y,w,s,foreground,measure);
            }
            "Diagram" => {
                field("title",14.0,true,Ink::Foreground,out,y);
                if n.props.contains_key("title") { *y+=12.0*s; }
                out.items.push(Item::Diagram(diagram::Canvas::new(&n.props,[x,*y,w,320.0*s],s,measure)));
                *y+=320.0*s;
            }
            _ => {}
        }
        let own_end=out.items.len();
        let own_bottom=*y;
        if let Some(children) = &n.children {
            for (i, child) in children.iter().enumerate() {
                if i > 0 || *y > start { *y += 20.0*s; }
                node(spec,child,out,x,y,w,s,measure);
            }
        }
        let info=interactive::NodeMeasure { start, own_bottom, bottom:*y, x, width:w, control_width, items:item_start..own_end, tabs };
        std::sync::Arc::make_mut(out.prepared.as_mut().unwrap()).nodes.insert(id.to_owned(),info);
    }
    let mut out = NativeLayout{height:0.0,width:width.max(s),items:vec![],prepared:Some(std::sync::Arc::new(interactive::Prepared { spec:spec.clone(), nodes:HashMap::new(), scale:s, source:String::new() }))};
    let mut y = 0.0;
    node(spec,&spec.root,&mut out,0.0,&mut y,width.max(s),s,measure);
    out.height = y.max(8.0*s);
    out
}

pub(super) fn completed_fence(paint: &mut dyn crate::primitives::paint::Paint, lang: &str, lines: &[String], width: f32, s: f32) -> Option<AssistantMarkdownBlock> {
    if lang.trim() != "json-render" { return None; }
    // Check before joining, including separating newlines.
    if lines.iter().map(String::len).sum::<usize>() + lines.len().saturating_sub(1) > 32768 { return None; }
    let spec = parse(&join_markdown_lines(lines))?;
    let font_id = crate::primitives::prose_font::ui_prose_font_id(paint.font_library());
    let mut prepared = layout(&spec,width.max(s),s,&mut |text,size,bold| {
        measure_text_cached(paint,text,&DrawOpts{font_id,font_size:size*s,bold,..DrawOpts::default()})
    });
    std::sync::Arc::make_mut(prepared.prepared.as_mut().unwrap()).source=join_markdown_lines(lines);
    Some(AssistantMarkdownBlock::JsonRender(prepared))
}

pub(super) fn render(paint: &mut dyn crate::primitives::paint::Paint, layout: &NativeLayout, x: f32, y: f32, theme: &IdeTheme, s: f32, clip: [f32;4], occlusion: &[[f32;4]]) {
    let Some(clip) = intersect_rect(clip,[x,y,layout.width,layout.height]) else { return; };
    let font_id = crate::primitives::prose_font::ui_prose_font_id(paint.font_library());
    let mut clips = vec![clip];
    for cover in occlusion {
        let mut next=Vec::new();
        for c in clips {
            if let Some(i)=intersect_rect(c,*cover) {
                for piece in [[c[0],c[1],c[2],i[1]-c[1]], [c[0],i[1]+i[3],c[2],c[1]+c[3]-i[1]-i[3]], [c[0],i[1],i[0]-c[0],i[3]], [i[0]+i[2],i[1],c[0]+c[2]-i[0]-i[2],i[3]]] {
                    if piece[2]>0.0 && piece[3]>0.0 { next.push(piece); }
                }
            } else { next.push(c); }
        }
        clips=next;
    }
    for item in &layout.items {
        match item {
            Item::Diagram(canvas) => diagram::paint(paint,canvas,x,y,theme,s,clip,occlusion),
            Item::Rect{rect,ink,radius} => {
                for c in &clips { draw_rounded_rect_clipped(paint,[x+rect[0],y+rect[1],rect[2],rect[3]],theme.f32(ink.color(theme)),*radius,ORDER_PANEL+2,*c); }
            }
            Item::Text{x:tx,y:ty,width,text,size,bold,ink} => {
                let Some(clip) = intersect_rect(clip,[x+tx,y+ty,*width,(size+5.0)*s]) else { continue; };
                if let Some(opts)=opts_with_clip(DrawOpts{font_id,font_size:size*s,bold:*bold,color:theme.u8(ink.color(theme)),..DrawOpts::default()},clip) {
                    draw_text_clipped(paint,x+tx,y+ty,text,&opts,occlusion);
                }
            }
            Item::Shape{points,ink,alpha} => {
                let points:Vec<_>=points.iter().map(|(px,py)|(px+x,py+y)).collect();
                let mut color=theme.f32(ink.color(theme));color[3]=*alpha;
                for c in &clips {
                    let polygon=chart::clip_polygon(&points,*c);
                    if polygon.len()>=3 {paint.polygon_ordered(&polygon,0.0,color,ORDER_PANEL+2);}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use crate::primitives::paint_record::{PaintCommand, RecordingPainter};

    fn one(kind: &str, props: Value) -> Value {
        json!({"root":"a","elements":{"a":{"type":kind,"props":props}}})
    }
    fn valid(v: &Value) -> bool { parse(&v.to_string()).is_some() }
    fn example() -> Value {
        json!({"root":"stack","elements":{
            "stack":{"type":"Stack","props":{},"children":["card","badge","progress"]},
            "card":{"type":"Card","props":{"title":"Report","description":"Read only"},"children":["metric","text"]},
            "metric":{"type":"Metric","props":{"label":"Revenue","value":"$42","detail":"Up today"}},
            "text":{"type":"Text","props":{"text":"Literal <script>alert(1)</script> ${state.secret}"}},
            "badge":{"type":"Badge","props":{"text":"Healthy","tone":"success"}},
            "progress":{"type":"Progress","props":{"label":"Complete","value":75}}
        }})
    }

    #[test]
    fn json_render_exact_literal_catalog() {
        assert!(valid(&example()));
        for tone in ["neutral","success","warning","danger"] { assert!(valid(&one("Badge",json!({"text":"ok","tone":tone})))); }
        assert!(valid(&one("Badge",json!({"text":"ok"}))));
        assert!(valid(&one("Card",json!({}))));
        assert!(valid(&one("Separator",json!({}))));
        assert!(valid(&one("Metric",json!({"label":"Revenue","value":"$12,400","change":"+18%","changeType":"positive"}))));
        assert!(valid(&one("LineGraph",json!({"data":[{"label":"Mon","value":10},{"label":"Tue","value":20}]}))));
        for v in [0.0, 0.5, 100.0] { assert!(valid(&one("Progress",json!({"label":"p","value":v})))); }
        for bad in [
            one("Button",json!({"text":"run"})), one("Stack",json!({"gap":2})),
            one("Text",json!({"text":{"$state":"secret"}})), one("Text",json!({"text":"x","on":{}})),
            one("Text",json!({})), one("Text",json!({"text":null})),
            one("Metric",json!({"label":"l","value":42})),
            one("Badge",json!({"text":"x","tone":"blue"})), one("Badge",json!({"text":"x","tone":null})),
            one("Card",json!({"title":null})), one("Card",json!({"description":null})),
            one("Progress",json!({"label":"p","value":"50"})),
            one("Progress",json!({"label":"p","value":-1})), one("Progress",json!({"label":"p","value":101})),
            one("Metric",json!({"label":"l","value":"v","changeType":"invented"})),
            one("LineGraph",json!({"data":[]})), one("LineGraph",json!({"data":[{"label":"Mon","value":1}]})),
            one("LineGraph",json!({"title":null,"data":[{"label":"Mon","value":1},{"label":"Tue","value":2}]})),
            one("LineGraph",json!({"data":[{"label":"Mon","value":1,"url":"https://example.com"},{"label":"Tue","value":2}]})),
            one("LineGraph",json!({"data":vec![json!({"label":"Day","value":1});33]}))
        ] { assert!(!valid(&bad),"{bad}"); }
        let mut bad = one("Stack",json!({}));
        bad["elements"]["a"]["children"] = Value::Null;
        assert!(!valid(&bad));
        bad["elements"]["a"]["children"] = json!([]);
        assert!(valid(&bad));
        bad["elements"]["a"]["type"] = json!("Text");
        bad["elements"]["a"]["props"] = json!({"text":"x"});
        assert!(!valid(&bad));
        for target in ["top","node"] {
            let mut bad = one("Stack",json!({}));
            if target=="top" { bad["actions"]=json!({}); } else { bad["elements"]["a"]["bindings"]=json!({}); }
            assert!(!valid(&bad));
        }
        assert!(parse("{\"root\":").is_none());
        assert!(parse("{\"root\":\"a\",\"elements\":{},\"root\":\"b\"}").is_none());
    }

    #[test]
    fn badges_center_glyph_blocks_and_support_literal_colors() {
        for s in [0.75,1.0,1.5,2.0] {
            for label in ["Partly cloudy","Partly\ncloudy"] {
                let spec=parse(&one("Badge",json!({"text":label,"color":"#60A5FA"})).to_string()).unwrap();
                let l=layout(&spec,280.0*s,s,&mut |text,size,_|text.chars().count() as f32*size*s*0.5);
                let Item::Rect{rect:badge,ink:Ink::CustomFill(0x60a5fa),..}=l.items[0] else{panic!("missing tinted badge");};
                let Item::Rect{rect:dot,..}=l.items[1] else{panic!("missing dot");};
                let lines:Vec<_>=l.items.iter().filter_map(|item|if let Item::Text{y,size,ink:Ink::Custom(0x60a5fa),..}=item {Some((*y,*size))}else{None}).collect();
                let center=badge[1]+badge[3]*0.5;
                assert!((dot[1]+dot[3]*0.5-center).abs()<0.01);
                let first=lines.first().unwrap();let last=lines.last().unwrap();
                assert!(((first.0+last.0+last.1*s)*0.5-center).abs()<0.01);
            }
        }
        for color in [json!("red"),json!("#fff"),json!("#12345678"),json!("#zzzzzz"),Value::Null] {
            assert!(!valid(&one("Text",json!({"text":"x","color":color}))));
        }
        assert!(valid(&one("Text",json!({"text":"x","color":"#34d399"}))));
        assert!(!valid(&one("Form",json!({"submitLabel":"Send","prompt":"p"}))));
        assert!(!valid(&one("Button",json!({"label":"Send","prompt":"p"}))));
    }

    fn chain(count: usize) -> Value {
        let mut elements = Map::new();
        for i in 0..count {
            let mut n=json!({"type":"Stack","props":{}});
            if i+1<count { n["children"]=json!([(i+1).to_string()]); }
            elements.insert(i.to_string(),n);
        }
        json!({"root":"0","elements":elements})
    }

    #[test]
    fn json_render_tree_uniqueness_and_depth() {
        assert!(valid(&chain(8)));
        assert!(!valid(&chain(9)));
        let mut v=chain(2);
        for children in [json!(["0"]),json!(["missing"]),json!(["1","1"])] {
            v["elements"]["0"]["children"]=children;
            assert!(!valid(&v));
        }
        v=chain(3);
        v["elements"]["0"]["children"]=json!(["1","2"]);
        assert!(!valid(&v)); // diamond/shared node
        v=chain(3);
        v["elements"]["2"]["children"]=json!(["0"]);
        assert!(!valid(&v)); // multi-node cycle
        v=chain(2);
        v["elements"]["0"]["children"]=json!([]);
        assert!(!valid(&v)); // unreachable node
        v["root"]=json!("absent");
        assert!(!valid(&v));
    }

    #[test]
    fn json_render_byte_element_and_unicode_bounds() {
        assert!(valid(&one("Text",json!({"text":"🦀".repeat(2000)}))));
        assert!(!valid(&one("Text",json!({"text":"🦀".repeat(2001)}))));
        let v=one("Text",json!({"text":"x"})).to_string();
        assert!(parse(&format!("{v}{}"," ".repeat(32768-v.len()))).is_some());
        assert!(parse(&format!("{v}{}"," ".repeat(32769-v.len()))).is_none());
        let mut v=chain(1);
        let mut ids=vec![];
        for i in 1..64 { let id=i.to_string(); v["elements"][&id]=json!({"type":"Stack","props":{}}); ids.push(id); }
        v["elements"]["0"]["children"]=json!(ids);
        assert!(valid(&v));
        v["elements"]["extra"]=json!({"type":"Stack","props":{}});
        assert!(!valid(&v));
        let id="x".repeat(2001);
        assert!(!valid(&json!({"root":id,"elements":{id:{"type":"Stack","props":{}}}})));
    }

    #[test]
    fn json_render_layout_wrap_scale_and_native_paint() {
        let spec=parse(&example().to_string()).unwrap();
        let measure=|s: &str,size:f32,_|s.chars().count() as f32*size*0.5;
        let wide=layout(&spec,400.0,1.0,&mut measure.clone());
        let narrow=layout(&spec,100.0,1.0,&mut measure.clone());
        assert!(narrow.height>wide.height);
        let scaled=layout(&spec,800.0,2.0,&mut |s,size,bold|measure(s,size,bold)*2.0);
        assert!((scaled.height-wide.height*2.0).abs()<0.01);
        for item in &narrow.items {
            match item {
                        Item::Diagram(_) => {},
                Item::Rect{rect,..} => assert!(rect.iter().all(|x|x.is_finite()) && rect[1]+rect[3]<=narrow.height),
                        Item::Text{y,size,..} => assert!(y+size<=narrow.height),
                        Item::Shape{points,..} => assert!(points.iter().all(|(_,y)|*y<=narrow.height)),
            }
        }
        let mut paint=RecordingPainter::new("monospace",817,vec![]);
        render(&mut paint,&wide,10.0,20.0,&IdeTheme::default(),1.0,[0.0,0.0,500.0,1000.0],&[]);
        let commands=paint.commands();
        assert!(commands.iter().any(|c|matches!(c,PaintCommand::Rect{..})));
        for expected in ["Report","Revenue","$42","Healthy","Complete  75%"] {
            assert!(commands.iter().any(|c|matches!(c,PaintCommand::Text{text,..} if text==expected)),"missing {expected}");
        }
        assert!(commands.iter().filter_map(|c|if let PaintCommand::Text{family,..}=c {Some(family)} else {None}).all(|family|family.as_deref()==Some("Geist")));
        let badge=wide.items.iter().find_map(|item|match item { Item::Rect{rect,ink:Ink::SuccessFill,..}=>Some(rect),_=>None }).unwrap();
        assert!(badge[2]<wide.width*0.5,"status pill must fit its content, not fill the card");
        let mut hidden=RecordingPainter::new("monospace",819,vec![]);
        render(&mut hidden,&wide,10.0,-1000.0,&IdeTheme::default(),1.0,[0.0,0.0,500.0,200.0],&[]);
        assert!(hidden.commands().is_empty());
        render(&mut hidden,&wide,10.0,20.0,&IdeTheme::default(),1.0,[0.0,0.0,500.0,1000.0],&[[0.0,0.0,500.0,1000.0]]);
        assert!(hidden.commands().is_empty());
    }

    #[test]
    fn json_render_neutral_palette_has_readable_labels_in_dark_and_light_modes() {
        fn luminance(rgb:u32)->f32 {
            let channel=|v:u32|{let x=v as f32/255.0;if x<=0.04045{x/12.92}else{((x+0.055)/1.055).powf(2.4)}};
            channel((rgb>>16)&255)*0.2126+channel((rgb>>8)&255)*0.7152+channel(rgb&255)*0.0722
        }
        let contrast=|a:u32,b:u32|{let(a,b)=(luminance(a),luminance(b));(a.max(b)+0.05)/(a.min(b)+0.05)};
        for bg in [0x000000,0xffffff] {
            let mut theme=IdeTheme::default();theme.bg=bg;theme.muted=bg;theme.accent=bg;
            assert!(contrast(Ink::Muted.color(&theme),Ink::Surface.color(&theme))>=4.5);
            assert!(contrast(Ink::Foreground.color(&theme),Ink::Surface.color(&theme))>=7.0);
            for (ink,fill) in [(Ink::Success,Ink::SuccessFill),(Ink::Warning,Ink::WarningFill),(Ink::Danger,Ink::DangerFill)] {
                assert!(contrast(ink.color(&theme),fill.color(&theme))>=4.5);
            }
        }
    }

    #[test]
    fn json_render_completed_only_cache_and_fallback() {
        let mut paint=RecordingPainter::new("monospace",818,vec![]);
        let theme=IdeTheme::default();
        let pane=NeoismAgentPane::new();
        // Exceeds the incremental code fast-path threshold: closing the fence
        // must still promote rather than retain the prior streaming code job.
        let source=one("Text",json!({"text":"x".repeat(1999)})).to_string();
        let open=format!("```json-render\n{source}\n");
        let blocks=layout_assistant_markdown_cached(&mut paint,&pane,&open,400.0,&theme,1.0);
        assert!(matches!(&blocks[0],AssistantMarkdownBlock::Code{..}));
        let closed=format!("{open}```");
        let blocks=layout_assistant_markdown_cached(&mut paint,&pane,&closed,400.0,&theme,1.0);
        assert!(matches!(&blocks[0],AssistantMarkdownBlock::JsonRender(_)));
        let AssistantMarkdownBlock::JsonRender(layout)=&blocks[0] else { unreachable!() };
        assert_eq!(markdown_block_height(&blocks[0],430.0,&pane,1.0),layout.height);
        let again=layout_assistant_markdown_cached(&mut paint,&pane,&closed,400.0,&theme,1.0);
        assert!(Rc::ptr_eq(&blocks,&again));
        for (lang,body) in [("json",source.as_str()),("json-render","{\"root\":"),("json-render","{\"root\":\"a\",\"elements\":{}}") ] {
            let blocks=layout_assistant_markdown(&mut paint,&format!("```{lang}\n{body}\n```"),400.0,&theme,1.0);
            let AssistantMarkdownBlock::Code{lines,..}=&blocks[0] else {panic!("must retain fenced source")};
            assert_eq!(lines.as_ref(),&vec![body.to_owned()]);
        }
    }
}