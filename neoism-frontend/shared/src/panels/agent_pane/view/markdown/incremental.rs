//! Bounded preparation caches for the inside of a streaming block. Whole-message
//! caching cannot help while a fence/table is still growing.
use super::*;

struct Fence {
    visit: Option<Visit>,
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
    static BUDGET: RefCell<Option<Budget>> = const {RefCell::new(None)};
    static SCOPES: RefCell<VecDeque<(u64,u64)>> = const {RefCell::new(VecDeque::new())};
    #[cfg(test)]
    static UNIT_LIMIT: std::cell::Cell<Option<usize>> = const {std::cell::Cell::new(None)};
    static TABLES: RefCell<VecDeque<Table>> = const {RefCell::new(VecDeque::new())};
    static CELLS: RefCell<HashMap<CellKey, Vec<String>>> = RefCell::new(HashMap::new());
}
#[derive(Hash,PartialEq,Eq)]
struct CellKey { source: Vec<String>, widths: Vec<u32>, scale: u32, generation: u64 }

#[derive(Clone,Copy)]
struct Visit { scope:u64, pass:u64 }
struct Budget { start:web_time::Instant, pending:bool, units:usize, visit:Visit }
fn current_visit()->Option<Visit>{BUDGET.with(|b|b.borrow().as_ref().map(|b|b.visit))}
fn same_scope(a:Option<Visit>,b:Option<Visit>)->bool{a.map(|v|v.scope)==b.map(|v|v.scope)}

// Limits are soft for the current/previous working set: evicting an unfinished
// job (or an already finished prefix of the pass) can prevent convergence forever.
// Only this scope's stale jobs may be evicted. Other panes have their own passes.
fn trim_jobs<T>(cache:&mut VecDeque<T>,visit:Option<Visit>,stamp:impl Fn(&T)->Option<Visit>,bytes:impl Fn(&T)->usize){
    let owned=|job:&T|same_scope(stamp(job),visit);
    let mut count=cache.iter().filter(|job|owned(job)).count();
    let mut size=cache.iter().filter(|job|owned(job)).map(&bytes).sum::<usize>();
    while count>8||size>8*1024*1024{
        let stale=cache.iter().position(|job|match (stamp(job),visit){
            (Some(old),Some(now))=>old.scope==now.scope&&old.pass<now.pass.saturating_sub(1),
            (None,None)=>true,
            _=>false,
        });
        let Some(i)=stale else{break;};
        // Synchronous hosts still retain one oversized job, as before.
        if visit.is_none()&&count==1{break;}
        size-=bytes(&cache.remove(i).unwrap());
        count-=1;
    }
}

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
    // The full-message layout probes this fast path before parsing its blocks.
    // Reject multiple fences BEFORE taking a matching job out of the cache:
    // the first block's cached source is also a prefix of that failed probe.
    // Discarding it here restarts that block every frame and starves the rest.
    if body.lines().any(|line|delimiter.closes(line)){return None;}
    let generation=paint.text_mut().font_cache_generation();
    let visit=current_visit();
    FENCES.with(|cache|{
        let mut cache=cache.borrow_mut();
        let hit=cache.iter().position(|entry|same_scope(entry.visit,visit)&&entry.generation==generation&&entry.scale==s.to_bits()&&entry.lang==lang&&body.starts_with(&entry.source[..stable_fence_prefix(entry)]));
        let mut entry=hit.and_then(|i|cache.remove(i)).unwrap_or_else(||Fence{visit,source:String::new(),lang:lang.to_owned(),generation,scale:s.to_bits(),lines:Rc::new(vec![]),width:0.0,complete_bytes:0,complete_lines:0,measured:0,widths:vec![]});
        entry.visit=visit;
        if entry.source!=body {
            if !body.starts_with(&entry.source[..entry.complete_bytes]){
                entry.complete_bytes=stable_fence_prefix(&entry);entry.complete_lines=entry.source[..entry.complete_bytes].lines().count();
            }
            let tail=&body[entry.complete_bytes..];
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
        cache.push_back(entry);
        trim_jobs(&mut cache,visit,|e|e.visit,|e|e.source.len()*2);
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
pub fn begin_preparation(){begin_preparation_scoped(0);}
/// Use a stable pane/view ID to keep interleaved preparation passes independent.
/// Eight recently used scopes are retained; switching through more panes releases
/// the least recently used scope, rather than accumulating pinned jobs forever.
pub fn begin_preparation_scoped(scope:u64){
    let visit=SCOPES.with(|scopes|{
        let mut scopes=scopes.borrow_mut();
        let pass=scopes.iter().position(|(id,_)|*id==scope).and_then(|i|scopes.remove(i)).map_or(1,|(_,pass)|pass+1);
        scopes.push_back((scope,pass));
        if scopes.len()>8{
            let (old,_)=scopes.pop_front().unwrap();
            FENCES.with(|cache|cache.borrow_mut().retain(|e|e.visit.is_none_or(|v|v.scope!=old)));
            TABLES.with(|cache|cache.borrow_mut().retain(|e|e.visit.is_none_or(|v|v.scope!=old)));
        }
        Visit{scope,pass}
    });
    BUDGET.with(|b|*b.borrow_mut()=Some(Budget{start:web_time::Instant::now(),pending:false,units:0,visit}));
}
/// Release a disposed host's jobs without disturbing other panes.
pub fn release_preparation_scope(scope:u64){
    SCOPES.with(|scopes|scopes.borrow_mut().retain(|(id,_)|*id!=scope));
    FENCES.with(|cache|cache.borrow_mut().retain(|e|e.visit.is_none_or(|v|v.scope!=scope)));
    TABLES.with(|cache|cache.borrow_mut().retain(|e|e.visit.is_none_or(|v|v.scope!=scope)));
    BUDGET.with(|b|{let mut b=b.borrow_mut();if b.as_ref().is_some_and(|b|b.visit.scope==scope){*b=None;}});
}
pub fn preparation_pending()->bool{BUDGET.with(|b|b.borrow().as_ref().is_some_and(|b|b.pending))}
pub fn end_preparation()->bool{BUDGET.with(|b|b.borrow_mut().take().is_some_and(|b|b.pending))}
fn exhausted()->bool{BUDGET.with(|b|{let mut b=b.borrow_mut();if let Some(b)=b.as_mut(){
    if b.units==0{b.start=web_time::Instant::now();}b.units+=1;
    #[cfg(test)]
    let over=UNIT_LIMIT.with(|limit|limit.get().map(|limit|b.units>limit));
    #[cfg(not(test))]
    let over:Option<bool>=None;
    if over.unwrap_or_else(||b.units>1&&b.start.elapsed().as_micros()>=1500){b.pending=true;return true;}
}false})}

struct Table {
    visit:Option<Visit>,
    source: Vec<Vec<String>>, width:u32, scale:u32, generation:u64,
    measured:usize, intrinsic:Vec<f32>, row_widths:Vec<Vec<f32>>, widths:Option<Vec<f32>>, wrapped:Vec<Vec<String>>, previous:Option<AssistantMarkdownBlock>,
}
pub(super) fn table(paint:&mut dyn crate::primitives::paint::Paint,rows:Vec<Vec<String>>,width:f32,s:f32,opts:&DrawOpts)->AssistantMarkdownBlock{
    if rows.len()<64 {let widths=table_column_widths(paint,&rows,width,s,opts);let rows=rows.into_iter().map(|row|table_row(paint,row,&widths,s,opts)).collect();return laid_out_table_block(rows,widths);}
    let generation=paint.text_mut().font_cache_generation();
    let visit=current_visit();
    TABLES.with(|cache|{
        let mut cache=cache.borrow_mut();
        let hit=cache.iter().position(|t|same_scope(t.visit,visit)&&t.width==width.to_bits()&&t.scale==s.to_bits()&&t.generation==generation&& (t.source==rows || t.source.len()<=rows.len()&&t.source[..t.source.len().saturating_sub(1)]==rows[..t.source.len().saturating_sub(1)]));
        let mut job=hit.and_then(|i|cache.remove(i)).unwrap_or_else(||Table{visit,intrinsic:vec![0.0;table_column_count(&rows)],source:rows.clone(),width:width.to_bits(),scale:s.to_bits(),generation,measured:0,row_widths:vec![],widths:None,wrapped:vec![],previous:None});
        job.visit=visit;
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
            let previous=cache.iter().rev().find(|old|same_scope(old.visit,visit)&&old.width==job.width&&old.scale==job.scale&&old.generation==job.generation&&old.wrapped.len()==old.source.len()&&job.source.starts_with(&old.source));
            if let Some(old)=job.previous.as_ref(){old.clone()}else if let Some(old)=previous{laid_out_table_block(old.wrapped.clone(),old.widths.clone().unwrap())}
            else{laid_out_table_block(job.source.clone(),resolve_table_column_widths(vec![TABLE_MAX_COLUMN_W*s;job.intrinsic.len()],width,s))}
        };
        let bytes=|t:&Table|t.source.iter().flatten().chain(t.wrapped.iter().flatten()).map(String::len).sum::<usize>();
        cache.push_back(job);trim_jobs(&mut cache,visit,|t|t.visit,bytes);
        block
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::paint_record::RecordingPainter;
    struct ForcedBudget;
    impl ForcedBudget {
        fn new(units:usize)->Self{
            FENCES.with(|c|c.borrow_mut().clear());TABLES.with(|c|c.borrow_mut().clear());
            SCOPES.with(|c|c.borrow_mut().clear());
            UNIT_LIMIT.with(|limit|limit.set(Some(units)));Self
        }
    }
    impl Drop for ForcedBudget {
        fn drop(&mut self){end_preparation();UNIT_LIMIT.with(|limit|limit.set(None));}
    }
    #[test]
    fn cached_full_message_layout_with_twelve_thousand_code_lines_converges(){
        let mut painter=RecordingPainter::new("monospace",403,vec![]);
        let pane=crate::panels::agent_pane::state::NeoismAgentPane::new();
        let theme=IdeTheme::by_name("dark");
        let text=(0..12).map(|block|format!("```rust\n{}\n```\n\n",(0..1000).map(|line|format!("let value_{block}_{line} = {line}; // block {block}, line {line}")).collect::<Vec<_>>().join("\n"))).collect::<String>();
        // Resolve fonts without populating this pane's whole-message cache.
        layout_assistant_markdown(&mut painter,&text,1000.0,&theme,1.0);
        let _budget=ForcedBudget::new(97);
        let mut completed=None;
        let mut previous=0;
        for frame in 0..130{
            begin_preparation_scoped(123);
            let blocks=layout_assistant_markdown_cached(&mut painter,&pane,&text,1000.0,&theme,1.0);
            let pending=end_preparation();
            let progress=FENCES.with(|cache|cache.borrow().iter().map(|job|job.measured).sum::<usize>());
            assert!(progress>previous,"full-message frame {frame} lost or starved preparation: {progress} <= {previous}");
            previous=progress;
            if !pending{completed=Some(blocks);break;}
        }
        let blocks=completed.expect("full cached layout must converge within its deterministic work bound");
        let code:Vec<_>=blocks.iter().filter_map(|block|if let AssistantMarkdownBlock::Code{lines,..}=block{Some(lines)}else{None}).collect();
        assert_eq!(code.len(),12);
        assert!(code.iter().all(|lines|lines.len()==1000));
        assert_eq!(previous,12000);
        // Completion must publish a coherent whole-message cache, not merely
        // stop requesting frames while an intrinsic-width job is unfinished.
        begin_preparation_scoped(123);
        let cached=layout_assistant_markdown_cached(&mut painter,&pane,&text,1000.0,&theme,1.0);
        assert!(!end_preparation());
        assert!(Rc::ptr_eq(&blocks,&cached));
    }
    #[test]
    fn rejected_multi_fence_probe_preserves_the_first_blocks_job(){
        let mut painter=RecordingPainter::new("monospace",404,vec![]);
        let first=format!("```rust\n{}\n```\n",(0..1000).map(|line|format!("let value_{line} = {line};")).collect::<Vec<_>>().join("\n"));
        fence(&mut painter,&first,1.0).unwrap();
        let _budget=ForcedBudget::new(3);
        begin_preparation();fence(&mut painter,&first,1.0).unwrap();assert!(end_preparation());
        let multiple=format!("{first}\n{first}");
        begin_preparation();
        assert!(fence(&mut painter,&multiple,1.0).is_none());
        FENCES.with(|cache|{
            let cache=cache.borrow();assert_eq!(cache.len(),1);
            assert_eq!(cache[0].measured,3,"a speculative rejection must not destroy prior work");
        });
        fence(&mut painter,&first,1.0).unwrap();assert!(end_preparation());
        FENCES.with(|cache|assert_eq!(cache.borrow()[0].measured,6));
    }
    #[test]
    fn over_capacity_fences_and_tables_converge_with_a_fixed_work_budget(){
        // Isolate the font library from concurrently running tests, and resolve
        // fallback/style faces before counting work (font changes restart jobs).
        let mut painter=RecordingPainter::new("monospace",401,vec![]);
        let opts=DrawOpts{font_size:13.5,..DrawOpts::default()};
        let fences:Vec<_>=(0..12).map(|job|format!("```rust\n{}",(0..64).map(|line|format!("job_{job}_line_{line}: unique code with Unicode 抹茶\n")).collect::<String>())).collect();
        let tables:Vec<Vec<Vec<String>>>=(0..12).map(|job|(0..64).map(|row|vec![format!("job {job} row {row}"),format!("**unique** cell {job}/{row}")]).collect()).collect();
        for text in &fences{fence(&mut painter,text,1.0).unwrap();}
        for rows in &tables{table(&mut painter,rows.clone(),700.0,1.0,&opts);}
        let _budget=ForcedBudget::new(37);
        let mut last_progress=0;
        let mut finished=false;
        // 12*64 fence measurements + 12*64 table measurements + 12*64
        // wraps = 2304 units. With 37 units per pass this must finish in 63.
        // The old FIFO never finishes, regardless of the wall clock speed.
        for pass in 0..64{
            begin_preparation();
            for text in &fences{assert!(fence(&mut painter,text,1.0).is_some());}
            let blocks:Vec<_>=tables.iter().map(|rows|table(&mut painter,rows.clone(),700.0,1.0,&opts)).collect();
            let pending=end_preparation();
            FENCES.with(|cache|assert_eq!(cache.borrow().len(),12));
            TABLES.with(|cache|assert_eq!(cache.borrow().len(),12));
            let progress=FENCES.with(|c|c.borrow().iter().map(|e|e.measured).sum::<usize>())+TABLES.with(|c|c.borrow().iter().map(|e|e.measured+e.wrapped.len()).sum::<usize>());
            assert!(progress>last_progress,"pass {pass} must preserve and advance work");last_progress=progress;
            if !pending{
                for (rows,block) in tables.iter().zip(blocks){
                    let AssistantMarkdownBlock::Table{rows:actual,column_widths,..}=block else{panic!()};
                    let widths=table_column_widths(&mut painter,rows,700.0,1.0,&opts);
                    assert_eq!(column_widths,widths);
                    let expected:Vec<_>=rows.iter().cloned().map(|row|wrap_table_cells(&mut painter,row,&widths,1.0,&opts)).collect();
                    assert_eq!(actual,expected);
                }
                FENCES.with(|cache|for entry in cache.borrow().iter(){
                    let expected=entry.lines.iter().map(|line|measure_text_cached(&mut painter,line,&DrawOpts{font_size:12.5,bold:true,..DrawOpts::default()})).fold(0.0_f32,f32::max);
                    assert_eq!(entry.width,expected);
                });
                finished=true;break;
            }
        }
        assert!(finished,"all over-capacity jobs must converge within the work bound");
    }
    #[test]
    fn byte_limit_protects_two_passes_but_reclaims_stale_work(){
        // Model oversized jobs without involving font measurement of huge lines.
        let stamp=|scope,pass|Some(Visit{scope,pass});
        let mut jobs=VecDeque::from([stamp(7,1),stamp(7,1),stamp(8,1)]);
        trim_jobs(&mut jobs,stamp(7,2),|v|*v,|_|9*1024*1024);
        assert_eq!(jobs.len(),3,"previous working set and other panes are protected");
        jobs.push_back(stamp(7,3));
        trim_jobs(&mut jobs,stamp(7,3),|v|*v,|_|9*1024*1024);
        assert_eq!(jobs.len(),2,"stale jobs must be evicted despite oversized active jobs");
        assert_eq!(jobs.front().unwrap().unwrap().scope,8);
    }
    #[test]
    fn scoped_passes_preserve_other_panes_and_bound_pane_switching(){
        let mut painter=RecordingPainter::new("monospace",402,vec![]);
        let texts:Vec<_>=(0..12).map(|job|format!("```rust\n{}",format!("unique scoped job {job} line\n").repeat(200))).collect();
        fence(&mut painter,&texts[0],1.0).unwrap();
        let _budget=ForcedBudget::new(1);
        for scope in [11,22,22,22,11]{
            begin_preparation_scoped(scope);
            for text in &texts{fence(&mut painter,text,1.0).unwrap();}
            assert!(end_preparation());
        }
        FENCES.with(|c|{
            let c=c.borrow();assert_eq!(c.len(),24);
            assert_eq!(c.iter().filter(|e|e.visit.unwrap().scope==11).map(|e|e.measured).sum::<usize>(),2);
            assert_eq!(c.iter().filter(|e|e.visit.unwrap().scope==22).map(|e|e.measured).sum::<usize>(),3);
        });
        for scope in 100..120{
            begin_preparation_scoped(scope);fence(&mut painter,&texts[0],1.0).unwrap();end_preparation();
        }
        FENCES.with(|c|assert_eq!(c.borrow().len(),8));
        SCOPES.with(|c|assert_eq!(c.borrow().len(),8));
        release_preparation_scope(119);
        FENCES.with(|c|{
            let c=c.borrow();assert_eq!(c.len(),7);
            assert!(c.iter().all(|e|e.visit.unwrap().scope!=119));
        });
        SCOPES.with(|c|assert_eq!(c.borrow().len(),7));
    }
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
        let _budget=ForcedBudget::new(17);
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
