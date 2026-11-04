use super::{Ink, Item, NativeLayout, Value};

// Clip actual path geometry so charts never bleed through chrome. Keep each
// band/stroke continuous: separately antialiased triangles leave visible seams.
pub(super) fn clip_polygon(points: &[(f32,f32)], clip: [f32;4]) -> Vec<(f32,f32)> {
    let mut points=points.to_vec();
    for (axis,edge,lower) in [(0,clip[0],true),(0,clip[0]+clip[2],false),(1,clip[1],true),(1,clip[1]+clip[3],false)] {
        let coordinate=|p:(f32,f32)|if axis==0 {p.0} else {p.1};
        let inside=|p:(f32,f32)|if lower {coordinate(p)>=edge} else {coordinate(p)<=edge};
        let mut next=Vec::new();
        if let Some(&last)=points.last() {
            let mut previous=last;
            for &point in &points {
                if inside(previous)!=inside(point) {
                    let t=(edge-coordinate(previous))/(coordinate(point)-coordinate(previous));
                    next.push((previous.0+(point.0-previous.0)*t,previous.1+(point.1-previous.1)*t));
                }
                if inside(point) {next.push(point);}
                previous=point;
            }
        }
        points=next;
    }
    points
}

pub(super) fn layout(out:&mut NativeLayout,data:&[Value],x:f32,y:f32,w:f32,s:f32,ink:Ink,measure:&mut impl FnMut(&str,f32,bool)->f32)->f32 {
    let inset=(10.0*s).min(w*0.1);
    let left=x+inset;
    let top=y+4.0*s;
    let width=(w-2.0*inset).max(s*0.1);
    let height=(w*0.52).clamp(80.0*s,180.0*s);
    let bottom=top+height;
    let values:Vec<_>=data.iter().map(|p|p["value"].as_f64().unwrap()).collect();
    // Normalize before subtraction: finite endpoints may still span > f64::MAX.
    let magnitude=values.iter().fold(1.0_f64,|m,v|m.max(v.abs()));
    let normalized:Vec<_>=values.iter().map(|v|v/magnitude).collect();
    let min=normalized.iter().copied().fold(f64::INFINITY,f64::min);
    let max=normalized.iter().copied().fold(f64::NEG_INFINITY,f64::max);
    let points:Vec<_>=normalized.iter().enumerate().map(|(i,value)|{
        let fraction=if max>min {(value-min)/(max-min)} else {0.5};
        (left+width*i as f32/(data.len()-1) as f32,bottom-height*fraction as f32)
    }).collect();
    for row in 0..=4 {
        out.items.push(Item::Rect{rect:[left,top+height*row as f32/4.0,width,s],ink:Ink::Grid,radius:0.0});
    }

    // Monotone cubic Hermite interpolation: smooth, but never invents peaks
    // outside adjacent observations. Sampling is bounded independently of width.
    let slopes:Vec<_>=points.windows(2).map(|p|(p[1].1-p[0].1)/(p[1].0-p[0].0)).collect();
    let mut tangents=vec![slopes[0];points.len()];
    for i in 1..points.len()-1 {
        tangents[i]=if slopes[i-1]*slopes[i]<=0.0 {0.0} else {2.0/(1.0/slopes[i-1]+1.0/slopes[i])};
    }
    *tangents.last_mut().unwrap()=*slopes.last().unwrap();
    let samples=(128/(points.len()-1)).clamp(4,24);
    let mut curve=vec![points[0]];
    for i in 0..points.len()-1 {
        let (a,b)=(points[i],points[i+1]);
        let dx=b.0-a.0;
        for sample in 1..=samples {
            let t=sample as f32/samples as f32;
            let t2=t*t;let t3=t2*t;
            let value=(2.0*t3-3.0*t2+1.0)*a.1+(t3-2.0*t2+t)*dx*tangents[i]
                +(-2.0*t3+3.0*t2)*b.1+(t3-t2)*dx*tangents[i+1];
            curve.push((a.0+dx*t,value.clamp(a.1.min(b.1),a.1.max(b.1))));
        }
    }
    let mut area=curve.clone();area.extend([(left+width,bottom),(left,bottom)]);
    for band in 0..32 {
        // Nested low-opacity fills avoid the dark AA seams of adjacent bands.
        let clip=[left,top,width,height*(band as f32+1.0)/32.0];
        let points=clip_polygon(&area,clip);
        if points.len()>=3 {out.items.push(Item::Shape{points,ink,alpha:0.004});}
    }
    let mut outer=Vec::new();let mut inner=Vec::new();
    for (i,&point) in curve.iter().enumerate() {
        let (a,b)=(curve[i.saturating_sub(1)],curve[(i+1).min(curve.len()-1)]);
        let (dx,dy)=(b.0-a.0,b.1-a.1);
        let length=dx.hypot(dy).max(0.001);
        let (nx,ny)=(-dy/length*0.9*s,dx/length*0.9*s);
        outer.push((point.0+nx,point.1+ny));inner.push((point.0-nx,point.1-ny));
    }
    outer.extend(inner.into_iter().rev());
    out.items.push(Item::Shape{points:outer,ink,alpha:1.0});
    let diameter=(6.0*s).min(width*0.1);
    for &(px,py) in &points {
        out.items.push(Item::Rect{rect:[px-diameter*0.5,py-diameter*0.5,diameter,diameter],ink,radius:diameter*0.5});
    }
    let cell=(width/(data.len()-1) as f32).min(60.0*s);
    let stride=((32.0*s/(width/(data.len()-1) as f32)).ceil() as usize).max(1);
    let mut previous_right=f32::NEG_INFINITY;
    for (i,point) in data.iter().enumerate() {
        if i%stride!=0 && i!=data.len()-1 {continue;}
        let label=point["label"].as_str().unwrap();
        let label_w=measure(label,11.0,false).min(cell).min(w);
        let label_x=(points[i].0-label_w*0.5).clamp(x,x+w-label_w);
        if label_x<previous_right+4.0*s {continue;}
        out.items.push(Item::Text{x:label_x,y:bottom+18.0*s,width:label_w,text:label.to_owned(),size:11.0,bold:false,ink:Ink::Muted});
        previous_right=label_x+label_w;
    }
    height+40.0*s
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn chart_is_bounded_for_flat_negative_extreme_and_dense_data() {
        for values in [vec![0.0,0.0],vec![-8.0,-2.0,-6.0],vec![-f64::MAX,f64::MAX],(0..32).map(|i|i as f64).collect()] {
            let data:Vec<_>=values.iter().enumerate().map(|(i,v)|json!({"label":format!("Day {i}"),"value":v})).collect();
            for w in [8.0,100.0,400.0] {
                let mut out=NativeLayout{height:0.0,width:w,items:vec![],prepared:None};
                out.height=layout(&mut out,&data,0.0,0.0,w,1.0,Ink::Foreground,&mut |text,size,_|text.len() as f32*size*0.5);
                assert!(out.items.len()<2000);
                for item in &out.items {
                    match item {
                        Item::Diagram(_) => unreachable!(),
                        Item::Shape{points,..}=>assert!(points.iter().all(|(x,y)|x.is_finite()&&y.is_finite()&&*y>=0.0&&*y<=out.height)),
                        Item::Text{x,width,..}=>assert!(*x>=0.0&&x+width<=w+0.001),
                        Item::Rect{rect,..}=>assert!(rect.iter().all(|v|v.is_finite())),
                    }
                }
            }
        }
    }

    #[test]
    fn chart_shapes_clip_all_edges() {
        let points=clip_polygon(&[(-2.0,-2.0),(12.0,-2.0),(12.0,12.0),(-2.0,12.0)],[0.0,0.0,10.0,10.0]);
        assert_eq!(points.len(),4);
        assert!(points.iter().all(|(x,y)|(0.0..=10.0).contains(x)&&(0.0..=10.0).contains(y)));
        assert!(clip_polygon(&[(20.0,20.0),(22.0,20.0),(21.0,22.0)],[0.0,0.0,10.0,10.0]).is_empty());
    }
}