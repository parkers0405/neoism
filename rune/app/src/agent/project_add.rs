//! "Add project": the folder button beside the conversations filter. A centred card with a
//! search row, the sources (a new repository, a folder on disk, a Git URL, a GitHub
//! repository) and a footer of key hints. A source that needs a name or an address asks for it
//! in the same card. Creating and cloning are the daemon's work (`CreateRepository`,
//! `CloneRepository`): the card waits on it and the workspace moves into the new project
//! when it is ready. A daemon too old to know the request does not acknowledge it; the same
//! work then runs as a command in the workspace's own terminal.

use crate::app::App;
use crate::tabs::TabKind;
use crate::ui::theme::{NERD, rgb};
use rune::tok::TextStyle;
use rune::{Color, El, Key, KeyEvent, TextEdit, Tx, col, el, row, text};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    New,
    Folder,
    GitUrl,
    GitHub,
}

const SOURCES: [Source; 4] = [Source::New, Source::Folder, Source::GitUrl, Source::GitHub];

impl Source {
    fn title(self) -> &'static str {
        match self { Source::New => "New project", Source::Folder => "Local folder", Source::GitUrl => "Git URL", Source::GitHub => "GitHub repository" }
    }
    fn detail(self) -> &'static str {
        match self {
            Source::New => "Start a new Git repository from a name",
            Source::Folder => "Browse a folder on disk",
            Source::GitUrl => "Clone from a remote URL",
            Source::GitHub => "Clone GitHub owner/repo",
        }
    }
    /// Octicons repo / link / GitHub mark; Material folder-plus.
    fn icon(self) -> &'static str {
        match self { Source::New => "\u{f401}", Source::Folder => "\u{f0b9d}", Source::GitUrl => "\u{f0337}", Source::GitHub => "\u{f408}" }
    }
    fn placeholder(self) -> &'static str {
        match self { Source::New => "Project name", Source::GitUrl => "https://example.com/owner/repo.git", Source::GitHub => "owner/repo", Source::Folder => "" }
    }
}

pub struct ProjectAdd {
    /// The source being filled in; `None` on the list of sources.
    pub source: Option<Source>,
    pub query: TextEdit,
    pub selected: usize,
    /// Where the project goes, when chosen in the folder browser instead of the default.
    pub location: Option<String>,
    /// What the daemon said went wrong with the last try.
    pub error: Option<String>,
}

/// A create or clone the daemon was asked for.
pub struct ProjectJob {
    id: u64,
    asked: std::time::Instant,
    /// The daemon said it has started: it knows the request.
    acknowledged: bool,
    /// "Cloning into ~/projects/rune…", for the card.
    label: String,
    /// The same work as a shell line, for a daemon that never acknowledges.
    shell: String,
}

/// How long a daemon gets to acknowledge before the terminal does the work instead.
const ACKNOWLEDGE: std::time::Duration = std::time::Duration::from_millis(2500);

/// What Enter will do for the typed text.
struct Plan {
    note: String,
    label: String,
    shell: String,
    message: neoism_protocol::git::GitClientMessage,
}

impl ProjectAdd {
    fn sources(&self) -> Vec<Source> {
        let needle = self.query.text.trim().to_lowercase();
        SOURCES.into_iter().filter(|s| needle.is_empty() || s.title().to_lowercase().contains(&needle) || s.detail().to_lowercase().contains(&needle)).collect()
    }
}

/// A word safe to hand a POSIX shell.
fn quote(word: &str) -> String { format!("'{}'", word.replace('\'', "'\\''")) }

/// The folder a repository address clones into: its last path segment, less `.git`.
fn repository_name(address: &str) -> Option<String> {
    let tail = address.trim().trim_end_matches('/').rsplit(['/', ':']).next()?;
    let name = tail.strip_suffix(".git").unwrap_or(tail);
    valid_name(name).then(|| name.to_owned())
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.starts_with('-')
        && name.chars().all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

impl App {
    /// Where new projects go: the chosen folder, else beside the current project, else
    /// `~/projects` from the home directory. As shown to the reader, as the shell is given
    /// it, and as the daemon is.
    fn projects_directory(&self) -> (String, String, String) {
        let home = crate::startup::home();
        let (home, cwd) = (home.trim_end_matches('/'), self.cwd.trim_end_matches('/'));
        let shown = |path: &str| path.strip_prefix(home).filter(|rest| !home.is_empty() && (rest.is_empty() || rest.starts_with('/'))).map_or_else(|| path.to_owned(), |rest| format!("~{rest}"));
        if let Some(chosen) = self.project_add.as_ref().and_then(|add| add.location.as_deref()) {
            let chosen = chosen.trim_end_matches('/');
            return (shown(chosen), quote(chosen), chosen.to_owned());
        }
        match cwd.rsplit_once('/') {
            Some((parent, _)) if cwd != home && !parent.is_empty() => (shown(parent), quote(parent), parent.to_owned()),
            _ => ("~/projects".into(), "\"$HOME/projects\"".into(), "~/projects".into()),
        }
    }

    /// What Enter will do for the typed text; or why it cannot yet.
    fn project_plan(&self, source: Source, typed: &str) -> Result<Plan, &'static str> {
        use neoism_protocol::git::GitClientMessage as Git;
        let typed = typed.trim();
        let (shown, base, host) = self.projects_directory();
        let clone = |url: String, name: &str, from: String| {
            let path = format!("{base}/{}", quote(name));
            Plan {
                note: format!("Clones {from}into {shown}/{name}."),
                label: format!("Cloning into {shown}/{name}\u{2026}"),
                shell: format!("mkdir -p {base} && git clone {} {path} && cd {path}", quote(&url)),
                message: Git::CloneRepository { url, path: format!("{host}/{name}") },
            }
        };
        match source {
            Source::Folder => Err(""),
            Source::New => {
                if typed.is_empty() { return Err("Type a name for the project."); }
                if !valid_name(typed) { return Err("Use letters, numbers, dashes, dots and underscores."); }
                let path = format!("{base}/{}", quote(typed));
                Ok(Plan {
                    note: format!("Creates {shown}/{typed} and starts a Git repository in it."),
                    label: format!("Creating {shown}/{typed}\u{2026}"),
                    shell: format!("mkdir -p {path} && cd {path} && git init"),
                    message: Git::CreateRepository { path: format!("{host}/{typed}") },
                })
            }
            Source::GitUrl => {
                if typed.is_empty() { return Err("Paste the repository's address."); }
                let Some(name) = repository_name(typed) else { return Err("That does not look like a repository address.") };
                if !(typed.contains("://") || typed.contains('@')) { return Err("Use a full address, like https://… or git@host:owner/repo.git."); }
                Ok(clone(typed.to_owned(), &name, String::new()))
            }
            Source::GitHub => {
                if typed.is_empty() { return Err("Type the repository as owner/repo."); }
                let Some((owner, repo)) = typed.trim_end_matches(".git").split_once('/') else { return Err("Type the repository as owner/repo.") };
                if !valid_name(owner) || !valid_name(repo) { return Err("Type the repository as owner/repo.") }
                Ok(clone(format!("https://github.com/{owner}/{repo}.git"), repo, format!("github.com/{owner}/{repo} ")))
            }
        }
    }

    pub fn open_project_add(&mut self) {
        self.project_menu = None;
        let mut query = TextEdit::new("");
        query.focused = true;
        self.project_add = Some(ProjectAdd { source: None, query, selected: 0, location: None, error: None });
    }

    pub fn close_project_add(&mut self) { self.project_add = None; }

    fn choose_source(&mut self, source: Source) {
        if source == Source::Folder {
            self.project_add = None;
            return self.open_file_browser(crate::browser::Mode::ChooseDirectory);
        }
        if let Some(add) = &mut self.project_add {
            add.source = Some(source);
            add.query = TextEdit::new("");
            add.query.focused = true;
            add.error = None;
        }
    }

    /// "Change" beside the location: pick the folder projects go in. The card waits behind
    /// the folder browser and takes its pick (`project_location_picked`).
    fn choose_project_location(&mut self) {
        if self.project_job.is_some() { return; }
        self.open_file_browser(crate::browser::Mode::ChooseDirectory);
    }

    /// The folder browser's pick while the card is open: where the project goes.
    pub fn project_location_picked(&mut self, path: &str) -> bool {
        let Some(add) = &mut self.project_add else { return false };
        add.location = Some(path.to_owned());
        add.error = None;
        add.query.focused = true;
        true
    }

    /// Ask the daemon; without one, the terminal does it.
    fn start_project(&mut self, plan: Plan) {
        if let Some(add) = &mut self.project_add { add.error = None; }
        let Some(daemon) = &self.daemon else { return self.run_project_command(plan.shell) };
        let id = daemon.request(crate::daemon::Family::Git, serde_json::to_value(&plan.message).expect("Git wire message"), Some(self.cwd.clone()));
        self.project_job = Some(ProjectJob { id, asked: std::time::Instant::now(), acknowledged: false, label: plan.label, shell: plan.shell });
    }

    /// The daemon's word on a create or clone. Returns whether the reply was this job's.
    pub(crate) fn apply_project_reply(&mut self, message: &neoism_protocol::git::GitServerMessage, request_id: u64) -> bool {
        use neoism_protocol::git::GitServerMessage as Reply;
        let Some(job) = self.project_job.as_mut().filter(|job| job.id == request_id) else { return false };
        match message {
            Reply::RepositoryStarted { .. } => job.acknowledged = true,
            Reply::RepositoryReady { path } => {
                self.project_job = None;
                self.project_add = None;
                self.conv_all = false;
                self.change_directory(path);
                self.refresh_threads();
            }
            Reply::Error { message } => {
                self.project_job = None;
                match &mut self.project_add {
                    Some(add) => { add.error = Some(message.clone()); add.query.focused = true; }
                    None => self.modal = Some(crate::ui::modal::Modal::notice("Could not add the project", message)),
                }
            }
            _ => return false,
        }
        true
    }

    /// While a job waits to be acknowledged: how long until it is given to the terminal. A
    /// daemon that predates the request never answers it.
    pub fn project_add_tick(&mut self) -> Option<std::time::Duration> {
        let job = self.project_job.as_ref().filter(|job| !job.acknowledged)?;
        let waited = job.asked.elapsed();
        if waited < ACKNOWLEDGE { return Some(ACKNOWLEDGE - waited); }
        let shell = self.project_job.take().map(|job| job.shell)?;
        self.run_project_command(shell);
        None
    }

    /// Back one step: from a source's field to the list; from the list, closed.
    fn project_add_back(&mut self) {
        match self.project_add.as_mut() {
            Some(add) if add.source.is_some() => {
                add.source = None;
                add.query = TextEdit::new("");
                add.query.focused = true;
            }
            _ => self.project_add = None,
        }
    }

    /// Run the command in the workspace's terminal: the tab whose shell owns the directory,
    /// so its `cd` takes the workspace into the new project.
    fn run_project_command(&mut self, command: String) {
        self.project_add = None;
        let shell = |a: &App, id: u64| a.tabs.iter().position(|t| t.id == id && matches!(t.kind, TabKind::Home | TabKind::Terminal));
        let index = self.main_terminal_tab().and_then(|id| shell(self, id))
            .or_else(|| self.tabs.iter().position(|t| matches!(t.kind, TabKind::Home | TabKind::Terminal)))
            .unwrap_or_else(|| self.open_tab(TabKind::Home));
        let id = self.tabs[index].id;
        self.select_tab(id);
        self.focus = crate::app::Focus::Content;
        // A shell that is busy or still starting keeps the line in its composer to send.
        self.run_command(index, command);
    }

    fn project_add_accept(&mut self) {
        let Some(add) = &self.project_add else { return };
        match add.source {
            None => if let Some(source) = add.sources().get(add.selected).copied() { self.choose_source(source) },
            Some(_) if self.project_job.is_some() => {}
            Some(source) => if let Ok(plan) = self.project_plan(source, &add.query.text) { self.start_project(plan) },
        }
    }

    /// Keys while the card is open; every key stops here.
    pub fn project_add_key(&mut self, event: &KeyEvent) -> bool {
        let Some(add) = self.project_add.as_mut() else { return false };
        let count = add.sources().len().max(1);
        match &event.key {
            Key::Escape => self.project_add = None,
            Key::Enter => self.project_add_accept(),
            Key::Up if add.source.is_none() => add.selected = (add.selected + count - 1) % count,
            Key::Down | Key::Tab if add.source.is_none() => add.selected = (add.selected + 1) % count,
            Key::Backspace if add.query.text.is_empty() => self.project_add_back(),
            _ => return false,
        }
        true
    }
}

fn keycap(app: &App, label: &str) -> El {
    let (p, s) = (app.pal, app.ty.scale());
    row().h(22.0 * s).min_w(22.0 * s).px(7.0 * s).radius(6.0 * s).center().fill(rgb(p.fg).alpha(0.08))
        .child(text(label.to_owned()).font(app.ty.sans(11.5, 500.0)).color(rgb(p.fg)))
}

/// The card over the window.
pub fn view(app: &App) -> El {
    let Some(add) = &app.project_add else { return el() };
    // Behind the folder browser while a location is being chosen.
    if app.browser.is_some() { return el(); }
    let (p, s) = (app.pal, app.ty.scale());
    let width = (560.0 * s).min((app.window[0] - 32.0).max(1.0));
    // The window's own background: black on the dark themes, as the other full cards are.
    let panel = rgb(p.bg);
    let icon = |glyph: &'static str, size: f32, color: u32| text(glyph).font(TextStyle::new(size * s, 400.0).family(NERD)).color(rgb(color));

    let input = rune::text_input(&add.query, app.ty.chat_sized(15.0)).key(format!("project-add-query:{:?}", add.source))
        .caret(rgb(p.fg), crate::ui::cursor::selection(app))
        .placeholder(add.source.map_or("Search...", Source::placeholder), rgb(p.muted))
        .color(rgb(p.fg)).grow(1.0).shrink()
        .on_edit(|a: &mut App| &mut a.project_add.as_mut().expect("add project open").query)
        .on_input_change(|a: &mut App, _: &mut Tx| { if let Some(add) = a.project_add.as_mut() { add.selected = 0; } })
        .on_input_key(|a: &mut App, e: &KeyEvent, _: &mut Tx| a.project_add_key(e));
    let header = row().w_full().h(52.0 * s).px(18.0 * s).gap(12.0 * s).center_y()
        .child(el().w(24.0 * s).h(24.0 * s).radius(6.0 * s).center().hover(rgb(p.fg).alpha(0.08)).cursor(rune::Cursor::Pointer)
            .child(icon("\u{f004d}", 15.0, p.dim))
            .on_press(|a: &mut App| a.project_add_back()))
        .child(input);

    let heading = |label: &str| text(label.to_owned()).font(app.ty.sans(12.0, 500.0)).color(rgb(p.muted)).ml(18.0 * s).mt(6.0 * s).mb(6.0 * s);
    let mut body = col().w_full().pb(10.0 * s);
    let mut hints: Vec<(Vec<&str>, &str)> = Vec::new();
    match add.source {
        None => {
            body = body.child(heading("Sources"));
            let sources = add.sources();
            if sources.is_empty() {
                body = body.child(text("No matching sources").font(app.ty.chat_sized(13.0)).color(rgb(p.muted)).ml(18.0 * s).my(10.0 * s));
            }
            for (index, source) in sources.into_iter().enumerate() {
                let selected = index == add.selected;
                body = body.child(row().key(format!("project-source:{source:?}")).h(50.0 * s).mx(8.0 * s).px(10.0 * s).gap(12.0 * s).center_y().radius(9.0 * s)
                    .fill(if selected { rgb(p.fg).alpha(0.07) } else { Color::TRANSPARENT })
                    .child(el().w(20.0 * s).center().child(icon(source.icon(), 16.0, p.dim)))
                    .child(col().grow(1.0).shrink().min_w(0.0).gap(1.0 * s)
                        .child(text(source.title()).font(app.ty.sans(14.0, 500.0)).color(rgb(p.fg)))
                        .child(text(source.detail()).font(app.ty.chat_sized(12.0)).color(rgb(p.muted)).truncate()))
                    .on_pointer_move(move |a: &mut App, _| { if let Some(add) = a.project_add.as_mut().filter(|add| add.selected != index) { add.selected = index; } })
                    .on_press(move |a: &mut App| a.choose_source(source)));
            }
            hints.extend([(vec!["\u{2191}", "\u{2193}"], "Navigate"), (vec!["Enter"], "Select"), (vec!["Backspace"], "Back"), (vec!["Esc"], "Close")]);
        }
        Some(source) => {
            let outcome = app.project_plan(source, &add.query.text);
            let (note, color) = match (&app.project_job, &add.error, &outcome) {
                (Some(job), ..) => (job.label.clone(), p.fg),
                (None, Some(error), _) => (error.clone(), p.red),
                (None, None, Ok(plan)) => (plan.note.clone(), p.dim),
                (None, None, Err(why)) => ((*why).to_owned(), p.muted),
            };
            let (shown, ..) = app.projects_directory();
            let working = app.project_job.is_some();
            body = body.child(heading(source.title()))
                .child(row().mx(8.0 * s).px(10.0 * s).py(10.0 * s).gap(12.0 * s).center_y()
                    .child(el().w(20.0 * s).center().child(if working { crate::agent::sidebar::loader(app, p.fg) } else { icon(source.icon(), 16.0, p.dim) }))
                    .child(text(note).font(app.ty.sans(13.0, 400.0)).color(rgb(color)).wrap().grow(1.0).shrink()))
                // Where it goes, and the way to put it somewhere else.
                .child(row().mx(8.0 * s).px(10.0 * s).py(8.0 * s).gap(12.0 * s).center_y()
                    .child(el().w(20.0 * s).center().child(icon(crate::ui::theme::glyph::FOLDER_LINE, 15.0, p.muted)))
                    .child(text("Location").font(app.ty.sans(12.5, 400.0)).color(rgb(p.muted)))
                    .child(text(shown).font(app.ty.sans(12.5, 500.0)).color(rgb(p.dim)).truncate().grow(1.0).shrink())
                    .child(row().key("project-add-location").h(24.0 * s).px(10.0 * s).radius(7.0 * s).center()
                        .fill(rgb(p.fg).alpha(0.08)).hover(rgb(p.fg).alpha(0.14)).cursor(rune::Cursor::Pointer)
                        .opacity(if working { 0.4 } else { 1.0 })
                        .child(text("Change").font(app.ty.sans(12.0, 500.0)).color(rgb(p.fg)))
                        .on_press(|a: &mut App| a.choose_project_location())));
            if working { hints.push((vec!["Esc"], "Close (keeps going)")); }
            else { hints.extend([(vec!["Enter"], if source == Source::New { "Create" } else { "Clone" }), (vec!["Backspace"], "Back"), (vec!["Esc"], "Close")]); }
        }
    }
    let mut footer = row().w_full().h(44.0 * s).px(18.0 * s).gap(14.0 * s).center_y().border_sides([1.0, 0.0, 0.0, 0.0], rgb(p.fg).alpha(0.08));
    for (keys, label) in hints {
        let mut hint = row().gap(5.0 * s).center_y();
        for key in keys { hint = hint.child(keycap(app, key)); }
        footer = footer.child(hint.child(text(label).font(app.ty.sans(12.0, 400.0)).color(rgb(p.muted)).ml(2.0 * s)));
    }
    let card = col().key("project-add").w(width).radius(16.0 * s).clip_rounded(16.0 * s).fill(panel)
        .shadow(12.0 * s, 40.0 * s, Color::BLACK.alpha(0.5))
        .on_press(|_: &mut App| {})
        .child(header)
        .child(body)
        .child(footer)
        .child(el().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).radius(16.0 * s).border(1.0, rgb(p.fg).alpha(0.1)));
    col().key("project-add-scrim").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).items_center()
        .pt((app.window[1] * 0.16).max(40.0)).fill(Color::BLACK.alpha(0.35))
        .on_press(|a: &mut App| a.close_project_add())
        .child(card)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wire(message: &neoism_protocol::git::GitClientMessage) -> serde_json::Value { serde_json::to_value(message).unwrap() }

    #[test]
    fn sources_plan_a_daemon_request_and_the_same_work_for_a_shell() {
        use neoism_protocol::git::GitClientMessage as Git;
        let mut app = App::new();
        app.cwd = "/work/it's/app".into();
        let plan = app.project_plan(Source::New, " notes-2 ").unwrap();
        assert_eq!(plan.shell, "mkdir -p '/work/it'\\''s'/'notes-2' && cd '/work/it'\\''s'/'notes-2' && git init");
        assert_eq!(wire(&plan.message), wire(&Git::CreateRepository { path: "/work/it's/notes-2".into() }));
        assert!(plan.note.contains("/work/it's/notes-2"));
        assert!(app.project_plan(Source::New, "a b").is_err());
        assert!(app.project_plan(Source::New, "-rf").is_err());
        let plan = app.project_plan(Source::GitHub, "owner/repo.git").unwrap();
        assert!(plan.shell.contains("git clone 'https://github.com/owner/repo.git' '/work/it'\\''s'/'repo' && cd"));
        assert_eq!(wire(&plan.message), wire(&Git::CloneRepository { url: "https://github.com/owner/repo.git".into(), path: "/work/it's/repo".into() }));
        assert!(app.project_plan(Source::GitHub, "owner").is_err());
        assert!(app.project_plan(Source::GitHub, "owner/$(x)").is_err());
        let plan = app.project_plan(Source::GitUrl, "git@host:team/tool.git").unwrap();
        assert!(plan.shell.contains("git clone 'git@host:team/tool.git' '/work/it'\\''s'/'tool'"));
        assert!(app.project_plan(Source::GitUrl, "tool").is_err());
        assert_eq!(repository_name("https://x.dev/a/b/"), Some("b".into()));
        // A chosen location wins; from the home directory projects go in ~/projects.
        app.open_project_add();
        assert!(app.project_location_picked("/elsewhere/code/"));
        assert_eq!(wire(&app.project_plan(Source::New, "x").unwrap().message), wire(&Git::CreateRepository { path: "/elsewhere/code/x".into() }));
        app.project_add = None;
        app.cwd = crate::startup::home();
        assert_eq!(wire(&app.project_plan(Source::New, "x").unwrap().message), wire(&Git::CreateRepository { path: "~/projects/x".into() }));
    }

    #[test]
    fn the_daemon_reply_opens_the_project_or_shows_why_not() {
        use neoism_protocol::git::GitServerMessage as Reply;
        let mut app = App::new();
        app.cwd = "/work/app".into();
        app.open_project_add();
        app.project_job = Some(ProjectJob { id: 7, asked: std::time::Instant::now(), acknowledged: false, label: String::new(), shell: "true".into() });
        assert!(!app.apply_project_reply(&Reply::RepositoryStarted { path: "/work/x".into() }, 8), "another request's reply");
        assert!(app.project_add_tick().is_some(), "still waiting to be acknowledged");
        assert!(app.apply_project_reply(&Reply::RepositoryStarted { path: "/work/x".into() }, 7));
        assert!(app.project_add_tick().is_none() && app.project_job.is_some(), "acknowledged: the daemon has it");
        assert!(app.apply_project_reply(&Reply::Error { message: "already exists and is not empty.".into() }, 7));
        assert!(app.project_job.is_none());
        assert_eq!(app.project_add.as_ref().unwrap().error.as_deref(), Some("already exists and is not empty."));
        app.project_job = Some(ProjectJob { id: 9, asked: std::time::Instant::now(), acknowledged: true, label: String::new(), shell: "true".into() });
        assert!(app.apply_project_reply(&Reply::RepositoryReady { path: "/work/x".into() }, 9));
        assert!(app.project_add.is_none() && app.project_job.is_none());
        assert_eq!(app.cwd, "/work/x");
    }

    #[test]
    fn the_list_filters_steps_into_a_source_and_backs_out() {
        let mut app = App::new();
        app.open_project_add();
        app.project_add.as_mut().unwrap().query.replace_text("git");
        assert_eq!(app.project_add.as_ref().unwrap().sources(), [Source::New, Source::GitUrl, Source::GitHub]);
        app.project_add.as_mut().unwrap().query.replace_text("");
        app.project_add.as_mut().unwrap().selected = 3;
        app.project_add_accept();
        assert_eq!(app.project_add.as_ref().unwrap().source, Some(Source::GitHub));
        // Enter with nothing valid typed stays put; Backspace on the empty field goes back.
        app.project_add_accept();
        assert!(app.project_add.is_some());
        app.project_add_back();
        assert_eq!(app.project_add.as_ref().unwrap().source, None);
        app.project_add_back();
        assert!(app.project_add.is_none());
    }
}
