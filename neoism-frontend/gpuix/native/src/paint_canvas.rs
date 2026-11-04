//! Encode the original painter straight into GPUIX's canvas wire format.
//! JavaScript only leases images and forwards the command string; it need not
//! allocate, sort, translate and stringify every primitive on every frame.
use neoism_ui::primitives::paint_record::{PaintCommand, PaintRaster};
use serde::Serialize;
use std::{cmp::Ordering, collections::HashSet};

#[derive(Serialize)]
pub struct CanvasImage { source: u8, raster: PaintRaster }
#[derive(Serialize)]
pub struct CanvasPacket { pub commands: String, pub images: Vec<CanvasImage>, #[serde(skip_serializing_if="Option::is_none")] pub chunks: Option<Vec<String>> }
#[derive(Serialize)]
#[serde(tag="k")]
enum Op<'a> {
    #[serde(rename="r")] Rect { b: &'a [f32;4], c:u32, r: &'a [f32;4] },
    #[serde(rename="i")] Image { id:u64, b:[f32;4], #[serde(skip_serializing_if="Option::is_none")] s:Option<[f32;4]> },
    #[serde(rename="p")] Path { c:u32, p:Vec<Vec<f32>> },
    #[serde(rename="t")] Text { x:f32,y:f32,t:&'a str,s:f32,c:u32,f:&'a str,w:u16,i:bool,lh:f32 },
    #[serde(rename="cl")] Clip { b:[f32;4] },
    #[serde(rename="u")] Unclip,
}
fn paint_order(command:&PaintCommand)->(u8,bool,f32,u8) {
    let (layer,depth,order)=match command {
        PaintCommand::Rect{layer,depth,order,..}|PaintCommand::Ring{layer,depth,order,..}=>(*layer,*depth,*order),
        PaintCommand::Polygon{layer,depth,..}|PaintCommand::Image{layer,depth,..}=>(*layer,*depth,0),
        PaintCommand::Text{layer,..}=>(*layer,0.0,0),
    };
    // Missing keys cannot compare equal to every value: that breaks transitivity.
    // Text finishes its layer; unordered shapes use order zero. Match PaintLayer.tsx.
    (layer,matches!(command,PaintCommand::Text{..}),if depth.is_finite() && depth!=0.0 {depth}else{0.0},order)
}
fn compare_paint(a:&PaintCommand,b:&PaintCommand)->Ordering {
    let (al,at,ad,ao)=paint_order(a);let (bl,bt,bd,bo)=paint_order(b);
    al.cmp(&bl).then_with(||at.cmp(&bt)).then_with(||ad.total_cmp(&bd)).then_with(||ao.cmp(&bo))
}
fn color(c:&[f32;4])->u32 { u32::from_be_bytes(c.map(|v| (v as f64*255.0).round().clamp(0.0,255.0) as u8)) }
fn points(p:&[(f32,f32)])->Vec<f32> {p.iter().flat_map(|&(x,y)|[x,y]).collect()}
fn image_key(source:u8,raster:&PaintRaster,seen:&mut HashSet<u64>,images:&mut Vec<CanvasImage>)->u64 {
    let key=raster.id*8+source as u64;
    if seen.insert(key) {images.push(CanvasImage{source,raster:raster.clone()});}
    key
}
pub fn encode(commands:&[PaintCommand],font:&str,weight:u16)->CanvasPacket {
    encode_inner(commands,font,weight,false)
}
pub fn encode_chunked(commands:&[PaintCommand],font:&str,weight:u16)->CanvasPacket {
    encode_inner(commands,font,weight,true)
}
fn encode_inner(commands:&[PaintCommand],font:&str,weight:u16,chunked:bool)->CanvasPacket {
    let mut sorted:Vec<_>=commands.iter().collect();
    sorted.sort_by(|a,b|compare_paint(a,b));
    let mut ops=Vec::with_capacity(commands.len());let mut images=Vec::new();let mut seen=HashSet::new();let mut clip=None;
    for command in sorted {
        let next=match command {PaintCommand::Rect{clip,..}|PaintCommand::Ring{clip,..}|PaintCommand::Text{clip,..}=>*clip,_=>None};
        if clip!=next {if clip.is_some(){ops.push(Op::Unclip);}if let Some(b)=next{ops.push(Op::Clip{b});}clip=next;}
        match command {
            PaintCommand::Rect{rect,color:c,radii,..}=>ops.push(Op::Rect{b:rect,c:color(c),r:radii}),
            PaintCommand::Ring{outer,inner,color:c,..}=>ops.push(Op::Path{c:color(c),p:vec![points(outer),points(inner)]}),
            PaintCommand::Polygon{points:p,color:c,..}=>ops.push(Op::Path{c:color(c),p:vec![points(p)]}),
            PaintCommand::Image{rect,source,raster,..}=>ops.push(Op::Image{id:image_key(4,raster,&mut seen,&mut images),b:*rect,s:if *source==[0.0,0.0,1.0,1.0]{None}else{Some(*source)}}),
            PaintCommand::Text{x,y,raster:Some(raster),..}=>{
                let scale=if raster.scale==0.0{1.0}else{raster.scale};
                ops.push(Op::Image{id:image_key(2,raster,&mut seen,&mut images),b:[x+raster.dx,y+raster.dy,raster.width as f32/scale,raster.height as f32/scale],s:None});
            }
            PaintCommand::Text{text,..} if text.bytes().all(|byte| matches!(byte,b' '|b'\t'|b'\r'|b'\n')) => {},
            PaintCommand::Text{x,y,text,size,color:c,bold,italic,family,..}=>ops.push(Op::Text{x:*x,y:*y,t:text,s:*size,c:u32::from_be_bytes(*c),f:family.as_deref().unwrap_or(font),w:if *bold{800}else{weight},i:*italic,lh:*size}),
        }
    }
    if clip.is_some(){ops.push(Op::Unclip);}
    if !chunked {return CanvasPacket{commands:serde_json::to_string(&ops).expect("finite canvas commands"),images,chunks:None};}
    // Keep unchanged transcript sections as independent retained canvases.
    // A pulsing activity word or appended token must not invalidate every
    // image and table above it. Sort globally first; each chunk restores its
    // inherited clip and closes it so canvases remain independently paintable.
    let mut chunks=Vec::new();let mut active_clip=None;
    for chunk in ops.chunks(64) {
        let inherited=active_clip.map(|b|Op::Clip{b});
        for op in chunk {match op {Op::Clip{b}=>active_clip=Some(*b),Op::Unclip=>active_clip=None,_=>{}}}
        let close=active_clip.map(|_|Op::Unclip);
        let framed:Vec<_>=inherited.iter().chain(chunk.iter()).chain(close.iter()).collect();
        chunks.push(serde_json::to_string(&framed).expect("finite canvas commands"));
    }
    CanvasPacket{commands:String::new(),images,chunks:Some(chunks)}
}

#[cfg(test)]
mod tests {
    use super::*;
    use neoism_ui::primitives::{paint::Paint, paint_record::RecordingPainter};

    fn rect(x:f32,depth:f32,order:u8,layer:u8)->PaintCommand {
        PaintCommand::Rect { rect:[x,0.0,1.0,1.0],color:[1.0;4],radii:[0.0;4],clip:None,depth,order,layer }
    }

    #[test]
    fn mixed_primitives_have_consistent_order_for_every_input_permutation() {
        let commands=[
            rect(20.0,1.0,20,3),
            PaintCommand::Polygon { points:vec![(0.0,0.0),(1.0,1.0),(0.0,1.0)],color:[1.0;4],depth:1.0,layer:3 },
            rect(10.0,1.0,10,3),
            PaintCommand::Text { x:99.0,y:0.0,text:"overlay".into(),size:12.0,width:40.0,color:[255;4],bold:false,italic:false,extrude:false,family:None,clip:None,layer:3,raster:None },
        ];
        for a in 0..4 { for b in 0..4 { for c in 0..4 { for d in 0..4 {
            if a==b||a==c||a==d||b==c||b==d||c==d { continue; }
            let input=[a,b,c,d].map(|i|commands[i].clone());
            for chunked in [false,true] {
                let packet=encode_inner(&input,"monospace",400,chunked);
                let encoded=if chunked { &packet.chunks.as_ref().unwrap()[0] } else { &packet.commands };
                let ops:serde_json::Value=serde_json::from_str(encoded).unwrap();
                assert_eq!(ops[0]["k"],"p");
                assert_eq!(ops[1]["b"][0],10.0);
                assert_eq!(ops[2]["b"][0],20.0);
                assert_eq!(ops[3]["t"],"overlay");
            }
        }}}}
    }

    #[test]
    fn paint_order_is_transitive_and_equal_keys_keep_recording_order() {
        let mut commands=vec![
            PaintCommand::Polygon { points:vec![],color:[1.0;4],depth:0.0,layer:3 },
            PaintCommand::Text { x:0.0,y:0.0,text:"text".into(),size:12.0,width:20.0,color:[255;4],bold:false,italic:false,extrude:false,family:None,clip:None,layer:3,raster:None },
        ];
        for depth in [-1.0,-0.0,0.0,1.0,f32::NAN,f32::INFINITY,f32::NEG_INFINITY] {
            for order in [0,10,20] { commands.push(rect(0.0,depth,order,3)); }
        }
        commands.push(rect(0.0,999.0,20,0));
        commands.push(rect(0.0,-999.0,0,4));
        for a in &commands { for b in &commands {
            assert_eq!(compare_paint(a,b),compare_paint(b,a).reverse());
            for c in &commands {
                if compare_paint(a,b)!=Ordering::Greater && compare_paint(b,c)!=Ordering::Greater {
                    assert_ne!(compare_paint(a,c),Ordering::Greater);
                }
            }
        }}
        let equal:Vec<_>=(0..160).map(|i|rect(i as f32,if i%2==0 {-0.0}else{f32::NAN},10,0)).collect();
        for start in [0,13,31,64] {
            for chunked in [false,true] {
                let packet=encode_inner(&equal[start..],"monospace",400,chunked);
                let chunks=packet.chunks.unwrap_or_else(||vec![packet.commands]);
                let ops:Vec<serde_json::Value>=chunks.iter().flat_map(|chunk|serde_json::from_str::<Vec<serde_json::Value>>(chunk).unwrap()).collect();
                for (i,op) in ops.iter().enumerate() { assert_eq!(op["b"][0],(start+i) as f32); }
            }
        }
    }

    #[test]
    fn clipped_ring_stays_clipped_in_direct_and_chunked_canvas_packets() {
        let mut painter = RecordingPainter::new("monospace", 400, vec![]);
        let clip = [14.0, 20.0, 6.0, 6.0];
        painter.ring_clipped([20.0, 20.0], 5.0, 2.0, [1.0; 4], 0.0, 20, clip);
        painter.rect(None, 0.0, 0.0, 2.0, 2.0, [1.0; 4], 0.0, 21);
        for chunked in [false, true] {
            let packet = encode_inner(&painter.commands(), "monospace", 400, chunked);
            let encoded = if chunked { &packet.chunks.as_ref().unwrap()[0] } else { &packet.commands };
            let ops: serde_json::Value = serde_json::from_str(encoded).unwrap();
            assert_eq!(ops[0], serde_json::json!({"k":"cl", "b":clip}));
            assert_eq!(ops[1]["k"], "p");
            assert_eq!(ops[2]["k"], "u");
            assert_eq!(ops[3]["k"], "r");
        }
    }
}
