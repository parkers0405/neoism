//! Encode the original painter straight into GPUIX's canvas wire format.
//! JavaScript only leases images and forwards the command string; it need not
//! allocate, sort, translate and stringify every primitive on every frame.
use neoism_ui::primitives::paint_record::{PaintCommand, PaintRaster};
use serde::Serialize;
use std::{cmp::Ordering, collections::HashSet};

#[derive(Serialize)]
pub struct CanvasImage { source: u8, raster: PaintRaster }
#[derive(Serialize)]
pub struct CanvasPacket { pub commands: String, pub images: Vec<CanvasImage> }
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
fn paint_order(command:&PaintCommand)->(u8,Option<f32>,Option<u8>) {
    match command {
        PaintCommand::Rect{layer,depth,order,..}|PaintCommand::Ring{layer,depth,order,..}=>(*layer,Some(*depth),Some(*order)),
        PaintCommand::Polygon{layer,depth,..}|PaintCommand::Image{layer,depth,..}=>(*layer,Some(*depth),None),
        PaintCommand::Text{layer,..}=>(*layer,None,None),
    }
}
fn color(c:&[f32;4])->u32 { u32::from_be_bytes(c.map(|v| (v as f64*255.0).round().clamp(0.0,255.0) as u8)) }
fn points(p:&[(f32,f32)])->Vec<f32> {p.iter().flat_map(|&(x,y)|[x,y]).collect()}
fn image_key(source:u8,raster:&PaintRaster,seen:&mut HashSet<u64>,images:&mut Vec<CanvasImage>)->u64 {
    let key=raster.id*8+source as u64;
    if seen.insert(key) {images.push(CanvasImage{source,raster:raster.clone()});}
    key
}
pub fn encode(commands:&[PaintCommand],font:&str,weight:u16)->CanvasPacket {
    let mut sorted:Vec<_>=commands.iter().collect();
    sorted.sort_by(|a,b| {
        let (al,ad,ao)=paint_order(a);let (bl,bd,bo)=paint_order(b);
        al.cmp(&bl).then_with(||ad.zip(bd).and_then(|(a,b)|a.partial_cmp(&b)).unwrap_or(Ordering::Equal))
            .then_with(||ao.zip(bo).map(|(a,b)|a.cmp(&b)).unwrap_or(Ordering::Equal))
    });
    let mut ops=Vec::with_capacity(commands.len());let mut images=Vec::new();let mut seen=HashSet::new();let mut clip=None;
    for command in sorted {
        let next=match command {PaintCommand::Rect{clip,..}|PaintCommand::Text{clip,..}=>*clip,_=>None};
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
    CanvasPacket{commands:serde_json::to_string(&ops).expect("finite canvas commands"),images}
}
