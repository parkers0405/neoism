//! Neoism multiplayer identities, without borrowing Neoism's credentials or renderer.
//! The generator is the original shared implementation, not a similar-looking avatar.
#[allow(dead_code)]
#[path = "../../../neoism-frontend/shared/src/editor/crdt/presence_avatar.rs"]
mod plasma;

use std::{cell::RefCell, collections::HashMap, sync::OnceLock, time::{Duration, Instant}};
use rune::{El, Pixels, el};

pub const STEP: Duration = Duration::from_millis(33);

// Compatibility clock for the shared generator's optional clock convenience function.
pub fn rainbow_now_seconds() -> f32 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_secs_f32()
}

fn choose_name<'a>(names: impl IntoIterator<Item = Option<&'a str>>) -> String {
    names.into_iter().flatten().map(str::trim).find(|n| !n.is_empty())
        .map(|n| n.chars().take(32).collect()).unwrap_or_else(|| "neoism".into())
}

/// Same precedence as Rune multiplayer: environment, presence setting, system hostname.
pub fn local_name(fixture: bool) -> String {
    if fixture { return "parker".into(); }
    let env = std::env::var("NEOISM_DISPLAY_NAME").ok();
    let config = config_dir().and_then(|p| std::fs::read_to_string(p.join("config.json")).ok())
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&strip_comments(&s)).ok());
    let configured = config.as_ref().and_then(|c| c["presence"]["display-name"].as_str());
    let host = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .or_else(|_| std::fs::read_to_string("/etc/hostname")).ok()
        .or_else(|| std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).ok())
        .or_else(|| std::process::Command::new("hostname").output().ok().filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned()));
    choose_name([env.as_deref(), configured, host.as_deref()])
}

fn config_dir() -> Option<std::path::PathBuf> {
    if let Some(path) = std::env::var_os("NEOISM_CONFIG_HOME") { return Some(path.into()); }
    let home = dirs::home_dir()?;
    #[cfg(target_os = "windows")]
    { Some(home.join("AppData/Local/neoism")) }
    #[cfg(target_os = "macos")]
    { Some(home.join(".config/neoism")) }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    { Some(std::env::var_os("XDG_CONFIG_HOME").map(std::path::PathBuf::from).unwrap_or_else(|| home.join(".config")).join("neoism")) }
}

// Same string-aware JSONC comment handling as Rune's config reader.
fn strip_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if c == '\\' { if let Some(n) = chars.next() { out.push(n); } } else if c == '"' { in_string = false; }
            continue;
        }
        match (c, chars.peek()) {
            ('"', _) => { in_string = true; out.push(c); }
            ('/', Some('/')) => { for n in chars.by_ref() { if n == '\n' { out.push('\n'); break; } } }
            ('/', Some('*')) => { chars.next(); let mut prev = ' '; for n in chars.by_ref() { if prev == '*' && n == '/' { break; } prev = n; } }
            _ => out.push(c),
        }
    }
    out
}

pub fn validate_posting_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 80 || !name.bytes().all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c)) {
        return Err("Neoboard posting profiles require 1..80 ASCII letters, digits, dash, underscore or dot. Use --profile NAME to override the multiplayer display name.".into());
    }
    Ok(())
}

/// The daemon deliberately stores `credential-actor/profile`. Never rewrite that author
/// or infer ownership from a matching display name. Legacy unprofiled posts stay literal.
pub fn display_name(author: &str) -> &str {
    author.split_once('/').map(|(_, profile)| profile).filter(|n| !n.is_empty()).unwrap_or(author)
}

struct Orb { profile: plasma::AvatarProfile, pixels: Pixels, seen: bool }

fn picture(profile: &plasma::AvatarProfile, side: u32, time: f32) -> Vec<u8> {
    let mut bytes = vec![0; (side * side * 4) as usize];
    profile.cells(0., 0., side as f32, time, |cell| {
        let [x, y, w, h] = cell.rect.map(|v| v as usize);
        let rgba = cell.color.map(|v| (v.clamp(0., 1.) * 255.).round() as u8);
        for row in y..y+h {
            for pixel in bytes[(row * side as usize + x)*4..(row * side as usize + x+w)*4].chunks_exact_mut(4) {
                pixel.copy_from_slice(&rgba);
            }
        }
    });
    bytes
}

#[derive(Default)]
pub struct Profiles {
    orbs: RefCell<HashMap<(String, u32), Orb>>,
    last: RefCell<Option<Instant>>,
}

impl Profiles {
    pub fn begin_view(&self) { for orb in self.orbs.borrow_mut().values_mut() { orb.seen = false; } }
    pub fn end_view(&self) { self.orbs.borrow_mut().retain(|_, orb| orb.seen); }

    pub fn view(&self, name: &str, size: f32, frozen: bool) -> El {
        let side = (size * 2.).round().max(1.) as u32;
        let mut orbs = self.orbs.borrow_mut();
        let orb = orbs.entry((name.into(), side)).or_insert_with(|| {
            let profile = plasma::AvatarProfile::from_seed(name);
            let pixels = Pixels::new();
            pixels.set(side, side, picture(&profile, side, if frozen { 0.6 } else { rainbow_now_seconds() }));
            Orb { profile, pixels, seen: true }
        });
        orb.seen = true;
        el().size(size, size).pixels(&orb.pixels)
    }

    /// Only Pixels change, never App state. `set` wakes Rune, so throttle BEFORE painting:
    /// an immediate pixel wake must not paint again and create a self-sustaining wake loop.
    pub fn tick(&self, now: Instant) -> Duration {
        if let Some(due) = self.last.borrow().map(|at| at + STEP).filter(|due| *due > now) { return due - now; }
        *self.last.borrow_mut() = Some(now);
        let time = rainbow_now_seconds();
        for ((_, side), orb) in self.orbs.borrow().iter() { orb.pixels.set(*side, *side, picture(&orb.profile, *side, time)); }
        STEP
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_are_not_credentials() {
        assert_eq!(choose_name([Some(" "), Some(" workstation "), Some("host")]), "workstation");
        assert_eq!(display_name("admin/workstation"), "workstation");
        assert_eq!(display_name("teammate/workstation"), "workstation");
        assert_eq!(display_name("admin"), "admin");
        assert_eq!(local_name(true), "parker");
        assert!(validate_posting_name("workstation-2").is_ok());
        assert!(validate_posting_name("admin/person").is_err());
        assert!(validate_posting_name("Person Name").is_err());
        let config: serde_json::Value = serde_json::from_str(&strip_comments("{/*comment*/\"presence\":{\"display-name\":\"host\"}}//end")).unwrap();
        assert_eq!(config["presence"]["display-name"], "host");
    }
    #[test]
    fn plasma_is_deterministic_round_and_moves() {
        let p = plasma::AvatarProfile::from_seed("workstation");
        let a = picture(&p, 56, 0.6);
        assert_eq!(a, picture(&plasma::AvatarProfile::from_seed("workstation"), 56, 0.6));
        assert_ne!(a, picture(&p, 56, 1.6));
        assert_eq!(a[3], 0);
        assert_eq!(a[(28*56+28)*4+3], 255);
    }
    #[test]
    fn new_posts_use_profile_without_admin_authority() {
        use neochat::{board::Board, protocol::Action};
        let token = "board-only-credential-with-32-bytes-minimum";
        let admin = "admin-only-credential-with-32-bytes-minimum";
        let board = Board::open(":memory:", token, admin).unwrap();
        let post = || Action::Post { channel: "general".into(), text: "hello".into(), labels: Default::default(), task: false, needs: vec![], assignee: None };
        board.action(token, Some("workstation"), None, post()).unwrap();
        let snapshot = board.snapshot(token).unwrap();
        assert_eq!(snapshot.messages[0].author, "local/workstation");
        assert_eq!(display_name(&snapshot.messages[0].author), "workstation");
        assert!(board.action(admin, Some("workstation"), None, post()).is_err());
    }
    #[test]
    fn ticks_are_throttled_and_unused_orbs_are_dropped() {
        let profiles = Profiles::default();
        let _ = profiles.view("host", 28., true);
        let now = Instant::now();
        assert_eq!(profiles.tick(now), STEP);
        assert_eq!(profiles.tick(now), STEP);
        assert_eq!(profiles.tick(now + Duration::from_millis(10)), STEP - Duration::from_millis(10));
        assert_eq!(*profiles.last.borrow(), Some(now));
        profiles.begin_view(); profiles.end_view();
        assert!(profiles.orbs.borrow().is_empty());
    }
}