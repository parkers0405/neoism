//! Bounded preparation caches for the inside of a streaming block. Whole-message
//! caching cannot help while a fence/table is still growing.
use super::*;

struct Fence {
    source: String,
    lang: String,
    generation: u64,
    scale: u32,
    lines: Rc<Vec<String>>,
    width: f32,
    complete_bytes: usize,
    complete_lines: usize,
    measured: usize,
    widths: Vec<f32>,
}
thread_local! {
    static FENCES: RefCell<VecDeque<Fence>> = const { RefCell::new(VecDeque::new()) };
    static BUDGET: RefCell<Option<(web_time::Instant,bool,usize)>> = const {RefCell::new(None)};
    static TABLES: RefCell<VecDeque<Table>> = const {RefCell::new(VecDeque::new())};
    static CELLS: RefCell<HashMap<CellKey, Vec<String>>> = RefCell::new(HashMap::new());
}
#[derive(Hash,PartialEq,Eq)]
struct CellKey { source: Vec<String>, widths: Vec<u32>, scale: u32, generation: u64 }

fn stable_fence_prefix(entry:&Fence)->usize{
    if entry.source.ends_with('\n'){entry.source[..entry.source.len()-1].rfind('\n').map_or(0,|i|i+1)}else{entry.complete_bytes}
}

pub(super) fn code_lines(paint:&mut dyn crate::primitives::paint::Paint,lang:&str,lines:&[String],s:f32)->Option<AssistantMarkdownBlock>{
    if lines.iter().map(String::len).sum::<usize>()<2048{return None;}
    // A delimiter longer than any source run cannot close inside the code.
    let fence_len=lines.iter().map(|line|line.bytes().filter(|b|*b==b'`').count()).max().unwrap_or(0).max(2)+1;
    let opening="`".repeat(fence_len);
    fence(paint,&format!("{opening}{lang}\n{}\n",lines.join("\n")),s)
}

pub(super) fn fence(paint:&mut dyn crate::primitives::paint::Paint,text:&str,s:f32)->Option<AssistantMarkdownBlock>{
    let (opening,body)=text.split_once('\n')?;
    let (delimiter,lang)=md::fence_open(opening)?;
    let trimmed=body.trim_end_matches(['\n','\r']);
    let body=if let Some((prefix,last))=trimmed.rsplit_once('\n'){if delimiter.closes(last){&body[..prefix.len()+1]}else{body}}else{body};
    // These fences have completion-dependent semantics beyond code layout.
    if matches!(lang.trim().to_ascii_lowercase().as_str(),"mermaid"|"stock"|"neoism-html"){return None;}
    if text.len()<2048{return None;}
    let generation=paint.text_mut().font_cache_generation();
    FENCES.with(|cache|{
        let mut cache=cache.borrow_mut();
        let hit=cache.iter().position(|entry|entry.generation==generation&&entry.scale==s.to_bits()&&entry.lang==lang&&body.starts_with(&entry.source[..stable_fence_prefix(entry)]));
        let mut entry=hit.and_then(|i|cache.remove(i)).unwrap_or_else(||Fence{source:String::new(),lang:lang.to_owned(),generation,scale:s.to_bits(),lines:Rc::new(vec![]),width:0.0,complete_bytes:0,complete_lines:0,measured:0,widths:vec![]});
        if entry.source!=body {
            if !body.starts_with(&entry.source[..entry.complete_bytes]){
                entry.complete_bytes=stable_fence_prefix(&entry);entry.complete_lines=entry.source[..entry.complete_bytes].lines().count();
            }
            let tail=&body[entry.complete_bytes..];
            if tail.lines().any(|line|delimiter.closes(line)){return None;}
            let lines=Rc::make_mut(&mut entry.lines);lines.truncate(entry.complete_lines);
            entry.widths.truncate(entry.complete_lines);entry.measured=entry.measured.min(entry.complete_lines);
            entry.width=entry.widths.last().copied().unwrap_or(0.0);
            for raw in tail.split_inclusive('\n'){
                let line=if let Some(line)=raw.strip_suffix('\n'){line.strip_suffix('\r').unwrap_or(line)}else{raw};
                lines.push(line.to_owned());
                if raw.ends_with('\n'){entry.complete_bytes+=raw.len();entry.complete_lines=lines.len();}
            }
            entry.source=body.to_owned();
        }
        let opts=DrawOpts{font_size:12.5*s,bold:true,..DrawOpts::default()};
        while entry.measured<entry.lines.len(){
            if exhausted(){break;}
            entry.width=entry.width.max(measure_text_cached(paint,&entry.lines[entry.measured],&opts));
            let current=paint.text_mut().font_cache_generation();
            if current!=entry.generation{entry.generation=current;entry.width=0.0;entry.widths.clear();entry.measured=0;continue;}
            entry.widths.push(entry.width);entry.measured+=1;
        }
        let block=AssistantMarkdownBlock::Code{lang:entry.lang.clone(),lines:entry.lines.clone(),copy_target:copy_ref_target_for_lines(&entry.lines),content_width:entry.width};
        // Keep only a handful of active tails, with a byte bound as well.
        cache.push_back(entry);
        while cache.len()>1&&(cache.len()>8||cache.iter().map(|e|e.source.len()*2).sum::<usize>()>8*1024*1024){cache.pop_front();}
        Some(block)
    })
}

pub(super) fn table_row(paint:&mut dyn crate::primitives::paint::Paint,row:Vec<String>,widths:&[f32],s:f32,opts:&DrawOpts)->Vec<String>{
    let key=CellKey{source:row.clone(),widths:widths.iter().map(|w|w.to_bits()).collect(),scale:s.to_bits(),generation:paint.text_mut().font_cache_generation()};
    if let Some(hit)=CELLS.with(|cache|cache.borrow().get(&key).cloned()){return hit;}
    let wrapped=wrap_table_cells(paint,row,widths,s,opts);
    // Very large cells are handled directly instead of pinning arbitrary input.
    if key.source.iter().map(String::len).sum::<usize>()<=4096{
        CELLS.with(|cache|{let mut cache=cache.borrow_mut();if cache.len()>=8192{cache.clear();}cache.insert(key,wrapped.clone());});
    }
    wrapped
}

/// Only retained hosts opt into resumable intrinsic-width preparation. Other
/// renderers keep synchronous behavior. Code row heights never depend on width.
pub fn begin_preparation(){BUDGET.with(|b|*b.borrow_mut()=Some((web_time::Instant::now(),false,0)));}
pub fn preparation_pending()->bool{BUDGET.with(|b|b.borrow().as_ref().is_some_and(|(_,pending,_)|*pending))}
pub fn end_preparation()->bool{BUDGET.with(|b|b.borrow_mut().take().is_some_and(|(_,pending,_)|pending))}
fn exhausted()->bool{BUDGET.with(|b|{let mut b=b.borrow_mut();if let Some((start,pending,units))=b.as_mut(){if *units==0{*start=web_time::Instant::now();}*units+=1;if *units>1&&start.elapsed().as_micros()>=1500{*pending=true;return true;}}false})}

struct Table {
    source: Vec<Vec<String>>, width:u32, scale:u32, generation:u64,
    measured:usize, intrinsic:Vec<f32>, row_widths:Vec<Vec<f32>>, widths:Option<Vec<f32>>, wrapped:Vec<Vec<String>>, previous:Option<AssistantMarkdownBlock>,
}
pub(super) fn table(paint:&mut dyn crate::primitives::paint::Paint,rows:Vec<Vec<String>>,width:f32,s:f32,opts:&DrawOpts)->AssistantMarkdownBlock{
    if rows.len()<64 {let widths=table_column_widths(paint,&rows,width,s,opts);let rows=rows.into_iter().map(|row|table_row(paint,row,&widths,s,opts)).collect();return laid_out_table_block(rows,widths);}
    let generation=paint.text_mut().font_cache_generation();
    TABLES.with(|cache|{
        let mut cache=cache.borrow_mut();
        let hit=cache.iter().position(|t|t.width==width.to_bits()&&t.scale==s.to_bits()&&t.generation==generation&& (t.source==rows || t.source.len()<=rows.len()&&t.source[..t.source.len().saturating_sub(1)]==rows[..t.source.len().saturating_sub(1)]));
        let mut job=hit.and_then(|i|cache.remove(i)).unwrap_or_else(||Table{intrinsic:vec![0.0;table_column_count(&rows)],source:rows.clone(),width:width.to_bits(),scale:s.to_bits(),generation,measured:0,row_widths:vec![],widths:None,wrapped:vec![],previous:None});
        if job.source!=rows {
            if job.wrapped.len()==job.source.len(){job.previous=Some(laid_out_table_block(job.wrapped.clone(),job.widths.clone().unwrap()));}
            let common=job.source.iter().zip(&rows).take_while(|(a,b)|a==b).count();
            job.source=rows;job.measured=job.measured.min(common);job.row_widths.truncate(job.measured);job.wrapped.truncate(common);
            job.intrinsic=vec![0.0;table_column_count(&job.source)];
            for row in &job.row_widths{for(col,w)in row.iter().enumerate(){job.intrinsic[col]=job.intrinsic[col].max(*w);}}
        }
        while job.measured<job.source.len(){
            if exhausted(){break;}
            let row=&job.source[job.measured];
            let measured:Vec<f32>=row.iter().map(|cell|cell.lines().map(|line|measure_markdown_inline_width(paint,line.trim(),opts)).fold(0.0_f32,f32::max)).collect();
            for (col,w) in measured.iter().enumerate(){job.intrinsic[col]=job.intrinsic[col].max(*w);}job.row_widths.push(measured);
            let current=paint.text_mut().font_cache_generation();
            if current!=job.generation{job.generation=current;job.measured=0;job.row_widths.clear();job.intrinsic.fill(0.0);job.widths=None;job.wrapped.clear();continue;}
            job.measured+=1;
        }
        if job.measured==job.source.len(){
            let widths=resolve_table_column_widths(job.intrinsic.iter().map(|w|w+28.0*s).collect(),width,s);
            if job.widths.as_ref()!=Some(&widths){job.wrapped.clear();job.widths=Some(widths);}
            let widths=job.widths.as_ref().unwrap();
            while job.wrapped.len()<job.source.len(){if exhausted(){break;}job.wrapped.push(table_row(paint,job.source[job.wrapped.len()].clone(),widths,s,opts));}
        }
        let complete=job.wrapped.len()==job.source.len();
        let block=if complete{laid_out_table_block(job.wrapped.clone(),job.widths.clone().unwrap())}else{
            // Keep the previous coherent table while preparing an appended row.
            // A cold table uses its source rows until exact wrapping is ready.
            let previous=cache.iter().rev().find(|old|old.width==job.width&&old.scale==job.scale&&old.generation==job.generation&&old.wrapped.len()==old.source.len()&&job.source.starts_with(&old.source));
            if let Some(old)=job.previous.as_ref(){old.clone()}else if let Some(old)=previous{laid_out_table_block(old.wrapped.clone(),old.widths.clone().unwrap())}
            else{laid_out_table_block(job.source.clone(),resolve_table_column_widths(vec![TABLE_MAX_COLUMN_W*s;job.intrinsic.len()],width,s))}
        };
        let bytes=|t:&Table|t.source.iter().flatten().chain(t.wrapped.iter().flatten()).map(String::len).sum::<usize>();
        // A currently visible large table may exceed the cache target; retain
        // that one job so incremental progress cannot restart every frame.
        cache.push_back(job);while cache.len()>1&&(cache.len()>8||cache.iter().map(bytes).sum::<usize>()>8*1024*1024){cache.pop_front();}
        block
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::paint_record::RecordingPainter;
    #[test]
    fn fence_appends_preserve_old_blocks_and_remeasure_partial_unicode_lines(){
        let mut painter=RecordingPainter::new("monospace",400,vec![]);
        let mut text=format!("```rust\n{}", "let old = 1;\n".repeat(200));
        let first=fence(&mut painter,&text,1.0).unwrap();
        for suffix in ["let emoji = \"", "抹茶🦀", "\";\r", "\n", "// done\n", "```\n"] {
            text.push_str(suffix);
            let actual=fence(&mut painter,&text,1.0).unwrap();
            let AssistantMarkdownBlock::Code{lines,content_width,..}=actual else{panic!()};
            let body=text.split_once('\n').unwrap().1;
            let expected:Vec<String>=body.lines().take_while(|l|*l!="```").map(str::to_owned).collect();
            assert_eq!(lines.as_ref(),&expected);
            let expected_width=expected.iter().map(|line|measure_text_cached(&mut painter,line,&DrawOpts{font_size:12.5,bold:true,..DrawOpts::default()})).fold(0.0_f32,f32::max);
            assert_eq!(content_width,expected_width,"suffix {suffix:?}");
        }
        let AssistantMarkdownBlock::Code{lines,..}=first else{panic!()};assert_eq!(lines.len(),200);
    }
    #[test]
    fn embedded_fence_preserves_blank_lines_and_partial_line_edits(){
        let mut painter=RecordingPainter::new("monospace",400,vec![]);
        let mut lines=vec!["const example = 1;".to_owned();200];lines.push(String::new());
        for suffix in ["", "// partial", "// partial extended 🦀"]{
            *lines.last_mut().unwrap()=suffix.into();
            let AssistantMarkdownBlock::Code{lines:actual,..}=code_lines(&mut painter,"javascript",&lines,1.0).unwrap() else{panic!()};
            assert_eq!(actual.as_ref(),&lines);
        }
    }
    #[test]
    fn interrupted_table_preparation_converges_and_appends_use_exact_wrapping(){
        let mut painter=RecordingPainter::new("monospace",400,vec![]);
        let opts=DrawOpts{font_size:13.5,..DrawOpts::default()};
        let mut rows:Vec<Vec<String>>=(0..180).map(|i|vec![format!("row {i}"),"a long **styled** cell with Unicode 抹茶 ".repeat(10)]).collect();
        for append in [false,true]{
            if append{rows.push(vec!["new".into(),"wider content ".repeat(80)]);}
            let mut finished=None;
            for _ in 0..1000{
                begin_preparation();let block=table(&mut painter,rows.clone(),700.0,1.0,&opts);let pending=end_preparation();
                if !pending{finished=Some(block);break;}
            }
            let AssistantMarkdownBlock::Table{rows:actual,column_widths,row_offsets}=finished.expect("preparation must make progress") else{panic!()};
            let expected_widths=table_column_widths(&mut painter,&rows,700.0,1.0,&opts);assert_eq!(column_widths,expected_widths);
            let expected:Vec<_>=rows.iter().cloned().map(|row|wrap_table_cells(&mut painter,row,&expected_widths,1.0,&opts)).collect();assert_eq!(actual,expected);assert_eq!(row_offsets.len(),rows.len()+1);
        }
    }
}
