//! Original Yrs document binding for the canonical code/Markdown document.
//! Views and renderers stay independent; only this owner publishes edits.
use neoism_protocol::crdt::{CrdtClientMessage as Out, CrdtServerMessage as In, CrdtSyncEnvelope};
use neoism_ui::editor::{code::{doc_sync::CodeDocBinding,CodeBuffer,CodeDocHistoryRequest},crdt::CrdtTextUpdate};
use serde_json::{json,Value};

pub struct EditorCollaboration {
    pub binding: CodeDocBinding,
    outbound: Vec<Out>,
    initial_text: String,
    initial_revision: u64,
    initial_dirty: bool,
    pending: Vec<CrdtTextUpdate>,
}
impl EditorCollaboration {
    pub fn new(client:u64,path:&str,buffer:&CodeBuffer)->Self {
        let id=neoism_ui::editor::crdt::presence_buffer_id_for_path(std::path::Path::new(path));
        Self { binding:CodeDocBinding::new(client,&id),outbound:vec![Out::OpenBuffer{buffer_id:id,initial_text:buffer.text_for_disk()}],initial_text:if buffer.is_dirty(){buffer.saved_text_for_disk()}else{buffer.text_for_disk()},initial_revision:buffer.revision,initial_dirty:buffer.is_dirty(),pending:Vec::new() }
    }
    fn push(&mut self,update:Option<CrdtTextUpdate>){if let Some(update)=update {self.pending.push(update.clone());self.outbound.push(Out::ApplySync{envelope:CrdtSyncEnvelope{buffer_id:self.binding.buffer_id().into(),origin_client_id:update.origin_client_id,update_v1:update.update_v1,state_vector_v1:update.state_vector_v1}});}}
    pub fn history(&mut self,buffer:&mut CodeBuffer,redo:bool){let result=if redo{self.binding.redo(buffer)}else{self.binding.undo(buffer)};self.push(result.flushed_local);self.push(result.history_update);}
    pub fn drain(&mut self,buffer:&mut CodeBuffer){
        buffer.set_doc_history_bound(self.binding.is_seeded());
        for action in buffer.take_doc_history_requests(){self.history(buffer,matches!(action,CodeDocHistoryRequest::Redo));}
        let update=self.binding.flush_local(buffer);self.push(update);
    }
    pub fn take(&mut self,buffer:&mut CodeBuffer)->Value{self.drain(buffer);json!({"send":std::mem::take(&mut self.outbound),"seeded":self.binding.is_seeded(),"bufferId":self.binding.buffer_id()})}
    pub fn reconnect(&mut self,buffer:&mut CodeBuffer){
        self.drain(buffer);
        // Pending raw transactions must be acknowledged individually. A full
        // catch-up update has different bytes and cannot retire their entries.
        self.outbound.clear();
        self.outbound.push(Out::OpenBuffer{buffer_id:self.binding.buffer_id().into(),initial_text:if self.binding.is_seeded(){self.binding.doc_text()}else{buffer.text_for_disk()}});
        if self.binding.is_seeded(){
            for update in self.pending.clone(){self.outbound.push(Out::ApplySync{envelope:CrdtSyncEnvelope{buffer_id:self.binding.buffer_id().into(),origin_client_id:update.origin_client_id,update_v1:update.update_v1,state_vector_v1:update.state_vector_v1}});}
            let update=self.binding.encode_full_update_v1();self.push(Some(update));self.outbound.push(Out::RequestSnapshot{buffer_id:self.binding.buffer_id().into(),state_vector_v1:self.binding.state_vector_v1()});}
    }
    /// Persist causal identity and unsent transactions, not just source bytes.
    /// Restoring source alone would turn stale recovery into a new replacement
    /// of the entire current shared document after an SSH/app reconnect.
    pub fn snapshot(&mut self,buffer:&mut CodeBuffer)->Value {
        self.drain(buffer);
        json!({"version":1,"bufferId":self.binding.buffer_id(),"clientId":self.binding.client_id(),"seeded":self.binding.is_seeded(),"document":self.binding.encode_full_update_v1(),"pending":self.pending,"initialText":self.initial_text,"initialRevision":self.initial_revision,"initialDirty":self.initial_dirty})
    }
    pub fn restore(value:&Value,buffer:&mut CodeBuffer)->Result<Self,String>{
        if value["version"].as_u64()!=Some(1){return Err("Unsupported shared document recovery version".into());}
        let id=value["bufferId"].as_str().filter(|id|!id.is_empty()).ok_or("Missing shared recovery buffer id")?;
        let client=value["clientId"].as_u64().filter(|id|*id>0&&*id<=9_007_199_254_740_991).ok_or("Invalid shared recovery actor id")?;
        let document:CrdtTextUpdate=serde_json::from_value(value["document"].clone()).map_err(|error|error.to_string())?;
        if document.origin_client_id!=client{return Err("Shared recovery actor does not match document".into());}
        let pending:Vec<CrdtTextUpdate>=serde_json::from_value(value["pending"].clone()).map_err(|error|error.to_string())?;
        if pending.iter().any(|update|update.origin_client_id!=client){return Err("Shared recovery pending actor mismatch".into());}
        let mut binding=CodeDocBinding::new(client,id);
        if value["seeded"].as_bool().unwrap_or(false){
            // Seed only the replica. The actual recovered view keeps its dirty
            // baseline, caret, selection and local bytes until causal merge.
            let mut replica_view=CodeBuffer::from_text("");
            binding.seed_from_snapshot(&document.update_v1,&mut replica_view).map_err(|error|error.to_string())?;
        }
        buffer.set_doc_history_bound(binding.is_seeded());
        Ok(Self {binding,outbound:Vec::new(),pending,initial_text:value["initialText"].as_str().unwrap_or_default().into(),initial_revision:value["initialRevision"].as_u64().unwrap_or(buffer.revision),initial_dirty:value["initialDirty"].as_bool().unwrap_or(buffer.is_dirty())})
    }
    pub fn event(&mut self,message:In,buffer:&mut CodeBuffer)->Result<bool,String>{
        let id=self.binding.buffer_id().to_owned();
        match message {
            In::Snapshot{buffer_id,update_v1,..}|In::SnapshotFallback{buffer_id,update_v1,..} if buffer_id==id=>{
                if self.binding.is_seeded(){let applied=self.binding.apply_remote(0,&update_v1,buffer).map_err(|e|e.to_string())?;self.push(applied.flushed_local);Ok(applied.changed)}
                else {
                    // Recovery-owned dirty bytes and typing during the initial
                    // snapshot wait stay in the actual editor, including undo.
                    let preserve=buffer.is_dirty()||buffer.revision!=self.initial_revision||buffer.text_for_disk()!=self.initial_text;
                    let changed=if preserve {
                        let local=buffer.text_for_disk();
                        let mut seed=CodeBuffer::from_text("");self.binding.seed_from_snapshot(&update_v1,&mut seed).map_err(|e|e.to_string())?;
                        let remote=self.binding.doc_text();
                        let desired=rebase_pending(&self.initial_text,&local,&remote);
                        // Match the binding's complete-file projection, including
                        // EOF/CRLF bytes, before flushing pending keystrokes.
                        self.binding.seed_from_snapshot(&update_v1,buffer).map_err(|e|e.to_string())?;
                        if let Some(delta)=neoism_ui::editor::markdown::doc_sync::diff_doc_texts(&remote,&desired){neoism_ui::editor::code::doc_sync::apply_remote_delta_to_buffer(buffer,&delta);}
                        let update=self.binding.flush_local(buffer);self.push(update);true
                    }else{self.binding.seed_from_snapshot(&update_v1,buffer).map_err(|e|e.to_string())?};
                    buffer.set_doc_history_bound(true);Ok(changed)
                }
            },
            In::Sync{envelope} if envelope.buffer_id==id=>{if envelope.origin_client_id==self.binding.client_id(){self.pending.retain(|update|update.update_v1!=envelope.update_v1);}let result=self.binding.apply_remote(envelope.origin_client_id,&envelope.update_v1,buffer).map_err(|e|e.to_string())?;self.push(result.flushed_local);Ok(result.changed)},
            In::Update{update} if update.buffer_id==id=>{if update.origin_client_id==self.binding.client_id(){self.pending.retain(|pending|pending.update_v1!=update.update_v1);}let result=self.binding.apply_remote(update.origin_client_id,&update.update_v1,buffer).map_err(|e|e.to_string())?;self.push(result.flushed_local);Ok(result.changed)},
            In::Saved{buffer_id,..} if buffer_id==id=>{if self.pending.is_empty(){buffer.mark_saved();}Ok(true)},
            In::Error{buffer_id:Some(buffer_id),message} if buffer_id==id&&message.contains("unknown CRDT buffer")=>{self.reconnect(buffer);Ok(false)},
            _=>Ok(false)
        }
    }
}

pub fn cursor(buffer:&CodeBuffer)->Value{
    fn point(buffer:&CodeBuffer,line:usize,col:usize)->Value {
        let line=line.min(buffer.lines.len().saturating_sub(1));let source=&buffer.lines[line];let mut col=col.min(source.len());while !source.is_char_boundary(col){col-=1;}
        let prefix=source[..col].encode_utf16().count();let offset=buffer.lines[..line].iter().map(|s|s.encode_utf16().count()+1).sum::<usize>()+prefix;
        json!({"line":line,"column":prefix,"offset":offset})
    }
    json!({"cursor":point(buffer,buffer.cursor_line,buffer.cursor_col),"selection":buffer.selection_range().map(|(a,b)|json!({"anchor":point(buffer,a.line,a.col),"head":point(buffer,b.line,b.col)})),"insert":buffer.mode==neoism_ui::editor::code::CodeMode::Insert})
}

/// Rebase keystrokes made while the first authoritative snapshot was in flight.
fn rebase_pending(base:&str,local:&str,remote:&str)->String {
    use neoism_ui::editor::markdown::doc_sync::diff_doc_texts;
    if local==remote{return remote.to_owned();}
    let Some(local_delta)=diff_doc_texts(base,local)else{return remote.to_owned();};
    let Some(remote_delta)=diff_doc_texts(base,remote)else{return local.to_owned();};
    let ls=local_delta.byte_start;let le=ls+local_delta.byte_removed;
    let rs=remote_delta.byte_start;let re=rs+remote_delta.byte_removed;
    let mut boundaries=vec![0,ls,le,rs,re,base.len()];boundaries.sort_unstable();boundaries.dedup();
    let mut merged=String::new();
    for (index,&position) in boundaries.iter().enumerate(){
        // Concurrent inserted characters survive a deletion of the original
        // surrounding range, just as they do when applying actual Yrs ops.
        // Never replace the already-live remote insertion with stale text.
        if position==rs{merged.push_str(&remote_delta.inserted);}
        if position==ls{merged.push_str(&local_delta.inserted);}
        if let Some(&next)=boundaries.get(index+1){
            if !(position>=ls&&position<le||position>=rs&&position<re){merged.push_str(&base[position..next]);}
        }
    }
    merged
}
