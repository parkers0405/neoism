//! Bounded original chip typography for native hosts with independent controls.
use super::{status_chip_font_size,model_chip_label,capitalize_chip_label,model_logo_advance,MODEL_LOGO_SIDE};
use crate::{primitives::paint::Paint,panels::agent_pane::icon::ProviderLogo};
use serde::Serialize;
use sugarloaf::text::DrawOpts;
#[derive(Serialize)]
pub struct Chip {pub rect:[f32;4],pub label_rect:[f32;4],pub caret_rect:[f32;4],pub logo_rect:Option<[f32;4]>,pub label:String}
#[derive(Serialize)]
pub struct Layout {pub chips:[Chip;3],pub attach_rect:[f32;4],pub font_size:f32,pub narrow:bool}
pub fn layout(paint:&mut dyn Paint,width:f32,height:f32,scale:f32,agent:&str,model:&str,thinking:&str)->Layout {
 let width=width.max(0.0);let height=height.max(0.0);let s=scale.max(0.001);
 let narrow=super::super::layout::is_narrow_composer_width(width,s);
 let font=status_chip_font_size(width,s);let opts=DrawOpts{font_size:font,bold:true,..Default::default()};
 let caret_opts=DrawOpts{font_size:font*0.66,..Default::default()};let caret=paint.text_mut().measure("\u{f078}",&caret_opts);
 let labels=[capitalize_chip_label(agent),model_chip_label(model,narrow),capitalize_chip_label(thinking)];
 let natural=labels.each_ref().map(|label|paint.text_mut().measure(label,&opts));
 let logo=model_logo_advance(ProviderLogo::from_model(model),s);
 let attach_side=(22.0*s).min(width);let attach_gap=(8.0*s).min((width-attach_side).max(0.0));
 let budget=(width-attach_side-attach_gap).max(0.0);let gap=(4.0*s).min(budget/6.0);
 let usable=(budget-2.0*gap).max(0.0);
 let mut overhead=[caret+24.0*s,caret+24.0*s+logo,caret+24.0*s];
 if overhead.iter().sum::<f32>()>usable{overhead=[caret+6.0*s,caret+6.0*s+logo,caret+6.0*s];}
 let mut widths=[natural[0]+overhead[0],natural[1]+overhead[1],natural[2]+overhead[2]];
 // Keep mode and reasoning readable; the original model label is the flexible
 // part. At very small widths all three menus retain bounded hit targets.
 widths[1]=(usable-widths[0]-widths[2]).max(overhead[1]);
 if widths.iter().sum::<f32>()>usable {
  let minimum=overhead.iter().sum::<f32>();
  if minimum>=usable {widths=overhead.map(|value|if minimum>0.0{value*usable/minimum}else{0.0});}
  else {
   widths=overhead;let mut remaining=usable-minimum;
   for index in [0,2,1]{let add=natural[index].min(remaining);widths[index]+=add;remaining-=add;}
  }
 }else{widths[1]=widths[1].min(natural[1]+overhead[1]);}
 let mut x=0.0;
 let chips=std::array::from_fn(|index|{
  let w=widths[index].max(0.0).min((budget-x).max(0.0));let own_caret=caret.min(w);
  let pad=(6.0*s).min((w-own_caret).max(0.0));
  let own_logo=if index==1&&w>=logo+own_caret+pad+font{logo}else{0.0};
  let label_width=(w-own_logo-own_caret-pad- (overhead[index]-caret-if index==1{logo}else{0.0}-6.0*s).max(0.0)).max(0.0);
  // Subtracting fractional-scale widths can lose a few f32 ulps. Keep a
  // label that fits its measured reservation instead of spuriously ellipsizing.
  let tolerance=0.0001*s;
  let mut label=if natural[index]<=label_width+tolerance{labels[index].clone()}else{crate::primitives::text::truncate_to_fit(&labels[index],label_width,paint,&opts)};
  if paint.text_mut().measure(&label,&opts)>label_width+tolerance{label.clear();}
  let text_y=(height-font*1.3)*0.5;
  let result=Chip{rect:[x,0.0,w,height],label_rect:[own_logo,text_y,label_width,font*1.3],caret_rect:[(w-own_caret-(3.0*s).min(pad)).max(0.0),(height-font*0.66)*0.5,own_caret,font*0.9],logo_rect:if own_logo>0.0{Some([0.0,(height-MODEL_LOGO_SIDE*s)*0.5,MODEL_LOGO_SIDE*s,MODEL_LOGO_SIDE*s])}else{None},label};
  x+=w+gap;result
 });
 Layout{chips,attach_rect:[width-attach_side,(height-attach_side)*0.5,attach_side,attach_side],font_size:font,narrow}
}
