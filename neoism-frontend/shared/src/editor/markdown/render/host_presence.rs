//! Original presence flag exported to other native GPU hosts.
use crate::primitives::paint::Paint;
use sugarloaf::text::DrawOpts;
#[allow(clippy::too_many_arguments)]
pub fn render_presence_name_flag(painter:&mut(impl Paint+?Sized),clip:[f32;4],name:&str,client_os:Option<&str>,origin:[f32;2],font_size:f32,color:[f32;4],ink:[u8;4]){
    if name.is_empty(){return;}
    let opts=DrawOpts{font_size,color:ink,clip_rect:Some(clip),..DrawOpts::default()};
    let width=painter.text_mut().measure(name,&opts);
    let height=font_size+4.0;
    let [x,y]=origin;
    super::draw::draw_presence_avatar_clipped(painter,clip,name,client_os,x,y,height,crate::editor::crdt::presence_orb_now_seconds(),0.0,8);
    let flag_x=x+height+2.0;
    super::draw::draw_rounded_rect_clipped(painter,clip,flag_x,y,width+8.0,height,3.0,color,0.0,8);
    painter.text_mut().draw(flag_x+4.0,y+2.0,name,&opts);
}
