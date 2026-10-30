//! Presence is logical document state, independent of scroll and paint clocks.
use serde_json::{json,Value};
type Point=(usize,usize);
type Key=(u64,Point,Option<(Point,Point)>,bool);
#[derive(Default)]
pub struct PresenceCache { sample:Option<(Key,Value)> }
impl PresenceCache {
 pub fn clear(&mut self){self.sample=None;}
 pub fn frame(&mut self,revision:u64,lines:&[String],cursor:Point,selection:Option<(Point,Point)>,insert:bool)->Value{
  let key=(revision,cursor,selection,insert);
  if let Some((previous,value))=&self.sample {if *previous==key{return value.clone();}}
  let point=|(line,col):Point|{
   let line=line.min(lines.len().saturating_sub(1));let source=lines.get(line).map(String::as_str).unwrap_or("");
   let mut col=col.min(source.len());while !source.is_char_boundary(col){col-=1;}
   let column=source[..col].encode_utf16().count();
   let offset=lines[..line].iter().map(|s|s.encode_utf16().count()+1).sum::<usize>()+column;
   json!({"line":line,"column":column,"offset":offset})
  };
  let value=json!({"cursor":point(cursor),"selection":selection.map(|(a,b)|json!({"anchor":point(a),"head":point(b)})),"insert":insert});
  self.sample=Some((key,value.clone()));value
 }
}
