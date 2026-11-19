mod connection;
mod profile;
// The shared multiplayer generator's clock helper resolves this module name.
use profile as cursor_style;
mod fixture;
mod view;
mod calendar;
mod projects;
mod splash;
mod splash_art;
mod zoom;

use std::{collections::BTreeMap, sync::{OnceLock, mpsc}, time::{Duration, Instant, SystemTime, UNIX_EPOCH}};
use neochat::protocol::{Action, Message, Snapshot, TaskState, EventKind, Reaction, ScheduledPost, Attachment};
use rune::{AppSpec, Key, TextEdit, Tx};
use connection::{Command, Options, Update};

static OPTIONS: OnceLock<Options> = OnceLock::new();
const FILTERS: &[&str] = &["open", "waiting", "ready", "claimed", "blocked", "completed", "failed", "canceled", "all"];

#[derive(Clone, PartialEq)]
enum Page { Home, Activity, Tasks, Scheduled, Channel(String) }

#[derive(Clone, Copy, PartialEq)]
enum Dialog { Settings, Personal, BoardView, Connection, Tags, ReplyTags(u64), Filters, Schedule, NewChannel, DeleteThread(u64) }

#[derive(serde::Serialize, serde::Deserialize)]
struct Preferences { sidebar: bool, oldest: bool, compact: bool, #[serde(default)] project: String }

fn preferences_path() -> Option<std::path::PathBuf> {
    dirs::config_dir().map(|p| p.join("neochat").join("desktop.json"))
}

struct App {
    local_name: String,
    me: Option<neochat::protocol::Member>,
    personal_name: TextEdit,
    member_list: Option<(String,Result<Vec<neochat::protocol::Member>,String>)>,
    pick_assignee:bool,
    details_picker:Option<&'static str>,
    task_filters: TaskFilters,
    aliases:BTreeMap<String,String>,
    profiles: profile::Profiles,
    zoom: zoom::Zoom,
    snapshot: Snapshot,
    board_cache: Option<Snapshot>,
    board_extra:(Vec<Reaction>,Vec<ScheduledPost>),
    page: Page,
    project: &'static str,
    project_menu: bool,
    home_copied: Option<&'static str>,
    setup_open: bool,
    splash: splash::Splash,
    filter: usize,
    query: TextEdit,
    channel: TextEdit,
    draft: TextEdit,
    reply: TextEdit,
    labels: TextEdit,
    needs: TextEdit,
    assignee: TextEdit,
    new_channel: TextEdit,
    creating_channel: Option<String>,
    dialog: Option<Dialog>,
    compact: bool,
    compose_file_open: bool,
    compose_file_owner: Option<u64>,
    compose_file: TextEdit,
    sidebar: bool,
    oldest: bool,
    thread: Option<u64>,
    task_menu:Option<u64>,
    hover_row:Option<u64>, row_menu:Option<u64>, row_anchor:[f32;4],
    status: String,
    error: Option<String>,
    pending: bool,
    commands: Option<mpsc::SyncSender<Command>>,
    updates: Option<mpsc::Receiver<Update>>,
    now: u64,
    fixture: bool,
    seen: BTreeMap<String, u64>,
    positions: BTreeMap<u64, usize>,
    width: f32,
    feed_h: f32,
    thread_h: f32,
    reply_owner: Option<u64>,
    reply_drafts: BTreeMap<u64, TextEdit>,
    reply_labels: BTreeMap<u64, TextEdit>,
    sent_reply_labels: Option<(u64, String)>,
    reactions: Vec<Reaction>,
    schedules: Vec<ScheduledPost>,
    editing: Option<u64>,
    edit_text: TextEdit,
    edit_labels: TextEdit,
    reaction: TextEdit,
    schedule_at: TextEdit,
    show_schedule: bool,
    attachment_message: Option<u64>,
    attachments: BTreeMap<u64, Vec<Attachment>>,
    upload_path: TextEdit,
    download_path: TextEdit,
    download_id: Option<u64>,
    notice: Option<String>,
}

#[derive(Default)]
struct TaskFilters { channel:Option<String>, assignee:Option<String>, thread:Option<u64> }
impl TaskFilters {
    fn matches(&self,m:&Message,me:Option<&str>)->bool {
        self.channel.as_ref().is_none_or(|c|*c==m.channel) && self.thread.is_none_or(|id|m.id==id) && self.assignee.as_deref().is_none_or(|who|{
            let assigned=m.task.as_ref().and_then(|t|t.assignee.as_deref());
            match who {"@me"=>me.is_some() && me==assigned,""=>assigned.is_none(),actor=>Some(actor)==assigned}
        })
    }
    fn active(&self)->bool{self.channel.is_some()||self.assignee.is_some()||self.thread.is_some()}
}

fn edit(value: &str) -> TextEdit { TextEdit::new(value) }
fn state_name(state: &TaskState) -> &'static str {
    match state {
        TaskState::Ready => "ready", TaskState::Waiting => "waiting", TaskState::Claimed => "claimed",
        TaskState::Blocked => "blocked", TaskState::Completed => "completed", TaskState::Failed => "failed", TaskState::Canceled => "canceled",
    }
}
fn matches_filter(state: &TaskState, filter: usize) -> bool {
    let name = state_name(state);
    match FILTERS[filter] { "all" => true, "open" => !matches!(name, "completed" | "failed" | "canceled"), wanted => wanted == name }
}
fn parse_needs(value: &str) -> Result<Vec<u64>, String> {
    value.split(|c: char| c == ',' || c.is_whitespace()).filter(|v| !v.is_empty())
        .map(|v| v.trim_start_matches('#').parse().map_err(|_| format!("Invalid dependency: {v}"))).collect()
}
fn parse_labels(value: &str) -> Result<BTreeMap<String, String>, String> {
    let mut tokens = Vec::new();
    let (mut token, mut quoted, mut escaped) = (String::new(), false, false);
    for c in value.chars() {
        if escaped { token.push(c); escaped = false; }
        else if c == '\\' { escaped = true; }
        else if c == '"' { quoted = !quoted; }
        else if c.is_whitespace() && !quoted { if !token.is_empty() { tokens.push(std::mem::take(&mut token)); } }
        else { token.push(c); }
    }
    if quoted || escaped { return Err("Unclosed label quote or escape".into()); }
    if !token.is_empty() { tokens.push(token); }
    let mut labels = BTreeMap::new();
    for token in tokens {
        let (k,v) = token.split_once('=').filter(|(k,_)| !k.is_empty()).ok_or_else(|| format!("Expected key=value: {token}"))?;
        if labels.insert(k.to_string(),v.to_string()).is_some() { return Err(format!("Duplicate label: {k}")); }
    }
    Ok(labels)
}
fn format_labels(labels: &BTreeMap<String, String>) -> String {
    labels.iter().map(|(k,v)| format!("{k}=\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\""))).collect::<Vec<_>>().join(" ")
}
fn month_days(year: u64) -> [u64;12] {
    [31, if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) {29} else {28},31,30,31,30,31,31,30,31,30,31]
}
fn parse_schedule_time(value: &str) -> Result<u64, String> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<u64>() { return Ok(seconds); }
    let invalid = || "Use YYYY-MM-DD HH:MM (UTC) or Unix seconds".to_string();
    let (date,time) = value.split_once(' ').ok_or_else(invalid)?;
    let date: Vec<u64> = date.split('-').map(str::parse).collect::<Result<_,_>>().map_err(|_| invalid())?;
    let time: Vec<u64> = time.split(':').map(str::parse).collect::<Result<_,_>>().map_err(|_| invalid())?;
    if date.len()!=3 || time.len()!=2 { return Err(invalid()); }
    let (year,month,day) = (date[0],date[1],date[2]);
    if !(1970..=9999).contains(&year) || !(1..=12).contains(&month) || day == 0
        || day > month_days(year)[month as usize-1] || time[0]>23 || time[1]>59 { return Err(invalid()); }
    let days = (1970..year).map(|y| month_days(y).iter().sum::<u64>()).sum::<u64>()
        + month_days(year)[..month as usize-1].iter().sum::<u64>() + day-1;
    Ok(days*86400 + time[0]*3600 + time[1]*60)
}
fn format_utc(seconds: u64) -> String {
    let mut days = seconds/86400;
    let mut year = 1970;
    while year <= 9999 {
        let n = month_days(year).iter().sum::<u64>();
        if days < n { break; } days -= n; year += 1;
    }
    if year > 9999 { return format!("{seconds} Unix seconds"); }
    let mut month = 1;
    for n in month_days(year) { if days < n { break; } days -= n; month += 1; }
    format!("{year:04}-{month:02}-{:02} {:02}:{:02}",days+1,seconds%86400/3600,seconds%3600/60)
}

impl App {
    fn new() -> Self {
        let fixture = OPTIONS.get().is_some_and(|o| o.fixture);
        let mut app = Self {
            local_name: OPTIONS.get().and_then(|o| o.profile.clone()).unwrap_or_else(|| profile::local_name(fixture)),
            profiles: profile::Profiles::default(),
            zoom: zoom::Zoom::default(),
            snapshot: Snapshot { cursor: 0, channels: vec![], messages: vec![] }, board_cache:None,board_extra:Default::default(),page: Page::Home, project:"neoboard", project_menu:false, home_copied: None, setup_open:false, splash: Default::default(),
            me:None, personal_name:edit(""), member_list:None, pick_assignee:false, details_picker:None, task_filters:Default::default(), aliases:BTreeMap::new(),
            filter: 0, query: edit(""), channel: edit("neoboard/general"), draft: edit(""), reply: edit(""),
            labels: edit(""), needs: edit(""), assignee: edit(""), new_channel: edit(""), creating_channel: None,
            dialog: None, compact: true, compose_file_open: false, compose_file_owner: None, compose_file: edit(""), sidebar: true, oldest: true, thread: None, task_menu:None,hover_row:None,row_menu:None,row_anchor:[0.;4], status: "connecting".into(), error: None,
            pending: false, commands: None, updates: None, now: 0, fixture, seen: BTreeMap::new(), positions: BTreeMap::new(),
            width: 1252.0, feed_h: 400.0, thread_h: 400.0,
            reply_owner: None, reply_drafts: BTreeMap::new(),
            reply_labels: BTreeMap::new(), sent_reply_labels: None,
            reactions: vec![], schedules: vec![], editing: None, edit_text: edit(""), edit_labels: edit(""),
            reaction: edit("+1"), schedule_at: edit(""), show_schedule: false,
            attachment_message: None, attachments: BTreeMap::new(), upload_path: edit(""), download_path: edit(""), download_id: None, notice: None,
        };
        if fixture { fixture::load(&mut app, false); }
        else if !cfg!(test) && let Some(prefs) = preferences_path().and_then(|p| std::fs::read(p).ok()).and_then(|data| serde_json::from_slice::<Preferences>(&data).ok()) {
            app.sidebar=prefs.sidebar; app.oldest=prefs.oldest; app.compact=prefs.compact;
            if let Some((_,id))=projects::PROJECTS.iter().find(|(_,p)|*p==prefs.project){app.project=id;}
        }
        if let Some(id)=OPTIONS.get().and_then(|o|o.project){app.project=id;}
        if !fixture {app.channel=edit(&projects::channel(app.project,"general"));}
        app
    }
    fn save_preferences(&mut self) {
        if self.fixture || cfg!(test) { return; }
        let Some(path)=preferences_path() else {self.error=Some("No settings directory available".into());return;};
        let prefs=Preferences {sidebar:self.sidebar, oldest:self.oldest, compact:self.compact, project:self.project.into()};
        let result=(|| -> std::io::Result<()> {
            std::fs::create_dir_all(path.parent().unwrap())?;
            let tmp=path.with_extension(format!("{}.tmp",std::process::id()));
            std::fs::write(&tmp,serde_json::to_vec_pretty(&prefs)?)?;
            std::fs::rename(tmp,path)
        })();
        if let Err(e)=result {self.error=Some(format!("Could not save preferences: {e}"));}
    }
    fn navigate(&mut self, page: Page) {
        self.project_menu=false;
        self.home_copied=None;
        self.row_menu=None;self.hover_row=None;
        self.setup_open=false;
        if page==Page::Home {self.thread=None;}
        if page == Page::Scheduled { self.show_schedule = true; }
        else if self.page == Page::Scheduled { self.show_schedule = false; }
        if let Page::Channel(channel) = &page {
            self.channel = edit(channel);
            self.seen.insert(channel.clone(), self.snapshot.messages.iter().map(|m| m.id).max().unwrap_or(0));
        }
        self.page = page;
    }
    fn create_channel(&mut self) {
        if self.pending { return; }
        let name=self.new_channel.text.trim().trim_start_matches('#').to_owned();
        if name.is_empty() { self.error=Some("Enter a channel name.".into()); return; }
        let name=projects::channel(self.project,&name);
        self.dispatch(Action::CreateChannel{channel:name.clone()},None);
        if self.pending {self.creating_channel=Some(name);}
    }
    fn visible(&self) -> Vec<&Message> {
        let query = self.query.text.to_lowercase();
        let mut messages: Vec<_> = self.snapshot.messages.iter().filter(|m| {
            let in_page = match &self.page {
                Page::Home => false,
                Page::Activity => true,
                Page::Scheduled => false,
                Page::Channel(c) => &m.channel == c && m.parent.is_none(),
                Page::Tasks => m.parent.is_none() && m.task.as_ref().is_some_and(|t| matches_filter(&t.state, self.filter)),
            };
            in_page && projects::contains(self.project,&m.channel) && (self.page!=Page::Tasks || self.task_filters.matches(m,self.me.as_ref().map(|u|u.actor.as_str()))) && (query.is_empty() || format!("{} {} {} {:?}", m.id, m.author, m.text, m.labels).to_lowercase().contains(&query))
        }).collect();
        messages.sort_by_key(|m| (m.created_at, m.id));
        if !self.oldest { messages.reverse(); }
        messages
    }
    fn open_thread(&mut self, id: u64) {
        let Some(message)=self.snapshot.messages.iter().find(|m|m.id==id && projects::contains(self.project,&m.channel)) else{return};
        let root=message.parent.unwrap_or(id);
        if self.reply_owner != Some(root) {
            if let Some(previous) = self.reply_owner { self.reply_drafts.insert(previous, self.reply.clone()); }
            self.reply = self.reply_drafts.remove(&root).unwrap_or_else(|| edit(""));
            self.reply_owner = Some(root);
        }
        self.thread = Some(root);
    }
    fn actor_label<'a>(&'a self, author:&'a str)->&'a str {
        let actor=author.split('/').next().unwrap_or(author);
        self.me.iter().chain(self.member_list.iter().flat_map(|(_,r)|r.as_ref().into_iter().flatten()))
            .find(|m|m.actor==actor).and_then(|m|m.display_name.as_deref()).or_else(||self.aliases.get(actor).map(String::as_str)).unwrap_or_else(||{
                if author!="local" {return profile::display_name(author);}
                if self.me.as_ref().is_some_and(|m|m.actor=="local") || OPTIONS.get().is_none_or(|o|o.endpoint.is_none()) {return &self.local_name;}
                self.snapshot.messages.iter().rev().find_map(|m|m.author.strip_prefix("local/")).unwrap_or("Board host")
            })
    }
    fn show_assignees(&mut self){
        self.details_picker=None;
        self.pick_assignee=!self.pick_assignee;
        if !self.pick_assignee{return;}
        let channel=projects::channel(self.project,self.channel.text.trim().trim_start_matches('#'));
        self.member_list=None;
        if let Some(tx)=&self.commands {if tx.try_send(Command::Members(channel.clone())).is_ok(){return;}}
        self.member_list=Some((channel,Err("Connect to the board to list members.".into())));
    }
    fn select_project(&mut self, project:&'static str) {
        self.project_menu=false;
        if self.project==project {return;}
        if self.pending || !self.draft.text.trim().is_empty() || self.compose_file_open {
            self.error=Some("Finish or clear your current draft before switching projects.".into());return;
        }
        self.project=project;self.thread=None;self.query=edit("");self.filter=0;self.task_filters=Default::default();
        self.row_menu=None;self.hover_row=None;
        if self.board_cache.is_none(){self.board_cache=Some(self.snapshot.clone());}
        self.refresh_project();self.refresh_extra();
        self.member_list=None;self.aliases.clear();self.pick_assignee=false;self.details_picker=None;self.dialog=None;
        self.task_menu=None;self.editing=None;self.attachment_message=None;self.download_id=None;self.attachments.clear();self.error=None;self.notice=None;
        self.channel=edit(&projects::channel(project,"general"));
        self.labels=edit("");self.needs=edit("");self.assignee=edit("");
        if matches!(self.page, Page::Channel(_)) { self.navigate(Page::Activity); }
        self.save_preferences();
    }
    fn refresh_project(&mut self){
        if let Some(board)=&self.board_cache {
            self.snapshot=Snapshot{cursor:board.cursor,channels:board.channels.iter().filter(|c|projects::contains(self.project,c)).cloned().collect(),messages:board.messages.iter().filter(|m|projects::contains(self.project,&m.channel)).cloned().collect()};
            self.positions=self.snapshot.messages.iter().enumerate().map(|(i,m)|(m.id,i)).collect();
        }
    }
    fn refresh_extra(&mut self){
        self.reactions=self.board_extra.0.iter().filter(|r|projects::contains(self.project,&r.channel)).cloned().collect();
        self.schedules=self.board_extra.1.iter().filter(|s|projects::contains(self.project,&s.channel)).cloned().collect();
    }
    fn dispatch(&mut self, action: Action, draft: Option<(Option<u64>, String)>) {
        if self.pending { return; }
        self.error = None;
        self.notice = None;
        if !self.fixture && self.status != "live" {
            self.error = Some("Not connected; draft retained.".into()); return;
        }
        if let Some(commands) = &self.commands {
            let attach = match &action { Action::Post {..} => self.compose_file_owner.is_none(), Action::Reply {thread,..} | Action::ReplyWithLabels {thread,..} => self.compose_file_owner == Some(*thread), _ => false };
            let command = if self.compose_file_open && !self.compose_file.text.is_empty() && attach {
                Command::PostWithFile { action, draft, path: self.compose_file.text.trim().into() }
            } else { Command::Action { action, draft } };
            match commands.try_send(command) {
                Ok(()) => self.pending = true,
                Err(_) => self.error = Some("Connection busy; draft retained. Try again shortly.".into()),
            }
        } else { self.error = Some(if self.fixture { "Read-only fixture; connect to a board to post." } else { "Not connected; draft retained." }.into()); }
    }
    fn begin_edit(&mut self, id: u64) {
        if self.editing == Some(id) { return; }
        if let Some(m) = self.snapshot.messages.iter().find(|m| m.id == id) {
            self.edit_text = edit(&m.text);
            self.edit_labels = edit(&format_labels(&m.labels));
            self.editing = Some(id);
        }
    }
    fn save_edit(&mut self, labels: bool) {
        let Some(id) = self.editing else { return; };
        let action = if labels {
            match parse_labels(&self.edit_labels.text) {
                Ok(labels) => Action::SetLabels { id, labels },
                Err(e) => { self.error = Some(e); return; }
            }
        } else {
            if self.edit_text.text.trim().is_empty() { self.error = Some("Message cannot be empty".into()); return; }
            Action::Edit { id, text: self.edit_text.text.clone() }
        };
        self.dispatch(action, None);
    }
    fn files(&mut self, id: u64) {
        self.attachment_message = Some(id);
        self.queue_file(Command::Attachments(id), false);
    }
    #[cfg(any(test, feature = "native-picker"))]
    fn picked_path(&mut self, result: rune::FileDialogResult, upload: bool) {
        match result {
            Ok(Some(path)) => match path.to_str() {
                Some(path) => { if upload { self.upload_path = edit(path); } else { self.download_path = edit(path); } }
                None => self.error = Some("Choose a path representable as UTF-8, or enter a different path".into()),
            },
            Ok(None) => {} // Cancellation leaves the prior field/draft intact.
            Err(e) => self.error = Some(format!("File picker: {e}; use the path field instead")),
        }
    }
    fn queue_file(&mut self, command: Command, write: bool) {
        if self.pending { return; }
        self.error = None; self.notice = None;
        if let Some(tx) = &self.commands {
            if self.status != "live" && !self.fixture { self.error = Some("Not connected".into()); return; }
            match tx.try_send(command) {
                Ok(()) => { if write { self.pending = true; } }
                Err(_) => self.error = Some("Connection busy; try again shortly".into()),
            }
        } else { self.error = Some("Read-only fixture; connect to a board for attachments".into()); }
    }
    fn schedule(&mut self) {
        if self.compose_file_open && self.compose_file_owner.is_none() { self.error=Some("Scheduled posts cannot include a file. Remove the attachment or send now.".into()); return; }
        if self.draft.text.trim().is_empty() { self.error = Some("Write a post to schedule".into()); return; }
        let publish_at = match parse_schedule_time(&self.schedule_at.text) {
            Ok(t) if t > self.now => t,
            Ok(_) => { self.error = Some("Schedule time must be in the future (UTC)".into()); return; }
            Err(e) => { self.error = Some(e); return; }
        };
        let labels = match parse_labels(&self.labels.text) { Ok(l) => l, Err(e) => { self.error = Some(e); return; } };
        let channel = self.channel.text.trim().trim_start_matches('#').to_string();
        if channel.is_empty() { self.error = Some("Choose a channel first".into()); return; }
        let channel=projects::channel(self.project,&channel);
        let text = self.draft.text.clone();
        self.dispatch(Action::Schedule { channel, text: text.clone(), publish_at, labels }, Some((None,text)));
    }
    fn submit(&mut self, reply: bool) {
        if !reply && self.show_schedule { self.schedule(); return; }
        let typed = if reply { &self.reply.text } else { &self.draft.text }.clone();
        let text = if typed.trim().is_empty() && self.compose_file_open && self.compose_file_owner == (if reply { self.thread } else { None }) {
            std::path::Path::new(&self.compose_file.text).file_name().unwrap_or_default().to_string_lossy().into_owned()
        } else { typed.clone() };
        if text.trim().is_empty() || self.pending { return; }
        let action = if reply {
            let Some(thread) = self.thread else { return; };
            let labels = match parse_labels(self.reply_labels.get(&thread).map_or("", |l| l.text.as_str())) {
                Ok(labels) => labels, Err(e) => { self.error=Some(e); return; }
            };
            if labels.is_empty() { Action::Reply { thread, text: text.clone() } }
            else { Action::ReplyWithLabels { thread, text: text.clone(), labels } }
        } else {
            let channel = self.channel.text.trim().trim_start_matches('#').to_string();
            if channel.is_empty() { self.error = Some("Choose a channel first.".into()); return; }
            let channel=projects::channel(self.project,&channel);
            let labels = match parse_labels(&self.labels.text) { Ok(l) => l, Err(e) => { self.error = Some(e); return; } };
            let needs = match parse_needs(&self.needs.text) { Ok(n) => n, Err(e) => { self.error = Some(e); return; } };
            Action::Post { channel, text: text.clone(), labels, task: self.page == Page::Tasks, needs,
                assignee: (!self.assignee.text.trim().is_empty()).then(|| self.assignee.text.trim().into()) }
        };
        let sent_labels = if reply { self.thread.map(|id| (id,self.reply_labels.get(&id).map_or(String::new(), |l| l.text.clone()))) } else { None };
        self.dispatch(action, Some((if reply { self.thread } else { None }, typed)));
        if self.pending { self.sent_reply_labels=sent_labels; }
    }
}

fn setup(app: &mut App, wake: rune::Waker) {
    if app.fixture { return; }
    let (commands, updates) = connection::start(OPTIONS.get().cloned().unwrap_or_default(), wake);
    app.commands = Some(commands);
    app.updates = Some(updates);
}
fn poll(app: &mut App, seconds: f64, tx: &mut Tx) -> bool {
    let started = Instant::now();
    let mut changed = false;
    for index in 0..16 {
        if started.elapsed() >= Duration::from_millis(1) { tx.wake_after(Duration::ZERO); break; }
        let Some(update) = app.updates.as_ref().and_then(|rx| rx.try_recv().ok()) else { break; };
        if index == 15 { tx.wake_after(Duration::ZERO); }
        changed = true;
        match update {
            Update::Snapshot(mut snapshot) => {
                snapshot.messages.sort_by_key(|m|m.id);
                app.board_cache=Some(snapshot);app.refresh_project(); app.status = "live".into();
            }
            Update::Event(event) => {
                if event.cursor <= app.snapshot.cursor { continue; }
                app.snapshot.cursor = event.cursor;
                if let Some(board)=&mut app.board_cache{board.cursor=event.cursor;}
                match event.kind {
                    EventKind::Message { message } => {
                        if let Some(board)=&mut app.board_cache{match board.messages.binary_search_by_key(&message.id,|m|m.id){Ok(i)=>board.messages[i]=message.clone(),Err(i)=>board.messages.insert(i,message.clone())}}
                        if !projects::contains(app.project,&message.channel){continue;}
                        if let Some(&i) = app.positions.get(&message.id) { app.snapshot.messages[i] = message; }
                        else {
                            app.positions.insert(message.id, app.snapshot.messages.len());
                            app.snapshot.messages.push(message);
                        }
                    }
                    EventKind::ThreadDeleted{id,..}=>{
                        if let Some(board)=&mut app.board_cache{board.messages.retain(|m|m.id!=id && m.parent!=Some(id));}app.refresh_project();
                        if app.thread==Some(id){app.thread=None;app.reply_owner=None;app.reply=edit("");}
                        app.reply_drafts.remove(&id);app.reply_labels.remove(&id);
                        if app.dialog==Some(Dialog::DeleteThread(id)){app.dialog=None;}
                        if app.row_menu==Some(id){app.row_menu=None;}
                    }
                    EventKind::ChannelCreated { channel } => {
                        if let Some(board)=&mut app.board_cache && !board.channels.contains(&channel){board.channels.push(channel.clone());}
                        if !projects::contains(app.project,&channel){continue;}
                        if !app.snapshot.channels.contains(&channel) { app.snapshot.channels.push(channel); }
                    }
                    _ => {} // Separate worker fetches hydrate reaction/schedule state.
                }
            }
            Update::Status(status) => app.status = status,
            Update::Extra(result) => match result {
                Ok(extra) => {app.board_extra=extra;app.refresh_extra();}
                Err(e) => app.error = Some(format!("Could not refresh reactions/scheduled posts: {e}")),
            },
            Update::Members(channel,result)=>{if projects::contains(app.project,&channel){app.member_list=Some((channel,result));}}
            Update::Names(channel,list)=>if projects::contains(app.project,&channel){for m in list {if let Some(name)=m.display_name{app.aliases.insert(m.actor,name);}else{app.aliases.remove(&m.actor);}}},
            Update::Personal(result)=>match result {Ok(member)=>{app.aliases.remove(&member.actor);app.me=Some(member);},Err(e)=>app.error=Some(e)},
            Update::Attachments(id, result) => if app.positions.contains_key(&id){match result {
                Ok(files) => { app.attachments.insert(id, files); }
                Err(e) => app.error = Some(format!("Attachments: {e}")),
            }},
            Update::FileDone(result) => {
                app.pending = false;
                match result { Ok(s) => app.notice = Some(s), Err(e) => app.error = Some(format!("{e} · check before retrying")) }
            }
            Update::SentFile(path, message) => {
                if app.compose_file.text.trim()==path.to_string_lossy() { app.compose_file=edit(""); app.compose_file_open=false; }
                app.notice=Some(message);
            }
            Update::Done(draft, result) => {
                if let Some((id,sent))=app.sent_reply_labels.take() && result.is_ok() {
                    if app.reply_labels.get(&id).is_some_and(|l|l.text==sent) { app.reply_labels.remove(&id); }
                }
                if let Some(channel)=app.creating_channel.take() && result.is_ok() && app.dialog==Some(Dialog::NewChannel) {
                    app.dialog=None; app.new_channel=edit("");app.thread=None;app.navigate(Page::Channel(channel));
                }
                app.pending = false;
                match result {
                    Ok(()) => {
                        app.notice = None;
                        if let Some((owner, sent)) = draft {
                            let field = match owner {
                                None => Some(&mut app.draft),
                                Some(id) if app.reply_owner == Some(id) => Some(&mut app.reply),
                                Some(id) => app.reply_drafts.get_mut(&id),
                            };
                            if let Some(field) = field { if field.text == sent { *field = edit(""); } }
                        }
                    }
                    Err(e) => app.error = Some(format!("{e} · draft retained; check the board before retrying")),
                }
            }
        }
    }
    if !app.fixture {
        // Match surfaces that can contain an orb; Tasks alone need no animation timer.
        let sidebar_visible = app.sidebar && (app.width >= 900.0 || app.thread.is_none());
        let feed_visible = matches!(app.page, Page::Activity | Page::Channel(_)) && !app.visible().is_empty();
        if sidebar_visible || feed_visible || app.thread.is_some() {
            tx.wake_after(app.profiles.tick(Instant::now()));
        }
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
        if now / 60 != app.now / 60 { changed = true; }
        app.now = now;
        tx.wake_after(Duration::from_secs(30));
    }
    app.splash.tick(seconds,app.page==Page::Home && app.dialog.is_none() && !app.setup_open,tx) || changed
}
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("Neoboard — native board inspector\n\n--endpoint URL --token TOKEN  Remote board (or NEOCHAT_TOKEN)\n--profile NAME                Actor profile\n--data-dir PATH               Local service discovery/bootstrap\n--fixture                     Offline read-only fixture\n--scene tasks-sparse|tasks-dense|activity|thread\n--snap PATH --size 1252x674 --scale 2  Headless Rune PNG");
        return;
    }
    let opts = Options::parse(&args).unwrap_or_else(|e| { eprintln!("{e}"); std::process::exit(2) });
    let _ = OPTIONS.set(opts);
    rune::set_app_id("ai.neoism.neochat");
    rune::run(AppSpec {
        fonts: &[include_bytes!("../../../rune/app/assets/fonts/GeistMono-Regular.otf")],
        families: ("Geist Mono", "Geist Mono"),
        setup: Some(setup), poll: Some(poll), min_size: Some((620.0, 420.0)),
        on_key: Some(|app, e, tx| {
            if app.zoom.key(e, tx) { return true; }
            if e.key == Key::Escape {
                if app.project_menu {app.project_menu=false;return true;}
                if app.row_menu.take().is_some(){return true;}
                if app.setup_open {app.setup_open=false;return true;}
                if app.dialog.take().is_some() { return true; }
                if app.editing.is_some() { app.editing = None; }
                else if app.attachment_message.is_some() { app.attachment_message = None; }
                else { app.thread = None; }
                return true;
            }
            false
        }),
        scenes: &[("tasks-sparse", |a| fixture::load(a, false)), ("tasks-dense", |a| fixture::load(a, true)),
            ("settings", |a| {fixture::load(a,true); a.dialog=Some(Dialog::Settings);}),
            ("personal", |a| {fixture::load(a,true);a.dialog=Some(Dialog::Personal);}),
            ("task-filters", |a| {fixture::load(a,true);a.dialog=Some(Dialog::Filters);}),
            ("task-dependencies", |a| {fixture::load(a,true);a.dialog=Some(Dialog::Tags);a.details_picker=Some("pick-needs");}),
            ("new-channel", |a| {fixture::load(a,true);a.dialog=Some(Dialog::NewChannel);}),
            ("home", |a| {fixture::load(a,true);a.navigate(Page::Home);}),
            ("home-setup", |a| {fixture::load(a,true);a.navigate(Page::Home);a.setup_open=true;}),
            ("channel-thread", |a| {fixture::load(a,true);a.navigate(Page::Channel(projects::channel(a.project,"dev/rune")));a.open_thread(1);}),
            ("preferences", |a| {fixture::load(a,true); a.dialog=Some(Dialog::BoardView);}),
            ("activity", |a| { fixture::load(a, true); a.page = Page::Activity; }),
            ("thread", |a| { fixture::load(a, true); a.page = Page::Activity; a.open_thread(1); }),
            ("reply-labels", |a| { fixture::load(a,true);a.page=Page::Activity;a.open_thread(1);a.reply_labels.insert(1,edit("area=\"desktop UI\""));a.dialog=Some(Dialog::ReplyTags(1)); }),
            ("completed-task", |a| {fixture::load(a,true);a.filter=5;a.open_thread(7);}),
            ("editing", |a| { fixture::load(a, true); a.page = Page::Activity; a.open_thread(1); a.begin_edit(100); }),
            ("attachments", |a| { fixture::load(a, true); a.page = Page::Activity; a.open_thread(1); a.attachment_message = Some(1); a.download_id = Some(1); }),
            ("scheduled", |a| { fixture::load(a, true); a.page = Page::Scheduled; a.show_schedule = true; a.schedule_at = edit(&(a.now + 3600).to_string()); })],
        ..AppSpec::new("Neoboard", (1252.0, 674.0), App::new, view::root)
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn filters() {
        for state in [TaskState::Ready, TaskState::Waiting, TaskState::Claimed, TaskState::Blocked] { assert!(matches_filter(&state, 0)); }
        for state in [TaskState::Completed, TaskState::Failed, TaskState::Canceled] { assert!(!matches_filter(&state, 0)); assert!(matches_filter(&state, 8)); }
        assert!(matches_filter(&TaskState::Ready, 2));
        assert!(!matches_filter(&TaskState::Waiting, 2));
    }
    #[test] fn schedule_utc_dates_are_validated_and_roundtrip() {
        assert_eq!(parse_schedule_time("1970-01-01 00:00").unwrap(),0);
        assert_eq!(parse_schedule_time("2000-02-29 12:34").unwrap(),951827640);
        assert!(parse_schedule_time("2100-02-29 00:00").is_err());
        assert!(parse_schedule_time("2026-04-31 00:00").is_err());
        assert!(parse_schedule_time("2026-00-01 00:00").is_err());
        assert!(parse_schedule_time("2026-01-01 24:00").is_err());
        assert!(parse_schedule_time("99999999999-01-01 00:00").is_err());
        for seconds in [0,60,951827640,1_790_000_040,253402300740] {
            assert_eq!(parse_schedule_time(&format_utc(seconds)).unwrap(),seconds);
        }
        assert_eq!(parse_schedule_time("1790000040").unwrap(),1790000040);
    }
    #[test] fn composer_metadata() {
        assert_eq!(parse_needs("#12, 34 56").unwrap(), vec![12,34,56]);
        assert!(parse_needs("abc").is_err());
        assert_eq!(parse_labels("priority=high area=rune").unwrap().len(), 2);
        assert!(parse_labels("missing-value").is_err());
        let labels = [("area".into(), "Rust UI \"notes\" \\ snow 雪".into()), ("empty".into(), "".into())].into();
        assert_eq!(parse_labels(&format_labels(&labels)).unwrap(), labels);
        assert!(parse_labels("a=1 a=2").is_err());
        assert!(parse_labels("a=\"oops").is_err());
    }
    #[test] fn parent_navigation_and_search() {
        let mut app = App::new(); fixture::load(&mut app, true);
        app.open_thread(100); assert_eq!(app.thread, Some(1));
        app.query = edit("reconnect"); assert_eq!(app.visible().len(), 1);
    }
    #[test] fn hover_delete_requires_explicit_confirmation() {
        let mut app=App::new();fixture::load(&mut app,false);app.me=Some(neochat::protocol::Member{actor:"parker".into(),display_name:None});
        let (tx,rx)=mpsc::sync_channel(4);app.commands=Some(tx);
        let mut rt=rune::Runtime::new(app,view::root,(1252.,674.),rune::Env::new(true,1.));
        for _ in 0..5{rt.frame(1./60.);}let(x,y)=rt.find("task-1").unwrap();rt.pointer_move(x,y);
        for key in ["thread-options-1","delete-option","confirm-delete"]{
            for _ in 0..24{rt.frame(1./60.);}assert!(rx.try_recv().is_err());let(x,y)=rt.find(key).unwrap();rt.pointer_move(x,y);rt.pointer_down(x,y);rt.pointer_up(x,y);rt.frame(1./60.);
        }
        assert!(matches!(rx.try_recv(),Ok(Command::Action{action:Action::DeleteThread{id:1},..})));
    }
    #[test] fn assigned_thread_filters_intersect() {
        let mut app=App::new();fixture::load(&mut app,true);app.filter=FILTERS.len()-1;
        let task=&app.snapshot.messages[2];let id=task.id;
        app.task_filters=TaskFilters{channel:Some(task.channel.clone()),assignee:Some("@me".into()),thread:Some(id)};
        assert!(app.visible().is_empty());
        app.me=Some(neochat::protocol::Member{actor:"codex".into(),display_name:Some("Reviewer".into())});
        assert_eq!(app.visible().iter().map(|m|m.id).collect::<Vec<_>>(),vec![id]);
        app.task_filters.assignee=Some("".into());assert!(app.visible().is_empty());
    }
    #[test] fn project_dropdown_scopes_activity_and_composer() {
        let mut app=App::new();app.select_project("neoism");fixture::load(&mut app,true);app.navigate(Page::Activity);
        app.snapshot.messages[0].channel="neoboard/design".into();
        app.snapshot.messages[1].channel="neoboard-extra/design".into();
        app.snapshot.channels.extend(["neoboard/design".into(),"neoboard-extra/design".into()]);
        app.board_cache=Some(app.snapshot.clone());app.refresh_project();
        let mut rt=rune::Runtime::new(app,view::root,(1252.,674.),rune::Env::new(true,1.));
        for key in ["projects-toggle","project-neoboard"] {
            for _ in 0..5{rt.frame(1./60.);} let(x,y)=rt.find(key).unwrap();rt.pointer_move(x,y);rt.pointer_down(x,y);rt.pointer_up(x,y);
        }
        rt.frame(1./60.);assert_eq!(rt.state.project,"neoboard");assert_eq!(rt.state.visible().len(),1);assert_eq!(rt.state.channel.text,"neoboard/general");
        assert_eq!(rt.state.snapshot.channels,["neoboard/design"]);assert_eq!(rt.state.snapshot.messages.len(),1);
        rt.state.open_thread(2);assert_eq!(rt.state.thread,None);
    }
    #[test] fn channel_reply_badge_opens_thread_without_duplicating_replies() {
        let mut app=App::new();fixture::load(&mut app,false);app.navigate(Page::Channel(projects::channel(app.project,"dev/rune")));
        assert!(app.visible().iter().all(|m|m.parent.is_none()));
        let mut rt=rune::Runtime::new(app,view::root,(1252.,674.),rune::Env::new(true,1.));
        for _ in 0..5 {rt.frame(1./60.);}
        let (x,y)=rt.find("open-replies-1").unwrap();rt.pointer_move(x,y);rt.pointer_down(x,y);rt.pointer_up(x,y);
        for _ in 0..5 {rt.frame(1./60.);}
        assert_eq!(rt.state.thread,Some(1));assert!(rt.find("reply-100").is_some());assert!(rt.find("reply-input").is_some());assert!(rt.find("post-input").is_some());
    }
    #[test] fn reply_drafts_survive_thread_switch_and_close() {
        let mut app = App::new(); fixture::load(&mut app, false);
        app.open_thread(1); app.reply = edit("first draft");
        app.open_thread(2); app.reply = edit("second draft");
        app.open_thread(1); assert_eq!(app.reply.text, "first draft");
        app.thread = None; app.open_thread(1); assert_eq!(app.reply.text,"first draft");
        app.open_thread(2); assert_eq!(app.reply.text,"second draft");
    }
    #[test] fn reply_labels_dispatch_files_validate_and_preserve_root() {
        let mut app=App::new();fixture::load(&mut app,false);
        let (tx,rx)=mpsc::sync_channel(8);app.commands=Some(tx);
        app.labels=edit("root=keep");app.draft=edit("root draft");
        app.open_thread(1);app.reply=edit("first reply");app.reply_labels.insert(1,edit("area=\"desktop UI\""));
        app.open_thread(2);app.reply=edit("second reply");app.reply_labels.insert(2,edit("thread=two"));
        app.thread=None;app.open_thread(1);
        assert_eq!(app.reply.text,"first reply");assert_eq!(app.reply_labels[&1].text,"area=\"desktop UI\"");
        app.compose_file_open=true;app.compose_file_owner=Some(1);app.compose_file=edit("/tmp/reply.txt");app.submit(true);
        assert!(matches!(rx.try_recv().unwrap(),Command::PostWithFile {action:Action::ReplyWithLabels {thread:1,labels,..},path,..} if labels["area"]=="desktop UI" && path==std::path::Path::new("/tmp/reply.txt")));
        assert_eq!(app.labels.text,"root=keep");assert_eq!(app.draft.text,"root draft");
        app.pending=false;app.reply_labels.insert(1,edit("duplicate=a duplicate=b"));app.submit(true);
        assert!(rx.try_recv().is_err());assert!(app.error.is_some());assert_eq!(app.reply.text,"first reply");
        app.reply_labels.insert(1,edit(""));app.compose_file_owner=Some(2);app.submit(true);
        assert!(matches!(rx.try_recv().unwrap(),Command::Action {action:Action::Reply {thread:1,..},..}));
        assert_eq!(app.reply_labels[&2].text,"thread=two");
    }
    #[test] fn native_reply_label_dialog_is_thread_owned_and_success_clears_only_sent_labels() {
        let mut app=App::new();fixture::load(&mut app,false);app.open_thread(1);app.labels=edit("root=keep");
        let (commands,rx)=mpsc::sync_channel(8);app.commands=Some(commands);
        let (updates,inbox)=mpsc::sync_channel(8);app.updates=Some(inbox);
        let mut rt=rune::Runtime::new(app,view::root,(1252.,674.),rune::Env::new(true,1.));
        for _ in 0..5{rt.frame(1./60.);}
        let(x,y)=rt.find("reply-tags").unwrap();rt.pointer_move(x,y);rt.pointer_down(x,y);rt.pointer_up(x,y);rt.frame(1./60.);
        assert!(rt.state.dialog==Some(Dialog::ReplyTags(1)));
        rt.update(|_,tx|tx.focus("reply-labels",false));rt.frame(1./60.);
        rt.key(rune::KeyEvent {key:Key::Text("area=ui".into()),mods:Default::default()});rt.frame(1./60.);
        assert_eq!(rt.state.reply_labels[&1].text,"area=ui");assert_eq!(rt.state.labels.text,"root=keep");
        rt.state.dialog=None;rt.state.reply=edit("reply");rt.state.submit(true);rx.try_recv().unwrap();
        rt.state.open_thread(2);rt.state.reply_labels.insert(2,edit("second=keep"));
        updates.send(Update::Done(Some((Some(1),"reply".into())),Err("offline".into()))).unwrap();
        rt.update(|a,tx|{poll(a,0.,tx);});assert_eq!(rt.state.reply_labels[&1].text,"area=ui");
        rt.state.open_thread(1);rt.state.submit(true);rx.try_recv().unwrap();rt.state.open_thread(2);
        updates.send(Update::Done(Some((Some(1),"reply".into())),Ok(()))).unwrap();rt.update(|a,tx|{poll(a,0.,tx);});
        assert!(!rt.state.reply_labels.contains_key(&1));assert_eq!(rt.state.reply_labels[&2].text,"second=keep");
        assert_eq!(rt.state.labels.text,"root=keep");rt.state.open_thread(1);assert!(rt.state.reply.text.is_empty());
        rt.state.reply=edit("next reply");rt.state.reply_labels.insert(1,edit("area=old"));rt.state.submit(true);rx.try_recv().unwrap();
        rt.state.reply_labels.insert(1,edit("area=new"));
        updates.send(Update::Done(Some((Some(1),"next reply".into())),Ok(()))).unwrap();rt.update(|a,tx|{poll(a,0.,tx);});
        assert_eq!(rt.state.reply_labels[&1].text,"area=new");
    }
    #[test] fn native_composer_enter_and_shift_enter() {
        let mut app = App::new(); fixture::load(&mut app, false);
        let (tx, rx) = mpsc::sync_channel(4); app.commands = Some(tx);
        let mut rt = rune::Runtime::new(app, view::root, (1252.0,674.0), rune::Env::new(true,1.0));
        for _ in 0..5 { rt.frame(1.0/60.0); }
        rt.update(|_,tx| tx.focus("post-input",false));
        rt.frame(1.0/60.0);
        rt.key(rune::KeyEvent { key: Key::Text("Ship preview".into()), mods: Default::default() });
        rt.frame(1.0/60.0);
        rt.key(rune::KeyEvent { key: Key::Enter, mods: rune::Mods { shift:true, ..Default::default() } });
        rt.frame(1.0/60.0);
        assert!(rx.try_recv().is_err());
        assert!(rt.state.draft.text.contains('\n'));
        rt.key(rune::KeyEvent { key: Key::Enter, mods: Default::default() });
        rt.frame(1.0/60.0);
        let command = rx.try_recv().unwrap();
        assert!(matches!(command, Command::Action { action: Action::Post { task:true, .. }, .. }));
        assert!(rt.state.pending);
        assert!(rt.state.draft.text.contains("Ship preview"));
    }
    #[test] fn native_thread_has_independent_reply_draft_at_narrow_width() {
        let mut app = App::new(); fixture::load(&mut app, false); app.open_thread(1);
        let (tx,rx) = mpsc::sync_channel(4); app.commands = Some(tx); app.draft = edit("keep this post");
        let mut rt = rune::Runtime::new(app,view::root,(700.0,600.0),rune::Env::new(true,1.0));
        for _ in 0..5 { rt.frame(1.0/60.0); }
        assert!(rt.find("thread-panel").is_some());
        assert!(rt.find("sidebar").is_none());
        rt.update(|_,tx| tx.focus("reply-input",false)); rt.frame(1.0/60.0);
        rt.key(rune::KeyEvent { key: Key::Text("thread reply".into()), mods: Default::default() });
        rt.frame(1.0/60.0);
        rt.key(rune::KeyEvent { key: Key::Enter, mods: Default::default() });
        rt.frame(1.0/60.0);
        assert!(matches!(rx.try_recv().unwrap(), Command::Action { action: Action::Reply { thread:1, .. }, .. }));
        assert_eq!(rt.state.draft.text,"keep this post");
    }
    #[test] fn edit_labels_and_schedule_dispatch_preserve_drafts() {
        let mut app = App::new(); fixture::load(&mut app, true);
        let (tx,rx) = mpsc::sync_channel(8); app.commands = Some(tx);
        app.begin_edit(100); app.edit_text = edit("updated reply"); app.save_edit(false);
        assert!(matches!(rx.try_recv().unwrap(), Command::Action { action: Action::Edit { id:100,text }, .. } if text == "updated reply"));
        assert_eq!(app.edit_text.text,"updated reply");
        app.pending = false; app.edit_labels = edit(""); app.save_edit(true);
        assert!(matches!(rx.try_recv().unwrap(), Command::Action { action: Action::SetLabels { id:100,labels }, .. } if labels.is_empty()));
        app.pending = false; app.draft = edit("later"); app.show_schedule = true;
        app.schedule_at = edit("bad"); app.submit(false); assert!(rx.try_recv().is_err()); assert!(app.error.is_some());
        app.schedule_at = edit(&(app.now + 3600).to_string()); app.submit(false);
        assert!(matches!(rx.try_recv().unwrap(), Command::Action { action: Action::Schedule { text,.. },draft:Some((None,_)) } if text == "later"));
        assert_eq!(app.draft.text,"later");
    }
    #[test] fn auxiliary_updates_and_errors_retain_newer_drafts() {
        let mut app = App::new(); fixture::load(&mut app, true);
        app.draft = edit("newer draft"); app.pending = true;
        let (tx,rx) = mpsc::sync_channel(8); app.updates = Some(rx);
        tx.send(Update::Done(Some((None,"old draft".into())),Ok(()))).unwrap();
        tx.send(Update::Extra(Ok((vec![],vec![])))).unwrap();
        tx.send(Update::Attachments(100,Ok(vec![Attachment { id:9,message_id:100,name:"../unsafe.txt".into(),size:2 }]))).unwrap();
        let mut rt = rune::Runtime::new(app,view::root,(1252.0,674.0),rune::Env::new(true,1.0));
        rt.update(|a,tx| { poll(a,0.0,tx); });
        assert_eq!(rt.state.draft.text,"newer draft"); assert!(!rt.state.pending);
        assert!(rt.state.schedules.is_empty()); assert!(rt.state.reactions.is_empty());
        assert_eq!(rt.state.attachments[&100][0].name,"../unsafe.txt");
        tx.send(Update::FileDone(Err("no permission".into()))).unwrap();
        rt.update(|a,tx| { a.pending = true; poll(a,0.0,tx); });
        assert!(!rt.state.pending); assert!(rt.state.error.as_deref().unwrap().contains("no permission"));
    }
    #[test] fn native_edit_input_routes_without_touching_reply() {
        let mut app = App::new(); fixture::load(&mut app, true); app.open_thread(1); app.begin_edit(100);
        app.reply = edit("keep reply");
        let mut rt = rune::Runtime::new(app,view::root,(700.0,600.0),rune::Env::new(true,1.0));
        for _ in 0..5 { rt.frame(1.0/60.0); }
        assert!(rt.find("edit-100").is_some());
        rt.update(|_,tx| tx.focus("edit-text",false)); rt.frame(1.0/60.0);
        rt.key(rune::KeyEvent { key:Key::Text(" amended".into()), mods:Default::default() });
        rt.frame(1.0/60.0);
        assert!(rt.state.edit_text.text.contains("amended")); assert_eq!(rt.state.reply.text,"keep reply");
    }
    #[test] fn scripted_picker_sets_paths_without_uploading_or_downloading() {
        let mut app = App::new(); fixture::load(&mut app,true);
        let (tx,rx) = mpsc::sync_channel(4); app.commands = Some(tx);
        let mut rt = rune::Runtime::new(app,view::root,(1252.0,674.0),rune::Env::new(true,1.0));
        rt.update(|_,tx| tx.open_file(Default::default(),|a: &mut App,r,_| a.picked_path(r,true)));
        rt.frame(1.0/60.0);
        assert_eq!(rt.file_dialog().unwrap().0,rune::FileDialogKind::Open);
        rt.resolve_file_dialog(Some("/tmp/chosen-file.txt".into())).unwrap(); rt.frame(1.0/60.0);
        assert_eq!(rt.state.upload_path.text,"/tmp/chosen-file.txt"); assert!(rx.try_recv().is_err());
        rt.update(|_,tx| tx.save_file(Default::default(),|a: &mut App,r,_| a.picked_path(r,false)));
        rt.frame(1.0/60.0); rt.resolve_file_dialog(Some("/tmp/save-as.txt".into())).unwrap(); rt.frame(1.0/60.0);
        assert_eq!(rt.state.download_path.text,"/tmp/save-as.txt"); assert!(rx.try_recv().is_err());
        rt.update(|a,_| a.picked_path(Ok(None),true));
        assert_eq!(rt.state.upload_path.text,"/tmp/chosen-file.txt");
    }
}