//! Original collaborator cursor geometry, flags and plasma avatars on the GPU host.
use neoism_protocol::crdt::CrdtPeerPresence;
use neoism_ui::{editor::{code::{CodePane,layout::{TAB_DISPLAY_WIDTH,display_col_for_byte}},markdown::render::render_presence_name_flag},primitives::{paint::Paint,paint_record::RecordingPainter,ide_theme::IdeTheme}};
use serde_json::{json,Value};

#[derive(Default)]
pub struct PeerPainter{painter:Option<RecordingPainter>,settings:Value}
pub fn byte_col(source:&str,column:usize)->usize{let mut units=0;for (byte,ch) in source.char_indices(){if units+ch.len_utf16()>column{return byte;}units+=ch.len_utf16();}source.len()}
impl PeerPainter {
 pub fn configure(&mut self,v:&Value){let settings=json!({"fontFamily":v["fontFamily"],"fontWeight":v["fontWeight"],"fontSize":v["fontSize"],"theme":v["theme"]});if self.settings!=settings{self.settings=settings;self.painter=None;}}
 pub fn frame(&mut self,pane:&CodePane,peers:&[CrdtPeerPresence])->Value {
  if peers.is_empty(){return json!([]);}
  let painter=self.painter.get_or_insert_with(||RecordingPainter::new(self.settings["fontFamily"].as_str().unwrap_or("Cascadia Code NF"),self.settings["fontWeight"].as_u64().unwrap_or(400) as u16,vec![]));painter.begin_frame();
  let font_size=self.settings["fontSize"].as_f64().unwrap_or(14.0) as f32;let theme=IdeTheme::by_name(self.settings["theme"].as_str().unwrap_or("pastel_dark"));
  let g=&pane.geometry;let [x,y,w,h]=g.rect;let clip=[g.text_x,y,(x+w-g.text_x).max(0.0),h];let cell=g.cell_w;let row=g.row_h;let caret_h=(font_size*1.2).min(row).round();let wrap=&g.wrap;
  for peer in peers {
   let rgb=if peer.rainbow{let c=neoism_ui::cursor_style::rainbow_color_f32(neoism_ui::cursor_style::rainbow_now_seconds());[c[0],c[1],c[2]]}else{[peer.color.r as f32/255.0,peer.color.g as f32/255.0,peer.color.b as f32/255.0]};
   if let Some(selection)=peer.selection {
    let a=(selection.anchor.line as usize,selection.anchor.column as usize);let b=(selection.head.line as usize,selection.head.column as usize);let (a,b)=if a<=b{(a,b)}else{(b,a)};
    let first=(g.scroll_y/row).floor().max(0.0) as usize;
    for visual in first..(first+(h/row).ceil() as usize+2).min(wrap.total_rows(pane.buffer.lines.len())){
     let Some(neoism_ui::editor::code::layout::VisualRow::Source{line,segment})=wrap.row_kind(visual)else{continue;};if line<a.0||line>b.0{continue;}let Some(seg)=wrap.segment(line,segment)else{continue;};let source=&pane.buffer.lines[line];let lo=if line==a.0{byte_col(source,a.1)}else{0}.max(seg.byte_start);let hi=if line==b.0{byte_col(source,b.1)}else{source.len()}.min(seg.byte_end);if lo>=hi{continue;}let col=seg.visual_indent+display_col_for_byte(source,lo,TAB_DISPLAY_WIDTH).saturating_sub(seg.source_col);let width=(display_col_for_byte(source,hi,TAB_DISPLAY_WIDTH)-display_col_for_byte(source,lo,TAB_DISPLAY_WIDTH)) as f32*cell;
     painter.rect(None,g.text_x+col as f32*cell-g.scroll_x,y+visual as f32*row-g.scroll_y,width,row,[rgb[0],rgb[1],rgb[2],0.2],0.0,7);
    }
   }
   let line=peer.cursor.line as usize;let Some(source)=pane.buffer.lines.get(line)else{continue;};let (segment,col)=wrap.visual_position(line,source,byte_col(source,peer.cursor.column as usize),TAB_DISPLAY_WIDTH);let visual=wrap.first_row_of_line(line)+segment;let ry=y+visual as f32*row-g.scroll_y;if ry+row<=y||ry>=y+h{continue;}let px=(g.text_x+col as f32*cell-g.scroll_x).clamp(g.text_x,(x+w-2.0).max(g.text_x));let cy=ry+((row-caret_h)*0.5).max(0.0).round();
   if neoism_ui::cursor_style::cursor_blink_visible(neoism_ui::editor::crdt::presence_orb_now_seconds()){painter.rect(None,px,cy,if peer.insert{2.0}else{cell},caret_h,[rgb[0],rgb[1],rgb[2],if peer.insert{0.95}else{0.38}],0.0,8);}
   if peer.display_name.is_empty(){continue;}
   let name_size=(font_size*0.58).max(8.0);let tag_y=(cy-name_size-4.0).max(y);
   render_presence_name_flag(painter,clip,&peer.display_name,peer.client_os.as_deref(),[px,tag_y],name_size,[rgb[0],rgb[1],rgb[2],0.9],theme.u8(theme.bg));
  }
  json!(&*painter.borrowed_commands())
 }
}
