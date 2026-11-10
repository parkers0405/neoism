//! The file browser modal (`shared/src/panels/file_browser.rs`): Change Directory on the splash
//! and Alt+D choose a folder with it, and the composer's paperclip picks a picture. A centred
//! card with a header, a places sidebar (Workspace, Home, Documents, Downloads, Pictures and
//! recent folders), a toolbar (back, forward, up, an editable path and a filter), the folder's
//! entries, and Cancel / Select. The listing is read from this computer's filesystem, as the
//! native host fulfilled `ListDirectory`; recent folders persist in the desktop app's
//! `file-browser-recents.json`.

pub mod view;

use crate::app::App;
use rune::{TextEdit, Tx};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    ChooseDirectory,
    AttachImage,
}

impl Mode {
    pub fn title(self) -> &'static str {
        match self { Mode::ChooseDirectory => "Choose a folder", Mode::AttachImage => "Attach a picture" }
    }
    pub fn action(self) -> &'static str {
        match self { Mode::ChooseDirectory => "Select", Mode::AttachImage => "Attach" }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    pub size: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub label: String,
    pub path: String,
    pub icon: &'static str,
}

pub struct Browser {
    pub mode: Mode,
    pub current: String,
    pub path: TextEdit,
    pub filter: TextEdit,
    pub entries: Vec<Entry>,
    /// The highlighted row among the filtered entries, once the keyboard or pointer chose one.
    pub selected: Option<usize>,
    pub back: Vec<String>,
    pub forward: Vec<String>,
    pub places: Vec<Place>,
    pub recents: Vec<String>,
    pub error: Option<String>,
    /// Bumped on each listing so the rows mount fresh (the original's 0.16 s entrance).
    pub listing: u64,
    pub follow: crate::ui::follow::Follow,
}

const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp"];

pub fn is_image(name: &str) -> bool {
    Path::new(name).extension().and_then(|e| e.to_str()).is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

fn recents_path() -> Option<PathBuf> { dirs::config_dir().map(|d| d.join("neoism").join("file-browser-recents.json")) }

fn load_recents() -> Vec<String> {
    recents_path().and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str::<Vec<String>>(&t).ok())
        .unwrap_or_default().into_iter().filter(|p| Path::new(p).is_dir()).take(8).collect()
}

fn save_recents(recents: &[String]) {
    let Some(path) = recents_path() else { return };
    if let Ok(text) = serde_json::to_string(recents) { let _ = std::fs::write(path, text); }
}

/// `place_paths`: the newest recent folder first, the standard places, then up to four recents.
fn places(workspace: &str, recents: &[String]) -> Vec<Place> {
    let mut out = Vec::new();
    if let Some(first) = recents.first() { out.push(Place { label: "Recent".into(), path: first.clone(), icon: "\u{f02da}" }); }
    let candidates = [
        ("Workspace", Some(PathBuf::from(workspace)), "\u{f024b}"),
        ("Home", dirs::home_dir(), "\u{f02dc}"),
        ("Documents", dirs::document_dir(), "\u{f0219}"),
        ("Downloads", dirs::download_dir(), "\u{f024d}"),
        ("Pictures", dirs::picture_dir(), "\u{f02e9}"),
    ];
    for (label, path, icon) in candidates {
        let Some(path) = path.and_then(|p| p.canonicalize().ok()).filter(|p| p.is_dir()) else { continue };
        let path = path.display().to_string();
        if out.iter().any(|p: &Place| p.path == path && p.label != "Recent") { continue; }
        out.push(Place { label: label.into(), path, icon });
    }
    for recent in recents.iter().take(4) {
        let label = Path::new(recent).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| recent.clone());
        out.push(Place { label, path: recent.clone(), icon: "\u{f02da}" });
    }
    out
}

/// `~/x` for display, as the path field shows it.
pub fn display_path(path: &str) -> String {
    match dirs::home_dir().map(|h| h.display().to_string()) {
        Some(home) if path == home => "~".into(),
        Some(home) if path.starts_with(&format!("{home}/")) => format!("~{}", &path[home.len()..]),
        _ => path.to_owned(),
    }
}

fn expand(input: &str) -> PathBuf {
    let input = input.trim();
    if input == "~" { return dirs::home_dir().unwrap_or_default(); }
    if let Some(rest) = input.strip_prefix("~/") { return dirs::home_dir().unwrap_or_default().join(rest); }
    PathBuf::from(input)
}

impl Browser {
    fn new(mode: Mode, start: &str, workspace: &str) -> Browser {
        let recents = load_recents();
        let mut b = Browser {
            mode, current: String::new(), path: TextEdit::default(), filter: TextEdit::default(), entries: Vec::new(), selected: None,
            back: Vec::new(), forward: Vec::new(), places: places(workspace, &recents), recents, error: None, listing: 0,
            follow: Default::default(),
        };
        b.filter.focused = true;
        b.load(start);
        b
    }

    /// List `path` (canonical): folders first, then files (pictures only when attaching), by name.
    fn load(&mut self, path: &str) {
        self.current = path.to_owned();
        self.path.replace_text(&display_path(path));
        self.filter.replace_text("");
        self.selected = None;
        self.error = None;
        self.listing += 1;
        self.entries.clear();
        match std::fs::read_dir(path) {
            Ok(read) => {
                let mut entries: Vec<Entry> = read.filter_map(Result::ok).filter_map(|e| {
                    let name = e.file_name().to_str()?.to_owned();
                    if name.starts_with('.') { return None; }
                    let meta = std::fs::metadata(e.path()).ok()?;
                    let is_dir = meta.is_dir();
                    let keep = is_dir || (self.mode == Mode::AttachImage && is_image(&name));
                    keep.then(|| Entry { name, is_dir, size: meta.is_file().then(|| meta.len()) })
                }).collect();
                entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
                self.entries = entries;
            }
            Err(error) => self.error = Some(format!("Could not open folder: {error}")),
        }
    }

    pub fn visible(&self) -> Vec<&Entry> {
        let q = self.filter.text.to_lowercase();
        self.entries.iter().filter(|e| q.is_empty() || e.name.to_lowercase().contains(&q)).collect()
    }

    fn selected_entry(&self) -> Option<Entry> { self.selected.and_then(|i| self.visible().get(i).map(|e| (*e).clone())) }

    pub fn navigate(&mut self, input: &str) -> bool {
        let Some(path) = expand(input).canonicalize().ok().filter(|p| p.is_dir()) else {
            self.error = Some(format!("No folder at {}", input.trim()));
            return false;
        };
        let path = path.display().to_string();
        if path != self.current {
            self.back.push(self.current.clone());
            self.forward.clear();
        }
        self.load(&path);
        true
    }

    pub fn go_back(&mut self) {
        let Some(path) = self.back.pop() else { return };
        self.forward.push(self.current.clone());
        self.load(&path);
    }

    pub fn go_forward(&mut self) {
        let Some(path) = self.forward.pop() else { return };
        self.back.push(self.current.clone());
        self.load(&path);
    }

    pub fn go_up(&mut self) {
        if let Some(parent) = Path::new(&self.current).parent().map(|p| p.display().to_string()) { self.navigate(&parent); }
    }

    pub fn can_accept(&self) -> bool {
        self.error.is_none() && match self.mode {
            Mode::ChooseDirectory => true,
            Mode::AttachImage => self.selected_entry().is_some_and(|e| !e.is_dir),
        }
    }

    pub fn move_selection(&mut self, delta: isize) {
        let count = self.visible().len();
        if count == 0 { return; }
        self.selected = Some(match self.selected {
            None => 0,
            Some(i) => (i as isize + delta).clamp(0, count as isize - 1) as usize,
        });
        self.follow.request();
    }

    fn remember(&mut self, path: &str) {
        self.recents.retain(|p| p != path);
        self.recents.insert(0, path.to_owned());
        self.recents.truncate(8);
        save_recents(&self.recents);
    }
}

/// What the browser picked.
pub enum Picked {
    Directory(String),
    Image(String),
}

impl App {
    pub fn open_file_browser(&mut self, mode: Mode) {
        self.picker = None;
        self.finder = None;
        let start = Path::new(&self.cwd).canonicalize().map(|p| p.display().to_string()).unwrap_or_else(|_| self.cwd.clone());
        self.browser = Some(Browser::new(mode, &start, &self.cwd));
        if let Some(wake) = &self.waker { wake(); }
    }

    /// Open the selected row: a folder opens in place; a picture (when attaching) is picked; with
    /// no row chosen, Choose a folder picks the folder being shown.
    pub fn browser_activate(&mut self, index: Option<usize>) {
        let Some(b) = &mut self.browser else { return };
        if index.is_some() { b.selected = index; }
        let entry = b.selected_entry();
        let picked = match entry {
            Some(e) if e.is_dir => {
                let path = Path::new(&b.current).join(&e.name).display().to_string();
                b.back.push(b.current.clone());
                b.forward.clear();
                b.load(&path);
                return;
            }
            Some(e) if b.mode == Mode::AttachImage => Picked::Image(Path::new(&b.current).join(&e.name).display().to_string()),
            None if b.mode == Mode::ChooseDirectory => Picked::Directory(b.current.clone()),
            _ => return,
        };
        self.browser_pick(picked);
    }

    /// The Select / Attach button: the selected folder (or the one shown), or the selected picture.
    pub fn browser_accept(&mut self) {
        let Some(b) = &self.browser else { return };
        if !b.can_accept() { return; }
        let picked = match (b.mode, b.selected_entry()) {
            (Mode::ChooseDirectory, Some(e)) if e.is_dir => Picked::Directory(Path::new(&b.current).join(&e.name).display().to_string()),
            (Mode::ChooseDirectory, _) => Picked::Directory(b.current.clone()),
            (Mode::AttachImage, Some(e)) if !e.is_dir => Picked::Image(Path::new(&b.current).join(&e.name).display().to_string()),
            _ => return,
        };
        self.browser_pick(picked);
    }

    fn browser_pick(&mut self, picked: Picked) {
        let Some(mut b) = self.browser.take() else { return };
        let current = b.current.clone();
        b.remember(&current);
        match picked {
            Picked::Directory(path) => self.change_directory(&path),
            Picked::Image(path) => self.attach_images(vec![path]),
        }
    }

    /// Keys while the browser is open (`handle_key`). Returns true when taken.
    pub fn browser_key(&mut self, event: &rune::KeyEvent, tx: &mut Tx) -> bool {
        use rune::Key;
        let Some(b) = &mut self.browser else { return false };
        let alt = event.mods.alt;
        match event.key {
            Key::Escape => { self.browser = None; }
            Key::Left if alt => b.go_back(),
            Key::Right if alt => b.go_forward(),
            Key::Up if alt => b.go_up(),
            Key::Tab => {
                let path = !b.path.focused;
                b.path.focused = path;
                b.filter.focused = !path;
            }
            Key::Up => { tx.animated(); b.move_selection(-1); }
            Key::Down => { tx.animated(); b.move_selection(1); }
            Key::Enter if b.path.focused => {
                let input = b.path.text.clone();
                if b.navigate(&input) { b.path.focused = false; b.filter.focused = true; }
            }
            Key::Enter => self.browser_activate(None),
            _ => return false,
        }
        true
    }

    /// Reveal the selected row after a keyboard move (from `poll`).
    pub fn browser_follow(&mut self, tx: &mut Tx) {
        let row_h = view::ROW_H * self.ty.scale();
        let Some(b) = &mut self.browser else { return };
        let (Some(selected), count) = (b.selected, b.visible().len()) else { return };
        b.follow.apply(view::LIST_KEY, selected, row_h, count as f32 * row_h, crate::ui::follow::Reveal::Margin(2), tx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_by_extension() {
        assert!(is_image("a.PNG"));
        assert!(is_image("b.jpeg"));
        assert!(!is_image("c.rs"));
    }

    #[test]
    fn lists_folders_first_and_navigates_up() {
        let root = std::env::temp_dir().join(format!("neoism-browser-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("zeta")).unwrap();
        std::fs::create_dir_all(root.join("Alpha")).unwrap();
        std::fs::write(root.join("pic.png"), b"x").unwrap();
        std::fs::write(root.join("notes.txt"), b"x").unwrap();
        let root_s = root.canonicalize().unwrap().display().to_string();
        let mut b = Browser::new(Mode::AttachImage, &root_s, &root_s);
        let names: Vec<_> = b.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Alpha", "zeta", "pic.png"]);
        b.mode = Mode::ChooseDirectory;
        b.load(&root_s);
        assert_eq!(b.entries.len(), 2);
        assert!(b.navigate(&format!("{root_s}/zeta")));
        b.go_up();
        assert_eq!(b.current, root_s);
        b.go_back();
        assert!(b.current.ends_with("zeta"));
        let _ = std::fs::remove_dir_all(&root);
    }
}

impl App {
    /// Attach pictures to the active composer. (Replaced by the composer's own attachments.)
    pub fn attach_images(&mut self, paths: Vec<String>) {
        let _ = paths;
    }
}
