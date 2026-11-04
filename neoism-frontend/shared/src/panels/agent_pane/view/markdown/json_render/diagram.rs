//! Bounded graph preparation and viewport-only transforms. Model data never
//! supplies coordinates, executable actions, or runtime transform defaults.
use super::*;
use serde_json::json;
use std::sync::Arc;

pub(super) fn valid(p:&Map<String,Value>)->bool {
    if !string(p,"title",false) || !p.get("direction").is_none_or(|v|matches!(v.as_str(),Some("LR"|"TB"))) {return false;}
    let Some(nodes)=p.get("nodes").and_then(Value::as_array) else{return false;};
    let Some(edges)=p.get("edges").and_then(Value::as_array) else{return false;};
    if !(1..=32).contains(&nodes.len()) || edges.len()>64 {return false;}
    let mut ids=HashSet::new();
    for node in nodes {
        let Some(n)=node.as_object() else{return false;};
        if n.keys().any(|k|!matches!(k.as_str(),"id"|"label"|"shape"|"color")) || !valid_color(n) {return false;}
        let Some(id)=n.get("id").and_then(Value::as_str) else{return false;};
        if !(1..=64).contains(&id.len()) || !id.bytes().all(|c|c.is_ascii_alphanumeric() || c==b'_' || c==b'-') || !ids.insert(id) {return false;}
        if !n.get("label").and_then(Value::as_str).is_some_and(|s|(1..=160).contains(&s.chars().count())) || !n.get("shape").is_none_or(|v|matches!(v.as_str(),Some("rect"|"rounded"|"diamond"))) {return false;}
    }
    let mut pairs=HashSet::new();
    for edge in edges {
        let Some(e)=edge.as_object() else{return false;};
        if e.keys().any(|k|!matches!(k.as_str(),"from"|"to"|"label"|"color")) || !valid_color(e) {return false;}
        let (Some(from),Some(to))=(e.get("from").and_then(Value::as_str),e.get("to").and_then(Value::as_str)) else{return false;};
        if !ids.contains(from) || !ids.contains(to) || !pairs.insert((from,to)) || !e.get("label").is_none_or(|v|v.as_str().is_some_and(|s|s.chars().count()<=80)) {return false;}
    }
    true
}

#[derive(Clone,Copy,Debug,PartialEq)]
pub(super) struct Transform {zoom:f32, pan_x:f32, pan_y:f32}
impl Default for Transform {fn default()->Self {Self{zoom:1.0,pan_x:0.0,pan_y:0.0}}}
impl Transform {
    pub fn value(self)->Value {json!({"zoom":self.zoom,"panX":self.pan_x,"panY":self.pan_y})}
    pub fn from_value(v:&Value)->Self {
        let finite=|key:&str,default:f32|v[key].as_f64().filter(|n|n.is_finite()).map(|n|n.clamp(-8192.0,8192.0) as f32).unwrap_or(default);
        Self{zoom:finite("zoom",1.0).clamp(0.25,4.0),pan_x:finite("panX",0.0),pan_y:finite("panY",0.0)}
    }
    pub fn event(mut self,v:Option<&Value>)->Result<Self,String> {
        match v {
            Some(Value::String(cmd))=>match cmd.as_str() {
                "fit"=>return Ok(Self::default()),
                "zoom-in"=>self.zoom=(self.zoom*1.25).min(4.0),
                "zoom-out"=>self.zoom=(self.zoom/1.25).max(0.25),
                "pan-left"=>self.pan_x-=40.0,"pan-right"=>self.pan_x+=40.0,
                "pan-up"=>self.pan_y-=40.0,"pan-down"=>self.pan_y+=40.0,
                _=>return Err("Invalid diagram command".into()),
            },
            Some(Value::Object(o)) if o.len()==2 && o.contains_key("dx") && o.contains_key("dy")=>{
                let delta=|key:&str|o[key].as_f64().filter(|n|n.is_finite()).map(|n|n.clamp(-1024.0,1024.0) as f32).ok_or_else(||"Invalid diagram delta".to_owned());
                self.pan_x+=delta("dx")?;self.pan_y+=delta("dy")?;
            }
            _=>return Err("Invalid diagram event".into()),
        }
        self.pan_x=self.pan_x.clamp(-8192.0,8192.0);self.pan_y=self.pan_y.clamp(-8192.0,8192.0);
        Ok(self)
    }
}

#[derive(Clone,Debug)]
struct Geometry {items:Vec<Item>,width:f32,height:f32}
#[derive(Clone,Debug)]
pub(super) struct Canvas {pub rect:[f32;4],pub transform:Transform,graph:Arc<Geometry>}

// Prefer whole words; only an individually oversized word is split, at grapheme
// boundaries. Explicit newlines (including empty lines) remain intentional.
fn lines(text:&str,max:f32,size:f32,measure:&mut impl FnMut(&str,f32,bool)->f32)->Vec<(String,f32)> {
    let mut out=vec![];
    for raw in text.split('\n') {
        let mut line=String::new();
        for word in raw.split_whitespace() {
            let next=if line.is_empty() {word.to_owned()}else{format!("{line} {word}")};
            if measure(&next,size,false)<=max {line=next;continue;}
            if !line.is_empty() {let w=measure(&line,size,false);out.push((std::mem::take(&mut line),w));}
            for g in word.graphemes(true) {
                let next=format!("{line}{g}");
                if !line.is_empty() && measure(&next,size,false)>max {let w=measure(&line,size,false);out.push((line,w));line=g.into();}else{line=next;}
            }
        }
        let w=measure(&line,size,false);out.push((line,w));
    }
    out
}
fn rect(items:&mut Vec<Item>,r:[f32;4],ink:Ink,radius:f32) {items.push(Item::Rect{rect:r,ink,radius});}
fn stroke_ink(items:&mut Vec<Item>,a:(f32,f32),b:(f32,f32),ink:Ink) {
    let (dx,dy)=(b.0-a.0,b.1-a.1);let len=dx.hypot(dy).max(0.001);let (nx,ny)=(-dy/len*0.7,dx/len*0.7);
    items.push(Item::Shape{points:vec![(a.0+nx,a.1+ny),(b.0+nx,b.1+ny),(b.0-nx,b.1-ny),(a.0-nx,a.1-ny)],ink,alpha:1.0});
}

fn overlaps(a:[f32;4],b:[f32;4],gap:f32)->bool {
    a[0]<b[0]+b[2]+gap && a[0]+a[2]+gap>b[0] && a[1]<b[1]+b[3]+gap && a[1]+a[3]+gap>b[1]
}
fn segment_hits(a:(f32,f32),b:(f32,f32),r:[f32;4])->bool {
    if a.0==b.0 {a.0>r[0] && a.0<r[0]+r[2] && a.1.min(b.1)<r[1]+r[3] && a.1.max(b.1)>r[1]}
    else {a.1>r[1] && a.1<r[1]+r[3] && a.0.min(b.0)<r[0]+r[2] && a.0.max(b.0)>r[0]}
}

// Repair obstructed orthogonal paths on a bounded visibility grid. Keep the
// preferred cycle/reciprocal lanes when unobstructed. Grid edges cannot enter
// node interiors; boundary ports are the only places paths meet nodes.
fn route(path:Vec<(f32,f32)>,nodes:&[[f32;4]])->Vec<(f32,f32)> {
    if !path.windows(2).any(|p|nodes.iter().any(|&r|segment_hits(p[0],p[1],r))) {return path;}
    let start=path[0];let end=*path.last().unwrap();
    let mut xs=vec![start.0,end.0];let mut ys=vec![start.1,end.1];
    for r in nodes {xs.extend([r[0]-12.0,r[0]+r[2]+12.0]);ys.extend([r[1]-12.0,r[1]+r[3]+12.0]);}
    xs.sort_by(f32::total_cmp);xs.dedup();ys.sort_by(f32::total_cmp);ys.dedup();
    let nx=xs.len();let count=nx*ys.len();
    let id=|p:(f32,f32)|ys.iter().position(|&y|y==p.1).unwrap()*nx+xs.iter().position(|&x|x==p.0).unwrap();
    let first=id(start);let last=id(end);let mut dist=vec![u64::MAX;count];let mut prev=vec![usize::MAX;count];
    let mut queue=std::collections::BinaryHeap::new();dist[first]=0;queue.push(std::cmp::Reverse((0u64,first)));
    while let Some(std::cmp::Reverse((cost,i)))=queue.pop() {
        if cost!=dist[i] {continue;}if i==last {break;}
        let (x,y)=(i%nx,i/nx);let a=(xs[x],ys[y]);
        for j in [x.checked_sub(1).map(|x|y*nx+x),(x+1<nx).then_some(i+1),y.checked_sub(1).map(|y|y*nx+x),(y+1<ys.len()).then_some(i+nx)].into_iter().flatten() {
            let b=(xs[j%nx],ys[j/nx]);
            if nodes.iter().any(|&r|segment_hits(a,b,r)) {continue;}
            let next=cost+((a.0-b.0).abs()+(a.1-b.1).abs()).mul_add(100.0,1.0).ceil() as u64;
            if next<dist[j] {dist[j]=next;prev[j]=i;queue.push(std::cmp::Reverse((next,j)));}
        }
    }
    assert_ne!(dist[last],u64::MAX,"outer visibility lanes must connect all ports");
    let mut out=vec![end];let mut i=last;
    while i!=first {i=prev[i];out.push((xs[i%nx],ys[i/nx]));}out.reverse();
    let mut compact:Vec<(f32,f32)>=vec![];
    for p in out {if compact.len()>=2 {let a=compact[compact.len()-2];let b=compact[compact.len()-1];if (a.0==b.0&&b.0==p.0)||(a.1==b.1&&b.1==p.1) {compact.pop();}}compact.push(p);}compact
}
impl Canvas {
    pub fn new(p:&Map<String,Value>,viewport:[f32;4],s:f32,measure:&mut impl FnMut(&str,f32,bool)->f32)->Self {
        let nodes=p["nodes"].as_array().unwrap();let edges=p["edges"].as_array().unwrap();let n=nodes.len();
        let index:HashMap<_,_>=nodes.iter().enumerate().map(|(i,v)|(v["id"].as_str().unwrap(),i)).collect();
        let pairs:Vec<_>=edges.iter().map(|e|(index[e["from"].as_str().unwrap()],index[e["to"].as_str().unwrap()])).collect();
        // Transitive closure identifies SCCs without recursive graph traversal.
        // Rank the condensation DAG; nodes within cycles share a layer.
        let mut reach=vec![vec![false;n];n];
        for i in 0..n {reach[i][i]=true;}
        for &(a,b) in &pairs {reach[a][b]=true;}
        for k in 0..n {for a in 0..n {for b in 0..n {let via=reach[a][k]&&reach[k][b];reach[a][b]|=via;}}}
        let component:Vec<_>=(0..n).map(|a|(0..n).find(|&b|reach[a][b]&&reach[b][a]).unwrap()).collect();
        let mut ranks=vec![0usize;n];
        for _ in 0..n {for &(a,b) in &pairs {if component[a]!=component[b] {let next=ranks[component[a]]+1;ranks[component[b]]=ranks[component[b]].max(next).min(n-1);}}}
        let mut measured=|text:&str,size,bold| {
            let w=measure(text,size,bold)/s;
            if w.is_finite() {w.clamp(0.0,4096.0)}else{(text.chars().count() as f32*size*0.6).min(4096.0)}
        };
        let labels:Vec<_>=nodes.iter().map(|v|lines(v["label"].as_str().unwrap(),168.0,13.0,&mut measured)).collect();
        // Diamonds need twice the label's bounding rectangle to keep text
        // inside the sloping sides. Uniform cells keep routing deterministic.
        let factor=|i:usize|if nodes[i]["shape"].as_str()==Some("diamond") {2.0}else{1.0};
        let nw=labels.iter().enumerate().map(|(i,l)|l.iter().map(|(_,w)|*w).fold(0.0,f32::max)*factor(i)+40.0).fold(120.0,f32::max).min(8240.0);
        let nh=labels.iter().enumerate().map(|(i,l)|l.len() as f32*18.0*factor(i)+32.0).fold(64.0,f32::max);
        let tb=p.get("direction").and_then(Value::as_str)==Some("TB");
        let cyclic=pairs.iter().any(|&(a,b)|a!=b && component[a]==component[b]);
        let (mx,my)=if cyclic {if tb {(40.0,160.0)}else{(160.0,40.0)}}else{(40.0,40.0)};
        let mut counts=vec![0usize;n];let mut positions=vec![];
        for a in 0..n {let rank=ranks[component[a]];let slot=counts[rank];counts[rank]+=1;
            positions.push(if tb {(mx+slot as f32*(nw+64.0),my+rank as f32*(nh+80.0))}else{(mx+rank as f32*(nw+100.0),my+slot as f32*(nh+64.0))});
        }
        let boxes:Vec<_>=labels.iter().enumerate().map(|(i,l)| {
            let w=(l.iter().map(|(_,w)|*w).fold(0.0,f32::max)*factor(i)+40.0).max(120.0);
            let h=(l.len() as f32*18.0*factor(i)+32.0).max(64.0);
            [positions[i].0+(nw-w)*0.5,positions[i].1+(nh-h)*0.5,w,h]
        }).collect();
        let width=positions.iter().map(|p|p.0+nw+56.0).fold(1.0,f32::max);
        let height=positions.iter().map(|p|p.1+nh+56.0).fold(1.0,f32::max);
        let mut items=vec![];
        let mut paths=vec![];
        for (edge_index,&(a,b)) in pairs.iter().enumerate() {
            let [ax,ay,aw,ah]=boxes[a];let [bx,by,bw,bh]=boxes[b];
            let path=if a==b {vec![(ax+aw,ay+ah*0.6),(ax+aw+28.0,ay+ah*0.6),(ax+aw+28.0,ay-24.0),(ax+aw*0.5,ay-24.0),(ax+aw*0.5,ay)]}
                // Keep cycle lanes opposite outgoing edges, with separate
                // lanes for reciprocal arrows and room for their labels.
                else if component[a]==component[b] {let lane=20.0+edge_index as f32*12.0;if tb {let mid=ay.min(by)-lane;vec![(ax+aw*0.5,ay),(ax+aw*0.5,mid),(bx+bw*0.5,mid),(bx+bw*0.5,by)]}else{let mid=ax.min(bx)-lane;vec![(ax,ay+ah*0.5),(mid,ay+ah*0.5),(mid,by+bh*0.5),(bx,by+bh*0.5)]}}
                else if tb {let mid=(ay+ah+by)*0.5;vec![(ax+aw*0.5,ay+ah),(ax+aw*0.5,mid),(bx+bw*0.5,mid),(bx+bw*0.5,by)]}
                else {let mid=(ax+aw+bx)*0.5;vec![(ax+aw,ay+ah*0.5),(mid,ay+ah*0.5),(mid,by+bh*0.5),(bx,by+bh*0.5)]};
            paths.push(route(path,&boxes));
        }
        for (edge,path) in edges.iter().zip(&paths) {
            let ink=custom_color(edge.as_object().unwrap()).map(Ink::Custom).unwrap_or(Ink::Muted);
            for pair in path.windows(2) {stroke_ink(&mut items,pair[0],pair[1],ink);}
            let tip=path[path.len()-1];let prev=path[path.len()-2];let len=(tip.0-prev.0).hypot(tip.1-prev.1).max(0.001);let (dx,dy)=((tip.0-prev.0)/len,(tip.1-prev.1)/len);
            items.push(Item::Shape{points:vec![tip,(tip.0-dx*8.0-dy*4.0,tip.1-dy*8.0+dx*4.0),(tip.0-dx*8.0+dy*4.0,tip.1-dy*8.0-dx*4.0)],ink,alpha:1.0});
        }
        // Place complete padded rectangles, not baseline points or lane offsets.
        // Nearby candidates follow each segment, avoiding other edges as well as
        // nodes/labels. A bounded exterior shelf guarantees a solution even for
        // 64 repeated, very tall labels. Leaders preserve edge association there.
        let mut occupied=boxes.clone();let mut shelf_y=0.0;let mut leaders:Vec<Vec<(f32,f32)>>=vec![];
        let shelf_x=paths.iter().flatten().map(|p|p.0).fold(width,f32::max)+24.0;
        for (edge_index,edge) in edges.iter().enumerate() {
            let ink=custom_color(edge.as_object().unwrap()).map(Ink::Custom).unwrap_or(Ink::Muted);
            let Some(label)=edge.get("label").and_then(Value::as_str).filter(|s|!s.is_empty()) else{continue;};
            let wrapped=lines(label,120.0,11.0,&mut measured);
            let w=wrapped.iter().map(|(_,w)|*w).fold(1.0,f32::max)+12.0;let h=wrapped.len() as f32*16.0+8.0;
            let path=&paths[edge_index];let mut chosen=None;
            'search: for segment in path.windows(2) {for t in [0.5,0.25,0.75] {
                let (cx,cy)=(segment[0].0+(segment[1].0-segment[0].0)*t,segment[0].1+(segment[1].1-segment[0].1)*t);
                let candidates=if segment[0].0==segment[1].0 {[[cx-w-6.0,cy-h*0.5,w,h],[cx+6.0,cy-h*0.5,w,h]]}else{[[cx-w*0.5,cy-h-6.0,w,h],[cx-w*0.5,cy+6.0,w,h]]};
                for r in candidates {
                    if !occupied.iter().any(|&b|overlaps(r,b,6.0)) && !paths.iter().chain(&leaders).any(|p|p.windows(2).any(|s|segment_hits(s[0],s[1],[r[0]-2.0,r[1]-2.0,w+4.0,h+4.0]))) {chosen=Some(r);break 'search;}
                }
            }}
            let r=chosen.unwrap_or_else(|| {
                // Nearby candidates from earlier edges may extend into the
                // shelf too. Jump below every intersecting rectangle.
                for _ in 0..=occupied.len() {
                    let r=[shelf_x,shelf_y,w,h];
                    let next=occupied.iter().filter(|&&b|overlaps(r,b,6.0)).map(|b|b[1]+b[3]+12.0).fold(shelf_y,f32::max);
                    if next==shelf_y {break;}shelf_y=next;
                }
                let r=[shelf_x,shelf_y,w,h];shelf_y+=h+12.0;r
            });
            occupied.push(r);
            if chosen.is_none() {
                let a=path[1];let b=(r[0],r[1]+h*0.5);
                let leader=route(vec![a,(b.0,a.1),b],&occupied);
                for s in leader.windows(2) {stroke_ink(&mut items,s[0],s[1],ink);}leaders.push(leader);
            }
            rect(&mut items,r,Ink::Surface,2.0);
            for (i,(text,tw)) in wrapped.into_iter().enumerate() {items.push(Item::Text{x:r[0]+(w-tw)*0.5,y:r[1]+4.0+i as f32*16.0,width:tw.max(1.0),text,size:11.0,bold:false,ink});}
        }
        for (i,node) in nodes.iter().enumerate() {
            let [x,y,nw,nh]=boxes[i];
            let color=custom_color(node.as_object().unwrap());
            let outline=color.map(Ink::Custom).unwrap_or(Ink::Border);let fill=color.map(Ink::CustomFill).unwrap_or(Ink::Surface);
            if node["shape"].as_str()==Some("diamond") {
                let points=vec![(x+nw*0.5,y),(x+nw,y+nh*0.5),(x+nw*0.5,y+nh),(x,y+nh*0.5)];
                items.push(Item::Shape{points:points.clone(),ink:fill,alpha:1.0});
                for j in 0..4 {stroke_ink(&mut items,points[j],points[(j+1)%4],color.map(Ink::Custom).unwrap_or(Ink::Muted));}
            }else{
                let radius=if node["shape"].as_str()==Some("rect") {0.0}else{8.0};
                rect(&mut items,[x,y,nw,nh],outline,radius);rect(&mut items,[x+1.0,y+1.0,nw-2.0,nh-2.0],fill,(radius-1.0_f32).max(0.0));
            }
            for (j,(text,w)) in labels[i].iter().enumerate() {items.push(Item::Text{x:x+(nw-w)*0.5,y:y+(nh-(labels[i].len() as f32*18.0-5.0))*0.5+j as f32*18.0,width:(*w).max(1.0),text:text.clone(),size:13.0,bold:false,ink:Ink::Foreground});}
        }
        // Include edge-label extents (including many explicit newlines) in fit.
        let mut min_x=0.0_f32;let mut min_y=0.0_f32;
        for item in &items {match item {Item::Rect{rect,..}=>{min_x=min_x.min(rect[0]);min_y=min_y.min(rect[1]);},Item::Text{x,y,..}=>{min_x=min_x.min(*x);min_y=min_y.min(*y);},Item::Shape{points,..}=>for &(x,y) in points {min_x=min_x.min(x);min_y=min_y.min(y);},Item::Diagram(_)=>unreachable!()}}
        let (sx,sy)=(4.0-min_x,4.0-min_y);
        let mut width=width+sx;let mut height=height+sy;
        for item in &mut items {match item {
            Item::Rect{rect,..}=>{rect[0]+=sx;rect[1]+=sy;width=width.max(rect[0]+rect[2]+4.0);height=height.max(rect[1]+rect[3]+4.0);}
            Item::Text{x,y,width:w,size,..}=>{*x+=sx;*y+=sy;width=width.max(*x+*w+4.0);height=height.max(*y+*size+9.0);}
            Item::Shape{points,..}=>for (x,y) in points {*x+=sx;*y+=sy;width=width.max(*x+4.0);height=height.max(*y+4.0);},
            Item::Diagram(_)=>unreachable!(),
        }}
        Self{rect:viewport,transform:Transform::default(),graph:Arc::new(Geometry{items,width,height})}
    }
}

pub(super) fn paint(paint:&mut dyn crate::primitives::paint::Paint,c:&Canvas,x:f32,y:f32,theme:&IdeTheme,s:f32,clip:[f32;4],occlusion:&[[f32;4]]) {
    let r=[x+c.rect[0],y+c.rect[1],c.rect[2],c.rect[3]];
    let Some(clip)=intersect_rect(clip,r) else{return;};
    let mut items=vec![];
    rect(&mut items,[0.0,0.0,r[2],r[3]],Ink::Border,6.0*s);
    rect(&mut items,[s,s,(r[2]-2.0*s).max(0.0),(r[3]-2.0*s).max(0.0)],Ink::Surface,5.0*s);
    // At most 1280 dots, independent of graph size, zoom and pan distance.
    let spacing=(20.0*s).max(r[2]/64.0).max(r[3]/20.0);
    for row in 1..20 {for col in 1..64 {let px=col as f32*spacing;let py=row as f32*spacing;if px<r[2]-s && py<r[3]-s {rect(&mut items,[px,py,s,s],Ink::Border,0.5*s);}}}
    let fit=((r[2]-32.0*s).max(s)/c.graph.width).min((r[3]-32.0*s).max(s)/c.graph.height);
    let k=fit*c.transform.zoom;
    let ox=(r[2]-c.graph.width*k)*0.5+c.transform.pan_x*s;let oy=(r[3]-c.graph.height*k)*0.5+c.transform.pan_y*s;
    for item in &c.graph.items {
        items.push(match item {
            Item::Rect{rect,ink,radius}=>Item::Rect{rect:[ox+rect[0]*k,oy+rect[1]*k,rect[2]*k,rect[3]*k],ink:*ink,radius:radius*k},
            Item::Shape{points,ink,alpha}=>Item::Shape{points:points.iter().map(|p|(ox+p.0*k,oy+p.1*k)).collect(),ink:*ink,alpha:*alpha},
            Item::Text{x,y,width,text,size,bold,ink}=>Item::Text{x:ox+x*k,y:oy+y*k,width:width*k,text:text.clone(),size:size*k/s,bold:*bold,ink:*ink},
            Item::Diagram(_)=>unreachable!(),
        });
    }
    let layout=NativeLayout{height:r[3],width:r[2],items,prepared:None};
    super::render(paint,&layout,r[0],r[1],theme,s,clip,occlusion);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::paint_record::{PaintCommand,RecordingPainter};
    fn props()->Value {json!({"nodes":[{"id":"a","label":"Alpha"},{"id":"b","label":"Beta","shape":"diamond"},{"id":"c","label":"Disconnected"}],"edges":[{"from":"a","to":"b","label":"next"}]})}
    fn accepted(p:&Value)->bool {parse(&json!({"root":"g","elements":{"g":{"type":"Diagram","props":p}}}).to_string()).is_some()}
    fn canvas(p:&Value)->Canvas {Canvas::new(p.as_object().unwrap(),[0.0,0.0,400.0,320.0],1.0,&mut |t,size,_|t.chars().count() as f32*size*0.5)}
    fn assert_geometry(c:&Canvas,expected_labels:usize) {
        let mut labels=vec![];let mut nodes=vec![];
        for item in &c.graph.items {
            let check=|x:f32,y:f32|assert!(x.is_finite()&&y.is_finite()&&x>=0.0&&y>=0.0&&x<=c.graph.width&&y<=c.graph.height,"out of bounds: {x},{y}");
            match item {
                Item::Rect{rect,ink,radius}=>{
                    check(rect[0],rect[1]);check(rect[0]+rect[2],rect[1]+rect[3]);
                    if matches!(ink,Ink::Surface)&&*radius==2.0 {labels.push(*rect);}
                    if matches!(ink,Ink::Border) {nodes.push(*rect);}
                },
                Item::Shape{points,ink,..}=>{
                    for &(x,y) in points {check(x,y);}
                    if matches!(ink,Ink::Surface) {
                        let x=points.iter().map(|p|p.0).fold(f32::INFINITY,f32::min);let y=points.iter().map(|p|p.1).fold(f32::INFINITY,f32::min);
                        nodes.push([x,y,points.iter().map(|p|p.0).fold(0.0,f32::max)-x,points.iter().map(|p|p.1).fold(0.0,f32::max)-y]);
                    }
                },
                Item::Text{x,y,width,size,..}=>{check(*x,*y);check(x+width,y+size);},Item::Diagram(_)=>unreachable!(),
            }
        }
        assert_eq!(labels.len(),expected_labels);
        for (i,&a) in labels.iter().enumerate() {
            for &b in &labels[..i] {assert!(!overlaps(a,b,5.9),"label collision: {a:?} {b:?}");}
            for &b in &nodes {assert!(!overlaps(a,b,5.9),"label/node collision: {a:?} {b:?}");}
        }
        for item in &c.graph.items {if let Item::Text{x,y,width,size,ink:Ink::Muted,..}=item {
            assert!(labels.iter().any(|r|*x>=r[0]&&*y>=r[1]&&x+width<=r[0]+r[2]+0.001&&y+size<=r[1]+r[3]),"edge text must fit its padded label rectangle");
        }}
    }
    #[test]
    fn json_render_diagram_word_wrap_and_centering() {
        let text="neoism-workspace-daemon\nPersistent workspace and session owner";
        let wrapped=lines(text,168.0,13.0,&mut |s,size,_|s.chars().count() as f32*size*0.5);
        assert_eq!(wrapped.iter().map(|(s,_)|s.as_str()).collect::<Vec<_>>(),vec!["neoism-workspace-daemon","Persistent workspace and","session owner"]);
        let mut measured=|s:&str,size:f32,_|s.graphemes(true).count() as f32*size;
        let long=lines("abcdefghij\n\nword pair",30.0,10.0,&mut measured);
        assert_eq!(long.iter().map(|(s,_)|s.as_str()).collect::<Vec<_>>(),vec!["abc","def","ghi","j","","wor","d","pai","r"]);
        let p=json!({"nodes":[{"id":"a","label":text}],"edges":[]});let c=canvas(&p);
        let centers:Vec<_>=c.graph.items.iter().filter_map(|it|if let Item::Text{x,width,..}=it {Some(x+width*0.5)}else{None}).collect();
        assert!(centers.iter().all(|x|(x-centers[0]).abs()<0.001));
    }
    #[test]
    fn json_render_diagram_branch_merge_cycle_label_rectangles() {
        let mut p=json!({"nodes":[{"id":"a","label":"neoism-workspace-daemon\nPersistent workspace and session owner"},{"id":"b","label":"neoism-protocol"},{"id":"c","label":"Transport adapter","shape":"diamond"},{"id":"d","label":"Merged destination"}],"edges":[{"from":"a","to":"b","label":"Transport contracts"},{"from":"a","to":"c","label":"Transport contracts"},{"from":"b","to":"d","label":"A longer multiword transport contract used by merging paths"},{"from":"c","to":"d","label":"Transport contracts"},{"from":"a","to":"d","label":"A long edge spanning multiple ranks"}]});
        for cyclic in [false,true] {
            if cyclic {p["edges"].as_array_mut().unwrap().extend([json!({"from":"b","to":"a","label":"Transport contracts"}),json!({"from":"a","to":"a","label":"A self loop with a long repeated contract label"})]);}
            for direction in ["TB","LR"] {
                p["direction"]=json!(direction);let c=canvas(&p);assert_geometry(&c,p["edges"].as_array().unwrap().len());
                assert_eq!(format!("{:?}",c.graph),format!("{:?}",canvas(&p).graph));
                for zoom in [0.25,1.0,4.0] {
                    let mut c=c.clone();c.transform=Transform{zoom,pan_x:-37.0,pan_y:23.0};
                    let mut painter=RecordingPainter::new("monospace",994,vec![]);let clip=[50.0,70.0,160.0,120.0];
                    paint(&mut painter,&c,10.0,20.0,&IdeTheme::default(),1.0,clip,&[]);
                    for command in painter.commands() {match command {
                        PaintCommand::Polygon{points,..}=>assert!(points.iter().all(|&(x,y)|x>=50.0&&y>=70.0&&x<=210.0&&y<=190.0)),
                        PaintCommand::Text{clip,..}|PaintCommand::Rect{clip,..}=>{let r=clip.expect("diagram primitives must be clipped");assert!(r[0]>=50.0&&r[1]>=70.0&&r[0]+r[2]<=210.0&&r[1]+r[3]<=190.0);},_=>{},
                    }}
                }
            }
        }
        let dense=json!({"nodes":(0..32).map(|i|json!({"id":format!("n{i}"),"label":"Node with multiple words","shape":"diamond"})).collect::<Vec<_>>(),"edges":(0..64).map(|i|json!({"from":format!("n{}",i/32),"to":format!("n{}",i%32),"label":"Repeated label with multiple words and a long contract description"})).collect::<Vec<_>>()});
        for direction in ["TB","LR"] {let mut p=dense.clone();p["direction"]=json!(direction);assert_geometry(&canvas(&p),64);}
    }
    #[test]
    fn json_render_diagram_long_edge_routes_around_node_interiors() {
        let nodes=[[0.0,0.0,100.0,80.0],[160.0,0.0,100.0,80.0],[320.0,0.0,100.0,80.0]];
        let path=route(vec![(100.0,40.0),(210.0,40.0),(320.0,40.0)],&nodes);
        assert_eq!(path[0],(100.0,40.0));assert_eq!(*path.last().unwrap(),(320.0,40.0));
        for s in path.windows(2) {assert!(s[0].0==s[1].0||s[0].1==s[1].1);assert!(!nodes.iter().any(|&r|segment_hits(s[0],s[1],r)));}
    }
    #[test]
    fn json_render_diagram_literal_node_and_edge_colors_are_cached() {
        let mut p=props();
        for parent in ["nodes","edges"] {
            for bad in [Value::Null,json!(3),json!("red"),json!("#123"),json!("#12345678"),json!("#12gg56")] {
                let mut invalid=p.clone();invalid[parent][0]["color"]=bad;assert!(!accepted(&invalid));
            }
        }
        p["nodes"][0]["color"]=json!("#123456");p["nodes"][1]["color"]=json!("#abcdef");p["edges"][0]["color"]=json!("#987654");
        assert!(accepted(&p));let c=canvas(&p);
        assert!(c.graph.items.iter().any(|it|matches!(it,Item::Rect{ink:Ink::Custom(0x123456),..})));
        assert!(c.graph.items.iter().any(|it|matches!(it,Item::Rect{ink:Ink::CustomFill(0x123456),..})));
        assert!(c.graph.items.iter().any(|it|matches!(it,Item::Shape{ink:Ink::CustomFill(0xabcdef),..})));
        assert!(c.graph.items.iter().any(|it|matches!(it,Item::Shape{ink:Ink::Custom(0xabcdef),..})));
        assert!(c.graph.items.iter().filter(|it|matches!(it,Item::Shape{ink:Ink::Custom(0x987654),..})).count()>=2);
        assert!(c.graph.items.iter().any(|it|matches!(it,Item::Text{text,ink:Ink::Custom(0x987654),..} if text=="next")));
        assert!(c.graph.items.iter().filter(|it|matches!(it,Item::Text{size,ink:Ink::Foreground,..} if *size==13.0)).count()>=3);
        let mut transformed=c.clone();transformed.transform=transformed.transform.event(Some(&json!("zoom-in"))).unwrap();assert!(Arc::ptr_eq(&c.graph,&transformed.graph));
    }
    #[test]
    fn json_render_diagram_strict_contract() {
        assert!(accepted(&props()));
        for key in ["title","direction","nodes","edges"] {let mut p=props();p[key]=Value::Null;assert!(!accepted(&p),"{key}");}
        for (key,v) in [("title",json!(42)),("direction",json!("RL")),("onClick",json!("go")),("nodes",json!([])),("nodes",json!(vec![json!({"id":"a","label":"x"});33])),("edges",json!(vec![json!({"from":"a","to":"b"});65]))] {let mut p=props();p[key]=v;assert!(!accepted(&p));}
        for id in ["", "a.b", "🦀", "a b", &"x".repeat(65)] {let mut p=props();p["nodes"][0]["id"]=json!(id);assert!(!accepted(&p));}
        for field in ["id","label","shape"] {let mut p=props();p["nodes"][0][field]=Value::Null;assert!(!accepted(&p));}
        for label in ["".to_owned(),"🦀".repeat(161)] {let mut p=props();p["nodes"][0]["label"]=json!(label);assert!(!accepted(&p));}
        let mut p=props();p["nodes"][0]["label"]=json!("🦀".repeat(160));assert!(accepted(&p));
        for field in ["from","to","label"] {let mut p=props();p["edges"][0][field]=Value::Null;assert!(!accepted(&p));}
        let mut p=props();p["edges"][0]["label"]=json!("🦀".repeat(80));assert!(accepted(&p));p["edges"][0]["label"]=json!("🦀".repeat(81));assert!(!accepted(&p));
        for (path,v) in [("/nodes/1/id",json!("a")),("/nodes/0/extra",json!(true)),("/nodes/0/shape",json!("circle")),("/edges/0/to",json!("absent")),("/edges/0/extra",json!(false))] {
            let mut p=props();let (parent,key)=path.rsplit_once('/').unwrap();p.pointer_mut(parent).unwrap()[key]=v;assert!(!accepted(&p));
        }
        let mut p=props();p["edges"].as_array_mut().unwrap().push(json!({"from":"a","to":"b","label":"different"}));assert!(!accepted(&p));
        let mut v=json!({"root":"g","elements":{"g":{"type":"Diagram","props":props(),"children":[]}}});assert!(parse(&v.to_string()).is_none());v["elements"]["g"]["children"]=Value::Null;assert!(parse(&v.to_string()).is_none());
        let mut p=props();p["nodes"]=json!((0..32).map(|i|json!({"id":format!("n{i}"),"label":"node"})).collect::<Vec<_>>());p["edges"]=json!((0..64).map(|i|json!({"from":format!("n{}",i/32),"to":format!("n{}",i%32)})).collect::<Vec<_>>());assert!(accepted(&p));
    }
    #[test]
    fn json_render_diagram_deterministic_direction_cycles_disconnected_and_bounds() {
        let mut p=props();
        let point=|c:&Canvas,label:&str|c.graph.items.iter().find_map(|it|if let Item::Text{x,y,text,..}=it {if text==label {Some((*x,*y))}else{None}}else{None}).unwrap();
        let lr=canvas(&p);assert!(point(&lr,"Beta").0>point(&lr,"Alpha").0);assert_eq!(point(&lr,"Beta"),point(&canvas(&p),"Beta"));
        assert!(lr.graph.width<500.0,"a short diamond label must not double every node's width");
        let diamond_y=lr.graph.items.iter().find_map(|item|if let Item::Shape{points,ink:Ink::Surface,..}=item {Some(points.iter().map(|p|p.1).sum::<f32>()/points.len() as f32)}else{None}).unwrap();
        assert!((point(&lr,"Beta").1+6.5-diamond_y).abs()<0.01,"node text must be vertically centered");
        p["direction"]=json!("TB");let tb=canvas(&p);assert!(point(&tb,"Beta").1>point(&tb,"Alpha").1);
        p["edges"].as_array_mut().unwrap().extend([json!({"from":"b","to":"a"}),json!({"from":"a","to":"a","label":"loop"})]);assert!(accepted(&p));
        for direction in ["LR","TB"] {p["direction"]=json!(direction);let c=canvas(&p);assert!(c.graph.items.len()<1000);assert!(c.graph.width<100000.0 && c.graph.height<100000.0);
            for item in &c.graph.items {let check=|x:f32,y:f32|assert!(x.is_finite()&&y.is_finite()&&x>=0.0&&y>=0.0&&x<=c.graph.width&&y<=c.graph.height,"{x},{y}");match item {
                Item::Rect{rect,..}=>{check(rect[0],rect[1]);check(rect[0]+rect[2],rect[1]+rect[3]);},
                Item::Text{x,y,width,size,..}=>{check(*x,*y);check(x+width,y+size);},
                Item::Shape{points,..}=>for &(x,y) in points {check(x,y);},Item::Diagram(_)=>unreachable!(),
            }}
        }
    }
    #[test]
    fn json_render_diagram_cycle_labels_stay_away_from_outgoing_labels() {
        let p=json!({"nodes":[{"id":"a","label":"Request"},{"id":"b","label":"Review","shape":"diamond"},{"id":"c","label":"Agent"}],"edges":[{"from":"a","to":"b"},{"from":"b","to":"a","label":"Revise"},{"from":"b","to":"c","label":"Approve"}]});
        for direction in ["LR","TB"] {
            let mut p=p.clone();p["direction"]=json!(direction);let c=canvas(&p);
            let bounds=|label:&str|c.graph.items.iter().find_map(|it|if let Item::Text{x,y,width,text,..}=it {if text==label {Some([*x,*y,*width,16.0])}else{None}}else{None}).unwrap();
            assert!(intersect_rect(bounds("Revise"),bounds("Approve")).is_none());
        }
    }
    #[test]
    fn json_render_diagram_canvas_clipping_transform_and_cache() {
        let mut c=canvas(&props());let cached=c.graph.clone();
        for command in [json!("zoom-in"),json!({"dx":-70,"dy":25}),json!("zoom-out"),json!("fit")] {
            c.transform=c.transform.event(Some(&command)).unwrap();assert!(Arc::ptr_eq(&cached,&c.graph));
            let mut painter=RecordingPainter::new("monospace",992,vec![]);
            let bounds=[50.0,70.0,160.0,120.0];paint(&mut painter,&c,10.0,20.0,&IdeTheme::default(),1.0,bounds,&[]);
            for command in painter.commands() {match command {
                PaintCommand::Polygon{points,..}=>assert!(points.iter().all(|&(x,y)|x>=bounds[0]&&y>=bounds[1]&&x<=bounds[0]+bounds[2]&&y<=bounds[1]+bounds[3])),
                PaintCommand::Text{clip,..}|PaintCommand::Rect{clip,..}=>{let clip=clip.expect("canvas primitive must be clipped");assert!(clip[0]>=bounds[0]&&clip[1]>=bounds[1]&&clip[0]+clip[2]<=bounds[0]+bounds[2]&&clip[1]+clip[3]<=bounds[1]+bounds[3]);},
                _=>{},
            }}
            let mut hidden=RecordingPainter::new("monospace",993,vec![]);paint(&mut hidden,&c,10.0,20.0,&IdeTheme::default(),1.0,[0.0,0.0,500.0,500.0],&[[0.0,0.0,500.0,500.0]]);assert!(hidden.commands().is_empty());
        }
        assert_eq!(c.transform,Transform::default());
        let mut t=Transform::default();for _ in 0..100 {t=t.event(Some(&json!({"dx":1e300,"dy":-1e300}))).unwrap();t=t.event(Some(&json!("zoom-in"))).unwrap();}
        assert_eq!(t.value(),json!({"zoom":4.0,"panX":8192.0,"panY":-8192.0}));
        for v in [Value::Null,json!(42),json!({"dx":1}),json!({"dx":1,"dy":2,"extra":0}),json!({"dx":"1","dy":0}),json!("send")] {assert!(t.event(Some(&v)).is_err());}assert!(t.event(None).is_err());
        for _ in 0..100 {t=t.event(Some(&json!("zoom-out"))).unwrap();}assert_eq!(t.zoom,0.25);
    }

    #[test]
    fn json_render_diagram_maximum_graph_and_multiline_geometry_are_finite() {
        let p=json!({"direction":"TB","nodes":(0..32).map(|i|json!({"id":format!("n{i}"),"label":"\n".repeat(160),"shape":"diamond"})).collect::<Vec<_>>(),"edges":(0..64).map(|i|json!({"from":format!("n{}",i/32),"to":format!("n{}",i%32),"label":"\n".repeat(80)})).collect::<Vec<_>>()});
        assert!(accepted(&p));let c=canvas(&p);
        assert!(c.graph.width.is_finite()&&c.graph.height.is_finite()&&c.graph.width<1_000_000.0&&c.graph.height<1_000_000.0);
        assert!(c.graph.items.len()<12000);
        for item in &c.graph.items {match item {
            Item::Text{x,y,width,size,..}=>assert!(x.is_finite()&&y.is_finite()&&width.is_finite()&&size.is_finite()&&*x>=0.0&&*y>=0.0&&x+width<=c.graph.width&&y+size<=c.graph.height),
            Item::Shape{points,..}=>assert!(points.iter().all(|p|p.0.is_finite()&&p.1.is_finite())),
            Item::Rect{rect,..}=>assert!(rect.iter().all(|n|n.is_finite())),Item::Diagram(_)=>unreachable!(),
        }}
        let bad_metrics=Canvas::new(props().as_object().unwrap(),[0.0,0.0,8.0,320.0],1.0,&mut |_,_,_|f32::NAN);
        assert!(bad_metrics.graph.width.is_finite()&&bad_metrics.graph.height.is_finite());
    }
}