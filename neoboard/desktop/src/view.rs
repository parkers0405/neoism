use super::*;
use rune::{Color, Cx, El, ScrollSpec, TextStyle, col, el, row, text, text_input};

fn gray(v: u8) -> Color {
    let s = v as f32 / 255.0;
    let c = if s <= 0.04045 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) };
    Color { r: c, g: c, b: c, a: 1.0 }
}
fn font() -> TextStyle { TextStyle::new(12.0, 400.0) }
fn label(value: impl Into<String>) -> El { text(value.into()).font(font()).color(gray(210)) }
fn muted(value: impl Into<String>) -> El { label(value).color(gray(144)) }
fn line() -> El { el().w_full().h(1.0).fill(gray(35)) }
fn spacer() -> El { el().grow(1.0) }
fn button(value: impl Into<String>, action: impl Fn(&mut App) + 'static) -> El {
    el().px(8.0).h(28.0).center().child(label(value)).on_press(action)
}
fn age(app: &App, m: &Message) -> String {
    let secs = app.now.saturating_sub(m.created_at);
    if secs < 60 { "now".into() } else if secs < 3600 { format!("{}m", secs / 60) }
    else if secs < 86400 { format!("{}h", secs / 3600) } else { format!("{}d", secs / 86400) }
}
fn chip(value: String) -> El {
    el().px(5.0).h(18.0).center_y().border(1.0, gray(38)).fill(gray(0)).child(muted(value).font(TextStyle::new(10.0, 400.0)))
}
fn nav(value: String, count: Option<usize>, active: bool, page: Page) -> El {
    static HOME:rune::Icon=rune::Icon{name:"Home",view:24.,body:r#"<path d="m3 10 9-7 9 7v10h-7v-6h-4v6H3z" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linejoin="round"/>"#};
    static ACTIVITY:rune::Icon=rune::Icon{name:"Activity",view:24.,body:r#"<path d="M2 12h5l3-8 4 16 3-8h5" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/>"#};
    static TASKS:rune::Icon=rune::Icon{name:"Tasks",view:24.,body:r#"<path d="m3 7 2 2 4-5m3 3h9M3 16l2 2 4-5m3 3h9" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/>"#};
    let ink=gray(if active {230}else{140});
    let mut item = row().w_full().h(30.0).px(8.0).items_center().fill(gray(if active { 18 } else { 0 }));
    if let Some(icon)=match &page {Page::Home=>Some(&HOME),Page::Activity=>Some(&ACTIVITY),Page::Tasks=>Some(&TASKS),_=>None} {
        item=item.gap(8.).child(rune::icon(icon,15.).color(ink));
    }
    item=item.child(label(value).color(ink).truncate())
        .on_press(move |a: &mut App| a.navigate(page.clone()));
    if let Some(count) = count.filter(|n| *n > 0) { item = item.child(spacer()).child(muted(count.to_string())); }
    item
}
fn sidebar(app: &App) -> El {
    let mut side = col().key("sidebar").w(199.0).h_full().fill(gray(0));
    side = side.child(row().w_full().h(45.0).px(16.0).items_center()
        .child(label("Neoboard").grow(1.0))
        .child(button("‹", |a| {a.sidebar = false;a.save_preferences();}))).child(line());
    let current=projects::PROJECTS.iter().find(|(_,p)|*p==app.project).map_or(app.project,|(n,_)|*n);
    side=side.child(row().w_full().h(42.).px(8.).items_center().child(row().key("projects-toggle").w_full().h(30.).px(9.).items_center().border(1.,gray(52)).radius(7.).hover(gray(32)).child(label(current)).child(spacer()).child(muted("▾")).on_press(|a:&mut App|a.project_menu=!a.project_menu)));
    let mut channels = col().w_full().padding(8.0)
        .child(nav("Home".into(), None, app.page == Page::Home, Page::Home))
        .child(nav("Activity".into(), None, app.page == Page::Activity, Page::Activity))
        .child(nav("Tasks".into(), Some(app.snapshot.messages.iter().filter(|m| projects::contains(app.project,&m.channel) && m.task.as_ref().is_some_and(|t| matches_filter(&t.state, 0))).count()), app.page == Page::Tasks, Page::Tasks))
        .child(row().h(46.0).px(8.0).items_center().child(muted("channels").grow(1.0)).child(el().key("add-channel").size(28.,28.).center().radius(7.).hover(gray(40)).child(label("+")).on_press_tx(|a:&mut App,tx| {a.error=None;a.new_channel=edit("");a.dialog=Some(Dialog::NewChannel);tx.focus("new-channel",false);} )));
    let mut names = app.snapshot.channels.clone();
    names.sort_by_key(|c| (c.contains('/'), c.clone()));
    for channel in names {
        if !projects::contains(app.project,&channel) {continue;}
        let leaf=channel.split_once('/').map_or(channel.as_str(),|(_,path)|path);
        let unread = app.snapshot.messages.iter().filter(|m| m.channel == channel && m.id > *app.seen.get(&channel).unwrap_or(&0)).count();
        channels = channels.child(nav(format!("#{leaf}"), Some(unread), app.page == Page::Channel(channel.clone()), Page::Channel(channel)));
    }
    side.child(col().grow(1.0).w_full().scroll(ScrollSpec { stick_bottom: false }).child(channels))
        .child(row().key("local-profile").w_full().h(40.).px(16.).gap(8.).items_center()
            .child(app.profiles.view(&app.local_name, 24., app.fixture))
            .child(label(app.me.as_ref().and_then(|m|m.display_name.as_deref()).unwrap_or(&app.local_name)).grow(1.).w(0.).truncate())
            .child(button("⚙", |a| a.dialog = Some(Dialog::Settings)).key("profile-settings").size(30.,30.).center().radius(7.).hover(gray(40))))
}
fn header(app: &App) -> El {
    let title = match &app.page { Page::Home => "Home".into(), Page::Activity => "Activity".into(), Page::Tasks => "Tasks".into(), Page::Scheduled => "scheduled".into(), Page::Channel(c) => format!("#{c}") };
    let mut header = row().key("header").w_full().h(45.0).px(16.0).gap(6.0).items_center();
    if !app.sidebar || (app.width < 900.0 && app.thread.is_some()) {
        header = header.child(button("›", |a| { a.sidebar = true; a.save_preferences(); if a.width < 900.0 { a.thread = None; } }));
    }
    header = header.child(label(title).color(gray(232)));
    if app.page==Page::Home {return header;}
    header = header.child(spacer());
    static FILTER: rune::Icon = rune::Icon{name:"Filters",view:24.,body:r#"<path d="M3 5h18M6 12h12M10 19h4" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"/>"#};
    header.child(el().key("filters").size(32.,32.).center().radius(8.).fill(gray(if !app.query.text.is_empty() || app.page==Page::Tasks && app.task_filters.active(){40}else{0})).hover(gray(40)).child(rune::icon(&FILTER,18.).color(gray(189))).on_press(|a:&mut App|{a.details_picker=None;a.dialog=Some(Dialog::Filters);} ))
}
fn task_status(state:TaskState)->El {
    static DONE:rune::Icon=rune::Icon{name:"Completed",view:24.,body:r#"<g fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round" stroke-linecap="round"><circle cx="12" cy="12" r="9.5"/><path d="m8 12 2.7 2.7 5.3-5.4"/></g>"#};
    if state==TaskState::Completed{return row().items_center().gap(5.).child(rune::icon(&DONE,14.).color(gray(227))).child(label("completed"));}
    let icon=match state {TaskState::Claimed=>"⦿",TaskState::Completed=>"☑",TaskState::Waiting=>"◌",TaskState::Failed=>"⊗",TaskState::Blocked|TaskState::Canceled=>"⊘",TaskState::Ready=>"○"};
    row().items_center().gap(5.).child(label(icon)).child(label(state_name(&state)))
}
fn lease(app:&App,expires:u64)->String {let s=expires.saturating_sub(app.now);format!("{}:{:02} left",s/60,s%60)}
fn thread_options(app:&App,id:u64,row:El)->El {
    let mut slot=el().size(23.,23.).center();
    if app.hover_row==Some(id)||app.row_menu==Some(id){slot=slot.child(el().key(format!("thread-options-{id}")).size(23.,23.).center().radius(5.).fill(gray(27)).hover(gray(45)).child(label("⋯"))
        .on_layout(move|a:&mut App,r|if a.row_menu.is_none(){a.row_anchor=r;}).on_press(move|a:&mut App|a.row_menu=Some(id)));}
    row.child(slot).on_hover_change(move|a:&mut App,inside,_|{if inside{a.hover_row=Some(id);}else if a.hover_row==Some(id){a.hover_row=None;}})
}
fn task_row(app: &App, m: &Message) -> El {
    let task = m.task.as_ref().unwrap();
    let id = m.id;
    let split=app.thread.is_some();let mut title=row().w_full().items_center().gap(6.).clip_soft(0.)
        .child(muted(format!("#{id}"))).child(label(m.text.lines().next().unwrap_or("")).truncate().shrink());
    for(k,v)in m.labels.iter().take(if split{2}else{4}){title=title.child(chip(format!("{k}={v}")));}
    let mut body=col().grow(1.).w(0.).gap(3.).child(title);
    if let Some(actor)=&task.owner {body=body.child(muted(format!("{}{}",app.actor_label(actor),task.expires_at.map_or(String::new(),|t|format!(" · {}",lease(app,t))))).truncate());}
    else if !task.needs.is_empty(){body=body.child(muted(format!("needs {}",task.needs.iter().map(|id|format!("#{id}")).collect::<Vec<_>>().join(", "))).truncate());}
    let mut r=row().key(format!("task-{id}")).w_full().min_h(if app.compact{30.}else{42.}).px(16.).py(5.).gap(7.).items_center().fill(gray(if app.thread==Some(id){18}else{0})).hover(gray(20)).child(task_status(task.state).w(92.)).child(body);
    if !split{r=r.child(muted(format!("#{}",m.channel)).truncate());}
    thread_options(app,id,r.child(muted(age(app,m)).w(24.)).on_press(move|a:&mut App|a.open_thread(id)))
}
fn message(app: &App, m: &Message, in_thread: bool) -> El {
    let id = m.id;
    let mut body = col().grow(1.0).w(0.0).gap(5.0)
        .child(row().gap(8.0).items_center().child(label(app.actor_label(&m.author)))
            .child(muted(if in_thread { String::new() } else { format!("{} #{}", if m.parent.is_some() { "replied in" } else { "in" }, m.channel) }))
            .child(muted(age(app, m)).font(TextStyle::new(10.0, 400.0))))
        .child(label(&m.text).wrap().w_full());
    if let Some(task) = &m.task { body = body.child(label(format!("○ {}", state_name(&task.state)))); }
    if !m.labels.is_empty() { body = body.child(row().gap(5.0).children(m.labels.iter().map(|(k,v)| chip(format!("{k}={v}"))))); }
    let mut counts = BTreeMap::<String, usize>::new();
    for r in app.reactions.iter().filter(|r| r.id == id) { *counts.entry(r.reaction.clone()).or_default() += 1; }
    if !counts.is_empty() {
        body = body.child(row().gap(4.0).children(counts.iter().map(|(r,n)| chip(format!("{r} {n}")))));
    }
    if in_thread {
        body = body.child(row().w_full().scroll_x().gap(4.0)
            .child(button("edit / labels", move |a| a.begin_edit(id)))
            .child(button("attachments", move |a| a.files(id)))
            .child(button("+1", move |a| a.dispatch(Action::React { id, reaction: "+1".into(), remove: false }, None))));
        if app.editing == Some(id) {
            body = body.child(col().key(format!("edit-{id}")).w_full().gap(6.0).padding(6.0).border(1.0, gray(42))
                .child(text_input(&app.edit_text, font()).key("edit-text").w_full().h(76.0).color(gray(220))
                    .on_edit(|a: &mut App| &mut a.edit_text))
                .child(row().child(button("save text", |a| a.save_edit(false)))
                    .child(button("close", |a| a.editing = None)))
                .child(input(app, "edit-labels", "key=value … (empty clears labels)"))
                .child(button("replace labels", |a| a.save_edit(true)))
                .child(input(app, "reaction", "reaction, e.g. +1 or heart"))
                .child(row().child(button("add reaction", move |a| {
                    let reaction = a.reaction.text.trim().to_string();
                    if !reaction.is_empty() { a.dispatch(Action::React { id, reaction, remove: false }, None); }
                })).child(button("remove mine", move |a| {
                    let reaction = a.reaction.text.trim().to_string();
                    if !reaction.is_empty() { a.dispatch(Action::React { id, reaction, remove: true }, None); }
                }))));
        }
    }
    if !in_thread && m.parent.is_none() {
        let (count,last)=app.snapshot.messages.iter().filter(|r|r.parent==Some(id)).fold((0usize,None::<&Message>),|(n,last),r| {
            (n+1,Some(last.filter(|p|(p.created_at,p.id)>(r.created_at,r.id)).unwrap_or(r)))
        });
        if let Some(last)=last {
            body=body.child(row().child(row().key(format!("open-replies-{id}")).h(27.).px(5.).gap(6.).items_center().radius(5.).hover(gray(39))
                .child(label(format!("{count} {}",if count==1{"reply"}else{"replies"})).font(TextStyle::new(12.,600.)))
                .child(muted(format!("· last {}",age(app,last))))
                .on_press(move |a:&mut App|a.open_thread(id))));
        }
    }
    let r=row().key(format!("{}-{id}", if in_thread { "reply" } else { "message" })).w_full().padding(12.0).gap(14.0)
        .child(app.profiles.view(profile::display_name(&m.author), 28., app.fixture))
        .child(body).on_press(move |a: &mut App| a.open_thread(id));
    if !in_thread && m.parent.is_none(){thread_options(app,id,r)}else{r}
}
fn form_input(value:&TextEdit, key:impl Into<String>, hint:&str)->El {
    text_input(value,font()).key(key.into()).w_full().h(36.).px(12.).py(10.).single_line()
        .radius(7.).fill(gray(0)).border(1.,gray(72)).color(gray(235)).placeholder(hint,gray(148))
}
fn input(app: &App, which: &'static str, hint: &'static str) -> El {
    let value = match which {
        "edit-labels" => &app.edit_labels, "reaction" => &app.reaction, "schedule-at" => &app.schedule_at,
        "upload-path" => &app.upload_path, _ => &app.download_path,
    };
    form_input(value,which,hint).on_edit(move |a: &mut App| match which {
            "edit-labels" => &mut a.edit_labels, "reaction" => &mut a.reaction, "schedule-at" => &mut a.schedule_at,
            "upload-path" => &mut a.upload_path, _ => &mut a.download_path,
        })
}
fn attachment_panel(app: &App, id: u64) -> El {
    let mut panel = col().key("attachments").w_full().padding(12.0).gap(6.0).border(1.0, gray(35))
        .child(row().child(label(format!("attachments · #{id}"))).child(spacer())
            .child(button("refresh", move |a| a.files(id)))
            .child(button("×", |a| a.attachment_message = None)))
        .child(input(app, "upload-path", "Upload: full local file path"))
        .child(button(if app.pending { "working…" } else { "upload file (≤8 MiB)" }, move |a| {
            let path = a.upload_path.text.as_str();
            if path.trim().is_empty() { a.error = Some("Enter the local file path".into()); return; }
            a.queue_file(Command::Upload { message: id, path: path.into() }, true);
        }));
    #[cfg(feature = "native-picker")]
    { panel = panel.child(el().key("browse-upload").h(28.0).px(8.0).center_y().child(label("browse local files…"))
        .on_press_tx(|_: &mut App,tx| tx.open_file(rune::FileDialogOptions { title:"Choose attachment to upload".into(),..Default::default() },
            |a: &mut App,result,_| a.picked_path(result,true)))); }
    if let Some(files) = app.attachments.get(&id) {
        if files.is_empty() { panel = panel.child(muted("No attachments")); }
        for file in files {
            let file_id = file.id;
            panel = panel.child(row().w_full().items_center()
                .child(label(format!("{} · {} B", file.name, file.size)).truncate().shrink()).child(spacer())
                .child(button("save as…", move |a| { a.download_id = Some(file_id); a.download_path = edit(""); })));
        }
    } else { panel = panel.child(muted("Refresh to load attachments")); }
    if let Some(file_id) = app.download_id.filter(|selected| app.attachments.get(&id).is_some_and(|fs| fs.iter().any(|f| f.id == *selected))) {
        panel = panel.child(muted(format!("Save attachment #{file_id} · existing files are never overwritten")).wrap())
            .child(input(app, "download-path", "Save as: full destination file path"))
            .child(button("download to path", move |a| {
                let path = a.download_path.text.as_str();
                if path.trim().is_empty() { a.error = Some("Enter a destination file path".into()); return; }
                a.queue_file(Command::Download { id: file_id, path: path.into() }, true);
            }));
        #[cfg(feature = "native-picker")]
        { panel = panel.child(el().key("browse-download").h(28.0).px(8.0).center_y().child(label("choose destination…"))
            .on_press_tx(|_: &mut App,tx| tx.save_file(rune::FileDialogOptions { title:"Save attachment to a new file".into(),file_name:Some("attachment.bin".into()),..Default::default() },
                |a: &mut App,result,_| a.picked_path(result,false)))); }
    }
    panel
}
fn field(app: &App, which: &'static str, hint: &'static str) -> El {
    let value = match which { "labels" => &app.labels, "needs" => &app.needs, "assignee" => &app.assignee, _ => &app.channel };
    form_input(value,format!("composer-{which}"),hint).grow(1.).w(0.)
        .on_edit(move |a: &mut App| match which { "labels" => &mut a.labels, "needs" => &mut a.needs, "assignee" => &mut a.assignee, _ => &mut a.channel })
}
fn composer(app: &App, reply: bool) -> El {
    use rune::{icon, Icon};
    static CLIP: Icon = Icon { name:"Attach file", view:24., body:r#"<path d="m21 11-9.2 9.2a6 6 0 0 1-8.5-8.5l10-10a4 4 0 0 1 5.7 5.7l-10 10a2 2 0 0 1-2.8-2.8l9.2-9.2" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/>"# };
    static TAG: Icon = Icon { name:"Labels", view:24., body:r#"<path d="M3 3h8l10 10-8 8L3 11z" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linejoin="round"/><circle cx="7.5" cy="7.5" r="1.2"/>"# };
    static CLOCK: Icon = Icon { name:"Schedule post", view:24., body:r#"<g fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round"><circle cx="12" cy="12" r="9.5"/><path d="M12 5.7v6.6l4.3 2.5"/></g>"# };
    let tool = |glyph: &'static Icon, active: bool| el().key(glyph.name).size(30.,30.).radius(7.).fill(gray(if active {46} else {0})).hover(gray(44)).center()
        .child(icon(glyph,17.).color(gray(if active {245} else {181})));
    let is_task = !reply && app.page == Page::Tasks;
    let draft = if reply { &app.reply } else { &app.draft };
    let mut card = col().key(if reply { "reply-composer" } else { "composer" }).w_full().padding(9.0).gap(8.0)
        .radius(12.).fill(gray(0)).border(1.0, gray(48));
    card = card.child(text_input(draft, font()).key(if reply { "reply-input" } else { "post-input" }).w_full().h(if reply { 46.0 } else { 54.0 })
        .color(gray(220)).placeholder(if reply { "reply to thread…" } else if is_task { "start a task…" } else { "start a thread…" }, gray(100))
        .on_edit(move |a: &mut App| if reply { &mut a.reply } else { &mut a.draft })
        .on_input_key(move |a: &mut App, e, _| {
            if e.key == Key::Enter && !e.mods.shift { a.submit(reply); true } else { false }
        }));
    let mut tools = row().w_full().items_center().gap(3.0);
    if is_task {
        let dest=app.channel.text.trim().trim_start_matches('#');
        let short=dest.strip_prefix(app.project).and_then(|s|s.strip_prefix('/')).unwrap_or(dest);
        let actor=app.assignee.text.trim();
        tools=tools.child(button(format!("#{short} · {} ▾",if actor.is_empty(){"Unassigned"}else{app.actor_label(actor)}),|a|{a.details_picker=None;a.pick_assignee=false;a.dialog=Some(Dialog::Tags);}).key("task-destination").shrink());
    }
    let owner = if reply { app.thread } else { None };
    let attached = app.compose_file_open && app.compose_file_owner == owner;
    tools=tools.child(tool(&CLIP,attached).on_press_tx(move |_:&mut App,tx| tx.open_file(rune::FileDialogOptions{title:"Attach a photo or file".into(),..Default::default()},move |a:&mut App,result,_| match result {
        Ok(Some(path))=>{ if let Some(path)=path.to_str(){a.compose_file=edit(path);a.compose_file_open=true;a.compose_file_owner=owner;}else{a.error=Some("Filename is not valid UTF-8".into());} },
        Ok(None)=>{}, Err(e)=>a.error=Some(format!("Could not open file chooser: {e}")),
    })));
    let tagged = if reply { owner.and_then(|id|app.reply_labels.get(&id)).is_some_and(|l|!l.text.is_empty()) } else { !app.labels.text.is_empty() };
    tools = tools.child(tool(&TAG,tagged).key(if reply {"reply-tags"} else {"post-tags"}).on_press(move |a:&mut App| {
        a.details_picker=None; a.pick_assignee=false;
        a.dialog=if reply { owner.map(|id|{a.reply_labels.entry(id).or_insert_with(||edit(""));Dialog::ReplyTags(id)}) } else {Some(Dialog::Tags)};
    }));
    if !reply { tools=tools.child(tool(&CLOCK,app.show_schedule).on_press(|a:&mut App| a.dialog=Some(Dialog::Schedule))); }
    if attached {
        let filename=std::path::Path::new(&app.compose_file.text).file_name().unwrap_or_default().to_string_lossy();
        tools=tools.child(el().h(26.).shrink().px(7.).radius(6.).fill(gray(41)).center().child(label(format!("{filename} ×")).truncate()).on_press(|a:&mut App| {a.compose_file_open=false;a.compose_file=edit("");}));
    }
    card = card.child(tools
        .child(spacer())
        .child(el().h(32.0).px(12.0).radius(7.).center().fill(gray(if app.pending { 50 } else { 235 }))
            .child(label("Send").color(gray(8)))
            .on_press(move |a: &mut App| a.submit(reply))));
    col().w_full().padding(16.).child(card)
}

fn dialog(app:&App, page:Dialog)->El {
    use Dialog::*;
    let title=match page {Settings=>"Settings",Personal=>"Personal",BoardView=>"Board View",Connection=>"Connection",Tags=>"Message details",ReplyTags(_)=>"Reply labels",Filters=>"Filters",Schedule=>"Schedule delivery",NewChannel=>"New channel",DeleteThread(_)=>"Delete thread?"};
    let nav=|name:&str, detail:&str, dest:Dialog| row().w_full().h(58.).px(14.).items_center().radius(10.).fill(gray(0)).border(1.,gray(55)).hover(gray(48))
        .child(col().gap(3.).child(label(name)).child(muted(detail).font(TextStyle::new(10.,400.)))).child(spacer()).child(muted("›")).on_press(move |a:&mut App| {if dest==Personal{a.personal_name=edit(a.me.as_ref().and_then(|m|m.display_name.as_deref()).unwrap_or(""));a.error=None;}a.dialog=Some(dest);});
    let mut back=el().w(62.).h(30.).center();
    if matches!(page,BoardView|Connection|Personal){back=back.child(button("‹ Back",|a|a.dialog=Some(Settings)).radius(7.).hover(gray(52)));}
    let top=row().w_full().items_center().child(back).child(spacer()).child(label(title).font(TextStyle::new(16.,600.))).child(spacer())
        .child(el().w(62.).h(30.).center().child(button(if matches!(page,NewChannel|DeleteThread(_)){"Cancel"}else{"Done"},|a|a.dialog=None).radius(7.).fill(gray(0)).border(1.,gray(55)).hover(gray(61))));
    let mut panel=col().key("board-dialog").w(436.).padding(18.).gap(12.).fill(gray(0)).border(1.,gray(60)).radius(16.)
        .child(top).child(line());
    let pref=|name:&str, value:&str, change:fn(&mut App)| row().w_full().h(44.).px(14.).items_center().hover(gray(49))
        .child(label(name)).child(spacer()).child(muted(value)).on_press(move |a:&mut App|{change(a);a.save_preferences();});
    panel=match page {
        DeleteThread(id)=>{let title=app.snapshot.messages.iter().find(|m|m.id==id).and_then(|m|m.text.lines().next()).unwrap_or("Thread");
            let mut p=panel.child(label(format!("#{id} · {title}")).truncate()).child(muted("This will remove the thread, its replies, and its attachments from the board. This cannot be undone.").wrap());
            if let Some(error)=&app.error {p=p.child(muted(error).wrap());}
            p.child(row().child(spacer()).child(button(if app.pending{"Deleting…"}else{"Delete thread"},move|a|a.dispatch(Action::DeleteThread{id},None)).key("confirm-delete").radius(7.).border(1.,gray(75))))},
        ReplyTags(id)=>{
            let empty=edit("");
            panel.child(muted("Labels")).child(form_input(app.reply_labels.get(&id).unwrap_or(&empty),"reply-labels","category=design")
                .on_edit(move |a:&mut App|a.reply_labels.entry(id).or_insert_with(||edit(""))))
                .child(muted("Labels apply only to this reply. Use key=value or key=\"multiple words\".").wrap())
        },
        NewChannel=>{
            let mut p=panel.child(muted("Channel name")).child(form_input(&app.new_channel,"new-channel","e.g. design or team/design").on_edit(|a:&mut App|&mut a.new_channel).on_submit(|a:&mut App,_|a.create_channel()));
            if let Some(error)=&app.error {p=p.child(muted(error).wrap());}
            p.child(row().w_full().child(spacer()).child(button(if app.creating_channel.is_some(){"Creating…"}else{"Create channel"},App::create_channel).key("create-channel").radius(7.).fill(gray(0)).border(1.,gray(55))))
        },
        Settings=>panel.child(nav("Personal","Your display name on this board",Personal)).child(nav("Board View","Task density, sort order and sidebar",BoardView)).child(nav("Connection","Current board and connection status",Connection)),
        Personal=>{let mut p=panel.child(muted("Display name")).child(form_input(&app.personal_name,"personal-name",&app.local_name).on_edit(|a:&mut App|&mut a.personal_name))
            .child(muted(format!("System name: {}. Leave blank to use it again.",app.local_name)).wrap());
            if let Some(e)=&app.error{p=p.child(muted(e).wrap());}
            p.child(row().child(spacer()).child(button("Save name",|a|{a.error=None;a.queue_file(Command::Personal(a.personal_name.text.trim().into()),false);})))},
        BoardView=>panel.child(col().w_full().fill(gray(0)).border(1.,gray(55)).radius(10.)
            .child(pref("Task spacing",if app.compact{"Compact"}else{"Comfortable"},|a|a.compact=!a.compact)).child(line())
            .child(pref("Sort order",if app.oldest{"Oldest first"}else{"Newest first"},|a|a.oldest=!a.oldest)).child(line())
            .child(pref("Sidebar",if app.sidebar{"Visible"}else{"Hidden"},|a|a.sidebar=!a.sidebar)))
            .child(muted("Preferences are saved on this device.").font(TextStyle::new(11.,400.))),
        Connection=>panel.child(muted(OPTIONS.get().and_then(|o|o.endpoint.as_deref()).unwrap_or("Local board")).wrap()).child(label(format!("Status: {}",app.status)).wrap()),
        Tags=>{let current=projects::channel(app.project,app.channel.text.trim().trim_start_matches('#'));
            let selector=|key:&'static str,title:String|button(format!("{title}  ▾"),move|a|{a.pick_assignee=false;a.details_picker=if a.details_picker==Some(key){None}else{Some(key)};}).key(key).border(1.,gray(60)).radius(7.);
            let mut p=panel.child(muted("Channel")).child(selector("pick-channel",format!("#{}",current.split_once('/').map_or(current.as_str(),|(_,s)|s))));
            if app.details_picker==Some("pick-channel") {
                let mut choices=col().w_full().max_h(116.).scroll(ScrollSpec{stick_bottom:false});
                for channel in app.snapshot.channels.iter().filter(|c|projects::contains(app.project,c)) {
                    let name=channel.clone();let short=channel.split_once('/').map_or(channel.as_str(),|(_,s)|s);
                    choices=choices.child(button(format!("#{short}"),move|a|{if a.channel.text!=name{a.assignee=edit("");a.needs=edit("");}a.channel=edit(&name);a.details_picker=None;}));
                }
                if !app.snapshot.channels.iter().any(|c|projects::contains(app.project,c)){choices=choices.child(muted("No channels yet. Use + in the sidebar."));}p=p.child(choices);
            }
            if app.page==Page::Tasks {
                p=p.child(muted("Assignee")).child(button(format!("{}  ▾",if app.assignee.text.is_empty(){"Unassigned"}else{app.actor_label(&app.assignee.text)}),App::show_assignees).key("choose-assignee").border(1.,gray(60)).radius(7.));
                if app.pick_assignee {
                    let mut list=col().w_full().max_h(138.).scroll(ScrollSpec{stick_bottom:false}).child(button("Unassigned",|a|{a.assignee=edit("");a.pick_assignee=false;}));
                    match app.member_list.as_ref().filter(|(ch,_)|*ch==current).map(|(_,r)|r){
                        Some(Ok(members))=>for member in members{let id=member.actor.clone(); let name=app.actor_label(&id).to_owned();list=list.child(button(name,move|a|{a.assignee=edit(&id);a.pick_assignee=false;}));},
                        Some(Err(e))=>list=list.child(muted(e).wrap()),None=>list=list.child(muted("Loading members…")),
                    }p=p.child(list);
                }
                let selected=parse_needs(&app.needs.text).unwrap_or_default();
                p=p.child(muted("Depends on")).child(selector("pick-needs",if selected.is_empty(){"None".into()}else{selected.iter().map(|id|format!("#{id}")).collect::<Vec<_>>().join(", ")}));
                if app.details_picker==Some("pick-needs") {
                    let mut list=col().w_full().h(120.).scroll(ScrollSpec{stick_bottom:false});
                    let mut tasks:Vec<_>=app.snapshot.messages.iter().filter(|m|projects::contains(app.project,&m.channel) && m.task.is_some()).collect();
                    tasks.sort_by_key(|m|(m.channel!=current,&m.channel,m.id));
                    if tasks.is_empty(){list=list.child(muted("No tasks in this workspace yet."));}
                    let mut group="";
                    for m in tasks {let id=m.id;
                        if group!=m.channel {group=&m.channel;let name=group.to_owned();let short=group.split_once('/').map_or(group,|(_,s)|s);
                            list=list.child(if group==current{muted(format!("#{short}")).padding(4.)}else{button(format!("Use #{short} for this draft"),move|a|{a.channel=edit(&name);a.assignee=edit("");a.needs=edit("");})});
                        }
                        if group!=current{list=list.child(muted(format!("#{id}  {}",m.text.lines().next().unwrap_or(""))).truncate());continue;}
                        list=list.child(button(format!("{} #{id}  {}",if selected.contains(&id){"✓"}else{"·"},m.text.lines().next().unwrap_or("")),move|a|{let mut ids=parse_needs(&a.needs.text).unwrap_or_default();if ids.contains(&id){ids.retain(|n|*n!=id);}else{ids.push(id);}a.needs=edit(&ids.iter().map(u64::to_string).collect::<Vec<_>>().join(", "));}));
                    }p=p.child(list);
                }
            }
            p.child(muted("Labels")).child(row().w_full().child(field(app,"labels","category=design")))},
        Filters=>{
            let mut p=panel.child(muted("Search")).child(form_input(&app.query,"filter","Words or labels").on_edit(|a:&mut App|&mut a.query));
            if app.page==Page::Tasks {
                let filters=&app.task_filters;
                let scoped=||app.snapshot.messages.iter().filter(|m|m.task.is_some() && projects::contains(app.project,&m.channel) && filters.channel.as_ref().is_none_or(|c|*c==m.channel));
                for (key,title,value) in [("filter-channel","Channel",filters.channel.clone().unwrap_or_else(||"All channels".into())),("filter-assignee","Assignee",match filters.assignee.as_deref(){None=>"Anyone",Some("")=>"Unassigned",Some("@me")=>"Assigned to me",Some(id)=>app.actor_label(id)}.into()),("filter-thread","Task thread",filters.thread.map_or_else(||"All task threads".into(),|id|format!("#{id}")))] {
                    p=p.child(row().w_full().items_center().child(muted(title)).child(spacer()).child(button(format!("{value}  ▾"),move|a|a.details_picker=if a.details_picker==Some(key){None}else{Some(key)}).key(key).border(1.,gray(52)).radius(7.)));
                    if app.details_picker!=Some(key){continue;}
                    let mut choices=col().w_full().max_h(124.).scroll(ScrollSpec{stick_bottom:false});
                    let options:Vec<(String,Option<String>)>=match key {
                        "filter-channel"=>std::iter::once(("All channels".into(),None)).chain(app.snapshot.channels.iter().filter(|c|projects::contains(app.project,c)).map(|c|(format!("#{c}"),Some(c.clone())))).collect(),
                        "filter-assignee"=>[("Anyone".into(),None),("Assigned to me".into(),Some("@me".into())),("Unassigned".into(),Some(String::new()))].into_iter().chain(scoped().filter_map(|m|m.task.as_ref()?.assignee.as_deref()).collect::<std::collections::BTreeSet<_>>().into_iter().map(|id|(app.actor_label(id).to_string(),Some(id.into())))).collect(),
                        _=>std::iter::once(("All task threads".into(),None)).chain(scoped().map(|m|(format!("#{}  {}",m.id,m.text.lines().next().unwrap_or("")),Some(m.id.to_string())))).collect(),
                    };
                    for (name,id) in options {choices=choices.child(button(name,move|a|{match key {"filter-channel"=>{a.task_filters.channel=id.clone();a.task_filters.thread=None;},"filter-assignee"=>a.task_filters.assignee=id.clone(),_=>a.task_filters.thread=id.as_ref().and_then(|s|s.parse().ok())}a.details_picker=None;}));}p=p.child(choices);
                }
            }
            p.child(muted("Sort order")).child(row().gap(4.).child(button("Oldest first",|a|{a.oldest=true;a.save_preferences();}).fill(gray(if app.oldest{58}else{0})).radius(7.)).child(button("Newest first",|a|{a.oldest=false;a.save_preferences();}).fill(gray(if !app.oldest{58}else{0})).radius(7.)))
             .child(button("Clear filters",|a|{a.query=edit("");a.task_filters=Default::default();a.details_picker=None;if a.page==Page::Tasks{a.filter=0;}}))},
        Schedule=>crate::calendar::picker(app,panel),
    };
    el().absolute(0.,0.).w_full().h_full().center().fill(rune::Color{r:0.,g:0.,b:0.,a:0.64}).child(panel.modal(true,|a:&mut App|a.dialog=None))
}
fn thread(app: &App, root: &Message) -> El {
    let id = root.id;
    let mut content = col().w_full().min_h(app.thread_h).child(spacer()).child(message(app, root, true));
    let mut panel=col().key("thread-panel").w(0.0).grow(0.65).h_full().clip_soft(0.0)
        .child(row().w_full().h(45.).px(18.).gap(10.).items_center().child(label(if root.task.is_some(){"task"}else{"thread"})).child(muted(format!("in #{}",root.channel.strip_prefix(app.project).and_then(|s|s.strip_prefix('/')).unwrap_or(&root.channel))).truncate().shrink()).child(spacer()).child(button("×",|a|a.thread=None))).child(line());
    if let Some(task) = &root.task {
        let field=|name:&str,value:String| row().w_full().items_center().gap(8.).child(muted(name).w(50.)).child(label(value).grow(1.).w(0.).truncate());
        let mut status=row().w_full().gap(6.).items_center().child(task_status(task.state));
        if let Some(t)=task.expires_at {status=status.child(muted(lease(app,t)));}
        let mut details=col().key("task-summary").w_full().padding(16.).gap(7.).child(status);
        if let Some(actor)=task.owner.as_ref().or(task.finished_by.as_ref()){details=details.child(field("owner",app.actor_label(actor).into()));}
        if let Some(actor)=&task.assignee {details=details.child(field("for",app.actor_label(actor).into()));}
        for dep in task.needs.iter().take(4){let status=app.snapshot.messages.iter().find(|m|m.id==*dep).and_then(|m|m.task.as_ref()).map_or("",|t|state_name(&t.state));let dep=*dep;details=details.child(field("needs",format!("#{dep} {status}")).on_press(move|a:&mut App|a.open_thread(dep)));}
        if task.needs.len()>4 {details=details.child(muted(format!("+{} more dependencies",task.needs.len()-4)));}
        if let Some(result)=&task.result{details=details.child(field("result",result.clone()));}
        let me=app.me.as_ref().map(|m|m.actor.as_str());
        let mine=me.is_some() && task.owner.as_deref()==me && task.state==TaskState::Claimed && task.expires_at.is_some_and(|t|t>app.now);
        let creator=me==root.author.split('/').next();
        let active=!matches!(task.state,TaskState::Completed|TaskState::Failed|TaskState::Canceled);
        let claim=task.state==TaskState::Ready && (task.assignee.is_none() || task.assignee.as_deref()==me);
        let primary=|name:&str|el().h(30.).px(12.).center().radius(7.).fill(gray(226)).hover(gray(248)).child(label(name).color(gray(10)));
        let mut actions=row().gap(8.).items_center();
        if claim{actions=actions.child(primary("Claim task").key("claim-task").on_press(move|a:&mut App|a.dispatch(Action::Claim{id,lease_secs:300},None)));}
        if mine {actions=actions.child(primary("Complete task").on_press(move|a:&mut App|{let text=a.reply.text.clone();a.dispatch(Action::Complete{id,text:text.clone()},Some((Some(id),text)));}));}
        if active && (creator||mine){actions=actions.child(button("More actions ▾",move|a|a.task_menu=if a.task_menu==Some(id){None}else{Some(id)}).radius(7.).border(1.,gray(52)));}
        if claim||mine||(active&&creator){details=details.child(actions);}
        if active && app.task_menu==Some(id) && (mine||creator){
            let mut menu=col().w_full().padding(6.).radius(7.).border(1.,gray(52));
            if mine {menu=menu.child(button("Extend claim by 5 minutes",move|a|{a.task_menu=None;a.dispatch(Action::Renew{id,lease_secs:300},None);})).child(button("Mark failed",move|a|{a.task_menu=None;let text=a.reply.text.clone();a.dispatch(Action::Fail{id,text:text.clone()},Some((Some(id),text)));}));}
            if creator {menu=menu.child(button("Cancel task",move|a|{a.task_menu=None;a.dispatch(Action::Cancel{id},None);}));}
            details=details.child(menu);
        }
        panel=panel.child(details).child(line());
    }
    let mut replies: Vec<_> = app.snapshot.messages.iter().filter(|m| m.parent == Some(id)).collect();
    replies.sort_by_key(|m| (m.created_at,m.id));
    content = content.child(line()).child(muted(format!("{} {}", replies.len(),if replies.len()==1{"reply"}else{"replies"})).padding(12.0));
    for reply in replies { content = content.child(message(app, reply, true)); }
    if let Some(message) = app.attachment_message.filter(|selected| *selected == id || app.snapshot.messages.iter().any(|m| m.id == *selected && m.parent == Some(id))) {
        content = content.child(attachment_panel(app, message));
    }
    panel
        .child(col().key(format!("thread-scroll-{id}")).w_full().grow(1.0).h(0.0).scroll(ScrollSpec { stick_bottom: true })
            .on_layout(|a: &mut App, r| a.thread_h = r[3]).child(content))
        .child(composer(app, true))
}

fn setup_drawer(app:&App)->El {
    const SETUP:&str="Help me connect this agent to Neoboard using the neochat-mcp stdio adapter. Ask whether my board runs locally or on another machine and which MCP client I use. For a local board I own, use the running board's explicit --data-dir. For a hosted board, configure NEOCHAT_URL and a channel-scoped NEOCHAT_TOKEN using the client's secret environment. Never ask me to paste a token or invite into chat. Show me the configuration change before applying it. Verify with the channels tool; don't post or claim work yet.";
    let card=|index:&str,title:&str,description:&str| col().w_full().padding(16.).gap(10.).radius(12.).fill(gray(0)).border(1.,gray(47))
        .child(row().gap(10.).items_center().child(chip(index.into())).child(label(title).font(TextStyle::new(15.,600.))))
        .child(muted(description).w_full().wrap());
    let copy=|key:&'static str,title:&'static str,contents:String|row().child(el().key(key).px(12.).h(30.).radius(8.).fill(gray(217)).hover(gray(240)).center()
        .child(label(if app.home_copied==Some(key){"Copied!"}else{title}).color(gray(18)))
        .on_press_tx(move |a:&mut App,tx|{tx.copy(contents.to_owned());a.home_copied=Some(key);}));
    let p=app.project;
    let cards=col().w_full().padding(14.).gap(10.)
        .child(card("01","Connect an agent","Paste this prompt into your MCP client. Keep credentials out of chat.").child(copy("copy-setup","Copy setup prompt",format!("{SETUP} My project is {p}."))))
        .child(card("02","Invite a teammate","On the host, replace the example endpoint and guest name, then send the invite file privately.").child(copy("copy-invite","Copy project invite",format!("neochat-share invite --endpoint https://board.example --project {p} maya maya-{p}.neochat"))))
        .child(card("03","Join this board","Connect to the host’s private network. On your machine, open the file you received.").child(copy("copy-join","Copy join command",format!("neochat-share open maya-{p}.neochat"))));
    let panel=col().key("setup-drawer").w(app.width.min(372.)).h_full().fill(gray(0)).border(1.,gray(48))
        .child(row().w_full().h(56.).px(16.).items_center().gap(8.).child(label("Setup").font(TextStyle::new(17.,600.))).child(chip(p.into())).child(spacer()).child(button("Close",|a|a.setup_open=false)))
        .child(line()).child(col().w_full().grow(1.).h(0.).scroll(ScrollSpec{stick_bottom:false}).child(cards));
    el().absolute(0.,0.).w_full().h_full().fill(rune::Color{r:0.,g:0.,b:0.,a:0.64}).child(panel.pin(None,Some(0.),Some(0.),Some(0.)).modal(true,|a:&mut App|a.setup_open=false))
}
fn home(app:&App)->El {
    let width=app.width-if app.sidebar{200.}else{0.};
    let narrow=width<540.;
    let module=|key:&str| col().key(key).padding(16.).gap(9.).radius(12.).border(1.,gray(52)).hover(gray(18));
    let top=|tile:El| if narrow {tile.w_full().min_h(130.)}else{tile.w(0.).grow(1.).min_h(164.)};
    let setup=module("home-setup").child(muted("01 / GET STARTED")).child(label("Setup").font(TextStyle::new(18.,600.)))
        .child(muted("Connect an agent, invite a teammate, or join a board.").wrap()).child(spacer()).child(label("Open guide  ›"))
        .on_press(|a:&mut App|{a.setup_open=true;a.home_copied=None;});
    let preview=|tasks:bool| {
        let mut items=Vec::with_capacity(4);
        for m in app.snapshot.messages.iter().filter(|m|projects::contains(app.project,&m.channel)) {
            if tasks && !m.task.as_ref().is_some_and(|t|matches_filter(&t.state,0)){continue;}
            let rank=(tasks && m.task.as_ref().is_some_and(|t|t.state==TaskState::Claimed),m.created_at,m.id);
            let pos=items.partition_point(|(other,_)|*other>rank);
            if pos<3 {items.insert(pos,(rank,m));items.truncate(3);}
        }
        let mut panel=module(if tasks{"home-tasks"}else{"home-activity"}).child(muted(if tasks{"02 / WORK"}else{"03 / FEED"}))
            .child(row().w_full().items_center().child(label(if tasks{"Tasks"}else{"Activity"}).font(TextStyle::new(18.,600.))).child(spacer()).child(muted("View all ›"))).child(line());
        if items.is_empty(){panel=panel.child(muted(if tasks{"No open tasks in this project."}else{"No activity in this project yet."}).wrap());}
        for (_,m) in items {let prefix=if tasks{format!("{:?}",m.task.as_ref().unwrap().state)}else{app.actor_label(&m.author).to_string()};panel=panel.child(row().w_full().gap(6.).child(muted(prefix)).child(label(m.text.lines().next().unwrap_or("")).grow(1.).w(0.).truncate()));}
        panel.on_press(move|a:&mut App|{a.query=edit("");a.filter=0;a.navigate(if tasks{Page::Tasks}else{Page::Activity});})
    };
    let layout=if narrow {col()}else{row()};
    let cards=col().w_full().gap(12.).child(crate::splash::view(app)).child(layout.w_full().gap(12.).child(top(setup)).child(top(preview(true)))).child(preview(false).w_full());
    col().w_full().grow(1.).h(0.).scroll(ScrollSpec{stick_bottom:false}).child(col().w_full().padding(20.).items_center().child(cards))
}

pub fn root(app: &App, cx: &mut Cx<App>) -> El {
    app.profiles.begin_view();
    let mut main = col().key("main").w(0.0).grow(1.0).h_full().clip_soft(0.0).child(header(app)).child(line());
    if app.page == Page::Tasks {
        main = main.child(row().w_full().h(34.0).px(20.0).gap(18.0).scroll_x()
            .children(FILTERS.iter().enumerate().map(|(i, f)| {
                let active = i == app.filter;
                col().h(34.0).child(el().h(33.0).center_y()
                    .child(label(*f).color(gray(if active { 232 } else { 153 }))))
                    .child(el().h(1.0).w_full().fill(gray(if active { 232 } else { 35 })))
                    .on_press(move |a: &mut App| a.filter = i)
            }))).child(line());
    }
    if app.page==Page::Home {main=main.child(home(app));} else {
    let messages = app.visible();
    let mut list = col().w_full();
    if matches!(app.page,Page::Activity | Page::Channel(_)) && app.oldest { list = list.min_h(app.feed_h).child(spacer()); }
    if app.page == Page::Scheduled {
        list = list.child(muted("Your pending scheduled posts · UTC (not local time)").wrap().padding(20.0));
        let query = app.query.text.to_lowercase();
        let mut scheduled: Vec<_> = app.schedules.iter().filter(|s| projects::contains(app.project,&s.channel) && (query.is_empty() || format!("{} {} {} {} {:?}",s.id,s.channel,s.author,s.text,s.labels).to_lowercase().contains(&query))).collect();
        scheduled.sort_by_key(|s| (s.publish_at,s.id));
        if !app.oldest { scheduled.reverse(); }
        if scheduled.is_empty() { list = list.child(muted("No pending scheduled posts match this view").padding(20.0)); }
        for post in scheduled {
            let id = post.id;
            list = list.child(col().w_full().padding(16.0).gap(6.0)
                .child(muted(format!("#{id} · #{} · {} UTC · in {}m", post.channel, format_utc(post.publish_at), post.publish_at.saturating_sub(app.now)/60)).wrap())
                .child(label(&post.text).wrap().w_full())
                .child(row().w_full().scroll_x().gap(5.0).children(post.labels.iter().map(|(k,v)| chip(format!("{k}={v}")))))
                .child(button("cancel scheduled post", move |a| a.dispatch(Action::CancelSchedule { id }, None)))
                .child(line()));
        }
    } else if messages.is_empty() {
        list = list.child(muted(if app.status == "connecting" { "connecting to board…" } else { "No messages match this view." }).padding(20.0));
    } else {
        for m in messages { list = list.child(if app.page == Page::Tasks { task_row(app,m) } else { message(app,m,false) }); }
    }
    main = main.child(col().key(format!("feed-{:?}-{}", match &app.page { Page::Home => "home", Page::Activity => "activity", Page::Tasks => "tasks", Page::Scheduled => "scheduled", Page::Channel(c) => c }, app.filter))
        .w_full().grow(1.0).h(0.0).scroll(ScrollSpec { stick_bottom: matches!(app.page,Page::Activity | Page::Channel(_)) && app.oldest })
        .on_layout(|a: &mut App, r| a.feed_h = r[3]).child(list));
    if let Some(error) = &app.error { main = main.child(label(error).color(rune::oklch(0.75,0.1,20.0)).wrap().padding(12.0)); }
    if let Some(notice) = &app.notice { main = main.child(muted(notice).wrap().px(20.0)); }
    if app.page!=Page::Activity {main = main.child(composer(app, false));}
    }
    let mut root = row().key("neochat").w_full().h_full().fill(gray(0)).on_layout(|a: &mut App, r| a.width = r[2]);
    if app.sidebar && (app.width >= 900.0 || app.thread.is_none()) { root = root.child(sidebar(app)).child(el().w(1.0).h_full().fill(gray(35))); }
    root = root.child(main);
    if let Some(thread_root) = app.thread.and_then(|id| app.snapshot.messages.iter().find(|m| m.id == id)) {
        root = root.child(el().w(1.0).h_full().fill(gray(35))).child(thread(app,thread_root));
    }
    if app.project_menu {
        let mut menu=col().key("projects-menu").w(184.).padding(6.).gap(2.).fill(gray(0)).border(1.,gray(62)).radius(8.);
        for &(name,id) in projects::PROJECTS {
            if OPTIONS.get().and_then(|o|o.project).is_some_and(|locked|id!=locked){continue;}
            menu=menu.child(row().key(format!("project-{id}")).w_full().h(32.).px(8.).items_center().radius(5.).hover(gray(36)).child(label(name)).child(spacer()).child(muted(if id==app.project{"✓"}else{""})).on_press(move|a:&mut App|a.select_project(id)));
        }
        root=root.child(menu.absolute(8.,85.).modal(true,|a:&mut App|a.project_menu=false));
    }
    if let Some(id)=app.row_menu && let Some(m)=app.snapshot.messages.iter().find(|m|m.id==id && m.parent.is_none()){
        let mut menu=col().key("thread-context").w(175.).padding(5.).gap(2.).fill(gray(0)).border(1.,gray(57)).radius(7.)
            .child(button("Open thread",move|a|{a.row_menu=None;a.open_thread(id);})).child(el().h(28.).px(8.).center_y().hover(gray(35)).child(label("Copy thread ID")).on_press_tx(move|a:&mut App,tx|{tx.copy(id.to_string());a.row_menu=None;}));
        if app.me.as_ref().map(|m|m.actor.as_str())==m.author.split('/').next(){menu=menu.child(line()).child(button("Delete thread…",move|a|{a.row_menu=None;a.error=None;a.dialog=Some(Dialog::DeleteThread(id));}).key("delete-option"));}
        let [x,y,w,h]=app.row_anchor;
        root=root.child(menu.absolute((x+w-175.).max(4.),if y>app.feed_h.max(180.){(y-99.).max(4.)}else{y+h}).modal(true,|a:&mut App|a.row_menu=None));
    }
    if let Some(page)=app.dialog {root=root.child(dialog(app,page));}
    if app.setup_open {root=root.child(setup_drawer(app));}
    app.profiles.end_view();
    zoom::wrap(app, cx, root)
}