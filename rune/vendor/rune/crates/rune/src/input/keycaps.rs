//! `--keys` and `--pointer`: screencast-style keycaps and click ripples drawn over headless
//! frames. A separate runtime draws them, so they never enter the app's tree.

use crate::edit::{Key, KeyEvent, Mods};
use crate::el::{El, col, el, text};
use crate::runtime::{Cx, Env, Runtime};
use crate::{Color, Transition, oklcha};

/// Typing within this many seconds of the last key joins its cap.
const BURST: f64 = 1.0;
/// A cap stays this long after its last key, then fades out.
const HOLD: f64 = 1.5;
/// Caps on screen at once; older ones leave early.
const SHOWN: usize = 3;
/// Typed characters a cap shows before eliding the oldest.
const TYPED: usize = 24;
/// A ripple holds this long before it expands and fades.
const RIPPLE: f64 = 0.12;
const MARGIN: f32 = 24.0;

/// Where the caps sit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Corner {
    #[default]
    BottomLeft,
    Bottom,
    BottomRight,
    TopLeft,
    Top,
    TopRight,
}

impl Corner {
    pub(crate) fn parse(s: &str) -> Option<Corner> {
        Some(match s {
            "bottom-left" => Corner::BottomLeft,
            "bottom" => Corner::Bottom,
            "bottom-right" => Corner::BottomRight,
            "top-left" => Corner::TopLeft,
            "top" => Corner::Top,
            "top-right" => Corner::TopRight,
            _ => return None,
        })
    }
}

/// One keycap: a burst of typing, or a chord with its repeat count.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Cap {
    pub id: u64,
    text: String,
    typing: bool,
    count: u32,
    last: f64,
}

impl Cap {
    pub(crate) fn label(&self) -> String {
        if self.typing {
            if self.text.trim().is_empty() { return "␣".repeat(self.text.chars().count().min(TYPED)); }
            let n = self.text.chars().count();
            if n <= TYPED { return self.text.clone(); }
            return format!("…{}", self.text.chars().skip(n - (TYPED - 1)).collect::<String>());
        }
        if self.count > 1 { format!("{} ×{}", self.text, self.count) } else { self.text.clone() }
    }
}

/// Groups key presses into caps.
#[derive(Default)]
pub(crate) struct Keycaps {
    caps: Vec<Cap>,
    next: u64,
}

impl Keycaps {
    /// A logical key from the script.
    pub(crate) fn key(&mut self, t: f64, ev: &KeyEvent) {
        if matches!(ev.key, Key::Modifier { pressed: false, .. }) { return; }
        let (label, typing) = key_label(ev);
        self.push(t, label, typing);
    }

    /// A chord (a global shortcut or a physical key press).
    pub(crate) fn chord(&mut self, t: f64, shortcut: &crate::Shortcut) {
        let name = shortcut.to_string();
        let key = name.rsplit('+').next().unwrap_or_default();
        self.push(t, format!("{}{}", mod_glyphs(shortcut.modifiers()), named_glyph(key)), false);
    }

    fn push(&mut self, t: f64, label: String, typing: bool) {
        if let Some(last) = self.caps.last_mut()
            && t - last.last <= BURST
            && last.typing == typing
            && (typing || last.text == label)
        {
            if typing { last.text.push_str(&label) } else { last.count += 1 }
            last.last = t;
            return;
        }
        self.caps.push(Cap { id: self.next, text: label, typing, count: 1, last: t });
        self.next += 1;
    }

    /// The caps on screen at `t`, oldest first; forgets those that have left.
    pub(crate) fn visible(&mut self, t: f64) -> Vec<Cap> {
        self.caps.retain(|c| t - c.last < HOLD);
        let skip = self.caps.len().saturating_sub(SHOWN);
        self.caps[skip..].to_vec()
    }
}

/// Apple's order: Control, Option, Shift, Command. `cmd` is Ctrl off macOS.
fn mod_glyphs(m: Mods) -> String {
    let mac = cfg!(target_os = "macos");
    let held = [(m.ctrl || (!mac && m.cmd), "⌃"), (m.alt, "⌥"), (m.shift, "⇧"), (m.logo || (mac && m.cmd), "⌘")];
    held.iter().filter(|(on, _)| *on).map(|(_, g)| *g).collect()
}

/// The cap text for a key, and whether it is plain typing (no ⌘, ⌃ or ⌥).
fn key_label(ev: &KeyEvent) -> (String, bool) {
    let m = ev.mods;
    let chord = m.cmd || m.ctrl || m.alt || m.logo;
    let key = match &ev.key {
        Key::Text(s) if !chord => return (s.clone(), true),
        Key::Text(s) if s == " " => "␣".to_owned(),
        Key::Text(s) => title(s),
        Key::Command(c) => {
            let mods = Mods { cmd: true, ..m };
            let label = if *c == ' ' { "␣".to_owned() } else { title(&c.to_string()) };
            return (format!("{}{label}", mod_glyphs(mods)), false);
        }
        Key::Enter => "↩".into(),
        Key::Backspace => "⌫".into(),
        Key::Delete => "⌦".into(),
        Key::Left => "←".into(),
        Key::Right => "→".into(),
        Key::Up => "↑".into(),
        Key::Down => "↓".into(),
        Key::Home => "↖".into(),
        Key::End => "↘".into(),
        Key::PageUp => "⇞".into(),
        Key::PageDown => "⇟".into(),
        Key::Insert => "Ins".into(),
        Key::PrintScreen => "PrtSc".into(),
        Key::ScrollLock => "ScrLk".into(),
        Key::Pause => "Pause".into(),
        Key::Escape => "⎋".into(),
        Key::Tab => "⇥".into(),
        Key::Function(n) => format!("F{n}"),
        Key::ContextMenu => "≣".into(),
        // Its own flag is already in `mods`.
        Key::Modifier { modifier: crate::Modifier::Fn, .. } => "fn".into(),
        Key::Modifier { .. } => return (mod_glyphs(m), false),
    };
    (format!("{}{key}", mod_glyphs(m)), false)
}

/// A physical key's stable name (`Shortcut`'s) as a cap.
fn named_glyph(name: &str) -> String {
    match name {
        "space" => "␣", "enter" => "↩", "escape" => "⎋", "tab" => "⇥", "backspace" => "⌫", "delete" => "⌦",
        "left" => "←", "right" => "→", "up" => "↑", "down" => "↓", "home" => "↖", "end" => "↘",
        "pageup" => "⇞", "pagedown" => "⇟", "insert" => "Ins", "minus" => "-", "equal" => "=", "comma" => ",",
        "period" => ".", "slash" => "/", "semicolon" => ";", "quote" => "'", "bracketleft" => "[",
        "bracketright" => "]", "backslash" => "\\", "backquote" => "`",
        other => return title(other),
    }.to_owned()
}

/// As on a keycap: "k" draws as "K" and "capslock" as "Capslock". A capital that doesn't
/// lower-case back to the same letter (Turkish "ı" → "I") stays as typed.
fn title(name: &str) -> String {
    let mut chars = name.chars();
    let Some(first) = chars.next() else { return String::new() };
    let upper: String = first.to_uppercase().collect();
    let first = if upper.to_lowercase() == first.to_string() { upper } else { first.to_string() };
    format!("{first}{}", chars.as_str().to_lowercase())
}

#[derive(Clone, Default, PartialEq)]
pub(crate) struct Scene {
    caps: Vec<(u64, String)>,
    ripples: Vec<(u64, f32, f32)>,
    corner: Corner,
    dark: bool,
}

/// `--keys-theme`: dark or light caps, or (None) whichever matches the app's theme.
pub(crate) fn parse_theme(s: &str) -> Option<Option<bool>> {
    Some(match s {
        "auto" => None,
        "light" => Some(false),
        "dark" => Some(true),
        _ => return None,
    })
}

/// The overlay's own runtime: caps and ripples as keyed elements whose enter and exit run on
/// the motion system's springs.
pub(crate) struct Overlay {
    rt: Runtime<Scene>,
    keys: Option<Keycaps>,
    ripples: Option<(Vec<(u64, f32, f32, f64)>, u64)>,
    theme: Option<bool>,
}

impl Overlay {
    pub(crate) fn new(size: (f32, f32), scale: f32, keys: Option<Corner>, theme: Option<bool>, pointer: bool) -> Overlay {
        let scene = Scene { corner: keys.unwrap_or_default(), ..Scene::default() };
        let rt = Runtime::new(scene, view, size, Env::new(true, scale));
        Overlay { rt, keys: keys.map(|_| Keycaps::default()), ripples: pointer.then(|| (Vec::new(), 0)), theme }
    }

    pub(crate) fn set_srgb_blend(&mut self, enabled: bool) {
        self.rt.set_srgb_blend(enabled);
    }

    pub(crate) fn key(&mut self, t: f64, ev: &KeyEvent) {
        if let Some(k) = &mut self.keys { k.key(t, ev) }
    }

    pub(crate) fn chord(&mut self, t: f64, shortcut: &crate::Shortcut) {
        if let Some(k) = &mut self.keys { k.chord(t, shortcut) }
    }

    pub(crate) fn press(&mut self, t: f64, x: f32, y: f32) {
        if let Some((ripples, next)) = &mut self.ripples {
            ripples.push((*next, x, y, t));
            *next += 1;
        }
    }

    /// Advances to `t` (`dt` after the last frame) over an app that is `app_dark`, and returns
    /// the instances to draw.
    pub(crate) fn frame(&mut self, t: f64, dt: f64, app_dark: bool) -> &mut Runtime<Scene> {
        let dark = self.theme.unwrap_or(app_dark);
        let caps = self.keys.as_mut().map(|k| k.visible(t).iter().map(|c| (c.id, c.label())).collect()).unwrap_or_default();
        let ripples = match &mut self.ripples {
            Some((r, _)) => { r.retain(|&(.., t0)| t - t0 < RIPPLE); r.iter().map(|&(id, x, y, _)| (id, x, y)).collect() }
            None => Vec::new(),
        };
        let s = &self.rt.state;
        if s.caps != caps || s.ripples != ripples || s.dark != dark {
            self.rt.update(move |s, _| { s.caps = caps; s.ripples = ripples; s.dark = dark; });
        }
        self.rt.frame(dt);
        &mut self.rt
    }
}

fn view(s: &Scene, _: &mut Cx<Scene>) -> El {
    let caps = col().gap(8.0).children(s.caps.iter().map(|(id, label)| cap(*id, label, s.dark)));
    let m = Some(MARGIN);
    let caps = match s.corner {
        Corner::BottomLeft => caps.items_start().pin(m, None, None, m),
        Corner::Bottom => caps.items_center().pin(Some(0.0), None, Some(0.0), m),
        Corner::BottomRight => caps.items_end().pin(None, None, m, m),
        Corner::TopLeft => caps.items_start().pin(m, m, None, None),
        Corner::Top => caps.items_center().pin(Some(0.0), m, Some(0.0), None),
        Corner::TopRight => caps.items_end().pin(None, m, m, None),
    };
    el().w_full().h_full().child(caps).children(s.ripples.iter().map(|&(id, x, y)| ripple(id, x, y)))
}

fn cap(id: u64, label: &str, dark: bool) -> El {
    // Blending is linear, so a dark cap needs a high alpha to read as dark over light content.
    let (fill, border, ink, shadow) = if dark {
        (oklcha(0.22, 0.0, 0.0, 0.96), oklcha(1.0, 0.0, 0.0, 0.16), Color::WHITE, 0.4)
    } else {
        (oklcha(0.99, 0.0, 0.0, 0.96), oklcha(0.0, 0.0, 0.0, 0.12), oklcha(0.2, 0.0, 0.0, 1.0), 0.18)
    };
    el().key(format!("cap-{id}")).px(12.0).py(6.0).radius(9.0)
        .fill(fill)
        .border(1.0, border)
        .shadow(4.0, 14.0, oklcha(0.0, 0.0, 0.0, shadow))
        .child(text(label).font(crate::TextStyle::new(18.0, 600.0)).color(ink))
        // Leaving caps keep their slot as it collapses, so the rest slide instead of jumping.
        .transition(Transition::asymmetric(Transition::scale(0.85), Transition::opacity()).hold_layout())
}

fn ripple(id: u64, x: f32, y: f32) -> El {
    const R: f32 = 14.0;
    el().key(format!("ripple-{id}")).absolute(x - R, y - R).size(2.0 * R, 2.0 * R).radius(R)
        // Mid grey with a white rim and a dark halo reads over light and dark apps alike.
        .fill(oklcha(0.5, 0.0, 0.0, 0.3))
        .border(2.0, oklcha(1.0, 0.0, 0.0, 0.95))
        .shadow(0.0, 6.0, oklcha(0.0, 0.0, 0.0, 0.55))
        .transition(Transition::asymmetric(Transition::scale(0.4), Transition::scale(2.2)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(c: &str) -> KeyEvent { KeyEvent { key: Key::Text(c.into()), mods: Mods::default() } }

    fn labels(k: &mut Keycaps, t: f64) -> Vec<String> { k.visible(t).iter().map(Cap::label).collect() }

    #[test]
    fn bursts_of_typing_share_one_cap_and_chords_get_their_own() {
        let mut k = Keycaps::default();
        for (i, c) in "Hi there".chars().enumerate() { k.key(0.1 * i as f64, &typed(&c.to_string())); }
        assert_eq!(labels(&mut k, 0.8), ["Hi there"]);
        let cmd_k = KeyEvent { key: Key::Command('k'), mods: Mods { cmd: true, ..Mods::default() } };
        k.key(0.9, &cmd_k);
        k.key(1.0, &typed("x"));
        let cmd_k = if cfg!(target_os = "macos") { "⌘K" } else { "⌃K" };
        assert_eq!(labels(&mut k, 1.0), ["Hi there", cmd_k, "x"], "a chord ends the burst");
        // A pause longer than the burst starts a new cap.
        k.key(2.1, &typed("y"));
        assert_eq!(labels(&mut k, 2.1), [cmd_k, "x", "y"], "and the oldest leaves early: at most three show");
        assert_eq!(labels(&mut k, 4.1), Vec::<String>::new(), "every cap fades once its hold ends");
    }

    #[test]
    fn repeated_chords_count_and_named_keys_use_glyphs() {
        let mut k = Keycaps::default();
        let down = KeyEvent { key: Key::Down, mods: Mods::default() };
        for i in 0..3 { k.key(0.2 * i as f64, &down); }
        let shift_tab = KeyEvent { key: Key::Tab, mods: Mods { shift: true, ..Mods::default() } };
        k.key(0.6, &shift_tab);
        let all = KeyEvent { key: Key::Text(" ".into()), mods: Mods { ctrl: true, alt: true, shift: true, cmd: true, logo: true } };
        k.key(0.7, &all);
        assert_eq!(labels(&mut k, 0.7), ["↓ ×3", "⇧⇥", "⌃⌥⇧⌘␣"]);
        k.chord(0.8, &"cmd+space".parse().unwrap());
        k.key(0.9, &KeyEvent { key: Key::Escape, mods: Mods::default() });
        k.key(1.0, &KeyEvent { key: Key::Enter, mods: Mods::default() });
        let cmd = if cfg!(target_os = "macos") { "⌘" } else { "⌃" };
        assert_eq!(labels(&mut k, 1.0), [format!("{cmd}␣"), "⎋".into(), "↩".into()]);
    }

    #[test]
    fn key_names_are_title_case_and_letters_capitals() {
        let mut k = Keycaps::default();
        k.chord(0.0, &"alt+shift+k".parse().unwrap());
        k.chord(0.1, &"alt+f12".parse().unwrap());
        let ctrl = |s: &str| KeyEvent { key: Key::Text(s.into()), mods: Mods { ctrl: true, ..Mods::default() } };
        k.key(0.2, &ctrl("SPACE"));
        assert_eq!(labels(&mut k, 0.2), ["⌥⇧K", "⌥F12", "⌃Space"]);
        assert_eq!((title("ı"), title("ß"), title("capslock")), ("ı".into(), "ß".into(), "Capslock".into()));
    }

    #[test]
    fn long_typing_elides_its_start_and_spaces_alone_show_as_glyphs() {
        let mut k = Keycaps::default();
        for (i, c) in "the quick brown fox jumps over".chars().enumerate() { k.key(0.05 * i as f64, &typed(&c.to_string())); }
        assert_eq!(labels(&mut k, 1.5), ["…ck brown fox jumps over"]);
        let mut k = Keycaps::default();
        k.key(0.0, &typed(" "));
        assert_eq!(labels(&mut k, 0.0), ["␣"]);
    }

    #[test]
    fn corners_and_themes_parse() {
        assert_eq!(Corner::parse("top-right"), Some(Corner::TopRight));
        assert_eq!(Corner::parse("middle"), None);
        assert_eq!((parse_theme("auto"), parse_theme("light"), parse_theme("dark"), parse_theme("blue")),
            (Some(None), Some(Some(false)), Some(Some(true)), None));
    }
}
