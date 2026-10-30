//! Native hosts persist the same virtual viewport anchor used for live edits.
use super::MarkdownPane;
use neoism_paint::{NodeId,VirtualScrollAnchor};
impl MarkdownPane {
 pub fn capture_host_scroll_anchor(&mut self)->Option<VirtualScrollAnchor>{
  self.virtual_render.surface.capture_scroll_anchor((self.scroll_viewport_height*0.35).max(0.0))
 }
 pub fn restore_host_scroll_anchor(&mut self,mut anchor:VirtualScrollAnchor)->bool{
  // A cold adapter allocates fresh node identities. Canonical source/block
  // order is retained by document recovery, so use its original index fallback.
  anchor.node=NodeId(u64::MAX);
  if self.virtual_render.surface.restore_scroll_anchor(anchor).is_err(){return false;}
  let scroll=self.virtual_render.surface.scroll().scroll_y;
  self.restore_scroll_position(scroll);
  self.virtual_render.pending_measure_anchor=Some(anchor);
  true
 }
}
