//! Parsing a `--script`'s JSON into input events.

use super::*;

/// `{"text": …, "html": …, "rtf": …, "png": "path/to/image.png"}`, each optional, as another
/// app's clipboard would hold.
pub(crate) fn paste_content(e: &Value) -> crate::ClipboardContent {
    let mut content = crate::ClipboardContent::new(e["text"].as_str().unwrap_or(""));
    if let Some(html) = e["html"].as_str() { content = content.with_html(html); }
    if let Some(rtf) = e["rtf"].as_str() { content = content.with_rtf(rtf); }
    if let Some(path) = e["png"].as_str() {
        let bytes = std::fs::read(path).unwrap_or_else(|err| die(&format!("paste png {path}: {err}")));
        let image = crate::ClipboardImage::from_png(bytes).unwrap_or_else(|| die(&format!("paste png {path}: not a PNG, or over clipboard::MAX_IMAGE_BYTES")));
        content = content.with_image(image);
    }
    content
}

pub(crate) fn parse_script(list: Value) -> Vec<Ev> {
    let Value::Array(items) = list else { die("script must be an array of events or {\"events\": [...]}") };
    let mut out = Vec::new();
    for e in items {
        let t = e["t"].as_f64().unwrap_or(0.0);
        if let Some(focused) = e["focus"].as_bool() {
            out.push(Ev { t, kind: Kind::Focus(focused), pos: Pos::Rel(0.0, 0.0) });
            continue;
        }
        if let Some(item) = e["menu"].as_str() {
            out.push(Ev { t, kind: Kind::Menu(item.into()), pos: Pos::Rel(0.0, 0.0) });
            continue;
        }
        if e["reopen"].as_bool() == Some(true) {
            out.push(Ev { t, kind: Kind::Reopen, pos: Pos::Rel(0.0, 0.0) });
            continue;
        }
        if let Some(url) = e["open_url"].as_str() {
            out.push(Ev { t, kind: Kind::OpenUrl(url.into()), pos: Pos::Rel(0.0, 0.0) });
            continue;
        }
        if let Some(chord) = e["physical"].as_str() {
            let chord: crate::Shortcut = chord.parse().unwrap_or_else(|e: crate::ShortcutError| die(&e.to_string()));
            let repeat = e["repeat"].as_bool().unwrap_or(false);
            for pressed in [true, false] {
                out.push(Ev { t, kind: Kind::Physical(crate::PhysicalInput::Key {
                    key: chord.physical_key(), mods: chord.modifiers(), pressed, repeat: repeat && pressed,
                }), pos: Pos::Rel(0.0, 0.0) });
            }
            // Then the modifiers come up, as on a real keyboard, so nothing stays held.
            if chord.modifiers() != Mods::default() {
                out.push(Ev { t, kind: Kind::Physical(crate::PhysicalInput::Modifiers(Mods::default())), pos: Pos::Rel(0.0, 0.0) });
            }
            continue;
        }
        // `{"hold": "alt"}` holds modifiers down until `{"hold": ""}` lets them go, as the window
        // reports them (`App::on_modifiers`): an Alt-hold reveal.
        if let Some(held) = e["hold"].as_str() {
            let mut mods = Mods::default();
            for part in held.split('+').filter(|p| !p.is_empty()) {
                match part {
                    "shift" => mods.shift = true,
                    "ctrl" | "control" => mods.ctrl = true,
                    "alt" | "option" => mods.alt = true,
                    "super" | "logo" | "win" => mods.logo = true,
                    "cmd" => mods.cmd = true,
                    other => die(&format!("unknown modifier {other:?} in hold")),
                }
            }
            out.push(Ev { t, kind: Kind::Physical(crate::PhysicalInput::Modifiers(mods)), pos: Pos::Rel(0.0, 0.0) });
            continue;
        }
        if let Some(shortcut) = e["shortcut"].as_str() {
            let shortcut = shortcut.parse().unwrap_or_else(|e: crate::ShortcutError| die(&e.to_string()));
            out.push(Ev { t, kind: Kind::Shortcut(shortcut), pos: Pos::Rel(0.0, 0.0) });
            continue;
        }
        if !e["paste"].is_null() {
            let paste = if e["paste"].is_string() { serde_json::json!({ "text": e["paste"] }) } else { e["paste"].clone() };
            out.push(Ev { t, kind: Kind::Paste(paste_content(&paste)), pos: Pos::Rel(0.0, 0.0) });
            continue;
        }
        if parse_files(&e, t, &mut out) { continue; }
        if e["type"] == "shortcut" {
            let chord = e["chord"].as_str().unwrap_or_else(|| die("a \"shortcut\" event needs a \"chord\", like \"cmd+space\""));
            let chord = chord.parse().unwrap_or_else(|e: crate::ShortcutError| die(&e.to_string()));
            out.push(Ev { t, kind: Kind::Shortcut(chord), pos: Pos::Rel(0.0, 0.0) });
            continue;
        }
        let pos = if let Some(k) = e["key"].as_str() {
            Pos::Key(k.to_owned())
        } else if let (Some(x), Some(y)) = (e["x"].as_f64(), e["y"].as_f64()) {
            Pos::Xy(x as f32, y as f32)
        } else {
            Pos::Rel(0.0, 0.0)
        };
        let ty = e["type"].as_str().unwrap_or("tap");
        // `"button": "middle"` (or "back", "forward", "secondary") presses with another button.
        let button = match e["button"].as_str().unwrap_or("primary") {
            "primary" => crate::PointerButton::Primary,
            "secondary" => crate::PointerButton::Secondary,
            "middle" => crate::PointerButton::Middle,
            "back" => crate::PointerButton::Back,
            "forward" => crate::PointerButton::Forward,
            other => die(&format!("unknown button {other}")),
        };
        let (down, up) = match button {
            crate::PointerButton::Primary => (Kind::Down, Kind::Up),
            crate::PointerButton::Secondary => (Kind::SecondaryDown, Kind::SecondaryUp),
            b => (Kind::ButtonDown(b), Kind::ButtonUp(b)),
        };
        // A gesture at an element or point moves the pointer there first.
        let gesture_at = |out: &mut Vec<Ev>, pos: &Pos| if !matches!(pos, Pos::Rel(..)) { out.push(Ev { t, kind: Kind::Move, pos: pos.clone() }) };
        match ty {
            "move" => out.push(Ev { t, kind: Kind::Move, pos }),
            "down" => {
                out.push(Ev { t, kind: Kind::Move, pos: pos.clone() });
                out.push(Ev { t, kind: down, pos });
            }
            "up" => out.push(Ev { t, kind: up, pos }),
            "secondary-down" => out.push(Ev { t, kind: Kind::SecondaryDown, pos }),
            "secondary-up" => out.push(Ev { t, kind: Kind::SecondaryUp, pos }),
            "secondary-click" => {
                out.push(Ev { t, kind: Kind::SecondaryDown, pos: pos.clone() });
                out.push(Ev { t: t + e["hold"].as_f64().unwrap_or(0.08), kind: Kind::SecondaryUp, pos });
            }
            "tap" | "click" => {
                out.push(Ev { t, kind: Kind::Move, pos: pos.clone() });
                out.push(Ev { t, kind: down, pos });
                out.push(Ev { t: t + e["hold"].as_f64().unwrap_or(0.08), kind: up, pos: Pos::Rel(0.0, 0.0) });
            }
            "double-click" | "double-tap" | "triple-click" => {
                // Two (three) presses `gap` apart (default 0.15 s), inside the default double-click interval.
                let hold = e["hold"].as_f64().unwrap_or(0.06);
                let gap = e["gap"].as_f64().unwrap_or(0.15).max(hold + 0.01);
                let presses = if ty == "triple-click" { 3 } else { 2 };
                for start in (0..presses).map(|k| t + gap * k as f64) {
                    out.push(Ev { t: start, kind: Kind::Move, pos: pos.clone() });
                    out.push(Ev { t: start, kind: Kind::Down, pos: pos.clone() });
                    out.push(Ev { t: start + hold, kind: Kind::Up, pos: Pos::Rel(0.0, 0.0) });
                }
            }
            "drag" => {
                let dx = e["dx"].as_f64().unwrap_or(0.0) as f32;
                let dy = e["dy"].as_f64().unwrap_or(0.0) as f32;
                let dur = e["dur"].as_f64().unwrap_or(0.3).max(0.01);
                out.push(Ev { t, kind: Kind::Move, pos: pos.clone() });
                out.push(Ev { t, kind: down, pos });
                let steps = (dur * 120.0).ceil() as usize;
                for s in 1..=steps {
                    let f = s as f32 / steps as f32;
                    // Ease in-out so the hand starts and stops gently.
                    let f = f * f * (3.0 - 2.0 * f);
                    out.push(Ev {
                        t: t + dur * s as f64 / steps as f64,
                        kind: Kind::Move,
                        pos: Pos::Rel(dx * f, dy * f),
                    });
                }
                out.push(Ev { t: t + dur + 0.02, kind: up, pos: Pos::Rel(dx, dy) });
            }
            "type" => {
                let text = e["text"].as_str().unwrap_or("");
                let every = e["every"].as_f64().unwrap_or(0.035);
                for (k, c) in text.chars().enumerate() {
                    let ev = KeyEvent { key: Key::Text(c.to_string()), mods: Mods::default() };
                    out.push(Ev { t: t + every * k as f64, kind: Kind::Key(ev, false), pos: Pos::Rel(0.0, 0.0) });
                }
            }
            "key" | "key-up" => {
                let name = e["name"].as_str().unwrap_or("");
                let mods = script_mods(&e);
                let key = if mods.cmd && name.len() == 1 {
                    Key::Command(name.chars().next().unwrap().to_ascii_lowercase())
                } else {
                    key_named(name).unwrap_or_else(|| die(&format!("unknown key {name}")))
                };
                let repeat = e["repeat"].as_bool().unwrap_or(false);
                // `"code"` names a press's physical key (`"7"` for `⌘⇧7`), with a `Tx::key_base`; it comes back up straight after.
                if let Some(code) = e["code"].as_str() {
                    let code = code.parse::<crate::Shortcut>().unwrap_or_else(|e| die(&e.to_string())).physical_key();
                    let base = script_key_base(&e, name, mods);
                    let press = |pressed| crate::PhysicalInput::Key { key: code, mods, pressed, repeat: repeat && pressed };
                    if ty == "key-up" {
                        out.push(Ev { t, kind: Kind::PhysicalKey(press(false), KeyEvent { key, mods }, base), pos: Pos::Rel(0.0, 0.0) });
                    } else {
                        out.push(Ev { t, kind: Kind::PhysicalKey(press(true), KeyEvent { key, mods }, base), pos: Pos::Rel(0.0, 0.0) });
                        out.push(Ev { t, kind: Kind::Physical(press(false)), pos: Pos::Rel(0.0, 0.0) });
                    }
                    continue;
                }
                let kind = if ty == "key-up" { Kind::KeyUp(KeyEvent { key, mods }) } else { Kind::Key(KeyEvent { key, mods }, repeat) };
                out.push(Ev { t, kind, pos: Pos::Rel(0.0, 0.0) });
            }
            "wheel" => {
                // One delta, unlike "scroll"'s eased swipe, with modifiers and a phase.
                let d = |k: &str| e[k].as_f64().unwrap_or(0.0) as f32;
                gesture_at(&mut out, &pos);
                let ev = crate::WheelEvent::new(d("dx"), d("dy")).mods(script_mods(&e)).phase(script_phase(&e))
                    .precise(e["precise"].as_bool().unwrap_or(true));
                out.push(Ev { t, kind: Kind::WheelEvent(ev), pos });
            }
            "magnify" => {
                gesture_at(&mut out, &pos);
                let ev = crate::MagnifyEvent::new(e["delta"].as_f64().unwrap_or(0.0) as f32).phase(script_phase(&e));
                out.push(Ev { t, kind: Kind::Magnify(ev), pos });
            }
            "rotate" => {
                gesture_at(&mut out, &pos);
                let ev = crate::RotateEvent::new(e["degrees"].as_f64().unwrap_or(0.0) as f32).phase(script_phase(&e));
                out.push(Ev { t, kind: Kind::Rotate(ev), pos });
            }
            "smart-magnify" => {
                gesture_at(&mut out, &pos);
                out.push(Ev { t, kind: Kind::SmartMagnify, pos });
            }
            "paste" => out.push(Ev { t, kind: Kind::Paste(paste_content(&e)), pos: Pos::Rel(0.0, 0.0) }),
            "scroll" => eased_input::swipe(&mut out, &e, t, pos),
            "resize" => eased_input::resize(&mut out, &e, t),
            "fling" => {
                let v = |k: &str| e[k].as_f64().unwrap_or(0.0) as f32;
                out.push(Ev { t, kind: Kind::Fling(v("vx"), v("v")), pos });
            }
            "menu-pick" => {
                let id = e["id"].as_str().unwrap_or_else(|| die("menu-pick needs an \"id\""));
                out.push(Ev { t, kind: Kind::PopupMenu(Some(id.into())), pos: Pos::Rel(0.0, 0.0) });
            }
            "menu-cancel" => out.push(Ev { t, kind: Kind::PopupMenu(None), pos: Pos::Rel(0.0, 0.0) }),
            "file-dialog" => {
                let path = e["path"].as_str().unwrap_or_else(|| die("file-dialog needs a \"path\" (file-dialog-cancel cancels)"));
                out.push(Ev { t, kind: Kind::FileDialog(Some(path.into())), pos: Pos::Rel(0.0, 0.0) });
            }
            "file-dialog-cancel" => out.push(Ev { t, kind: Kind::FileDialog(None), pos: Pos::Rel(0.0, 0.0) }),
            "mark" => {
                let name = e["name"].as_str().filter(|n| !n.is_empty()).unwrap_or_else(|| die("a mark needs a \"name\""));
                out.push(Ev { t, kind: Kind::Mark(name.to_owned()), pos });
            }
            other => die(&format!("unknown event type {other}")),
        }
    }
    out.sort_by(|a, b| a.t.total_cmp(&b.t));
    out
}

/// `{"drag_files": {"paths": […], "at": [x, y]}}` (or `"key": …` for an element's centre) moves a
/// file drag from another app over the window; `{"drag_files": "leave"}` takes it away.
/// `{"drop_files": {…, "hover": s}}` drags the files there for `s` seconds (default 0), then
/// drops them. Whether `e` was one.
pub(crate) fn parse_files(e: &Value, t: f64, out: &mut Vec<Ev>) -> bool {
    let (spec, drop) = match (e.get("drag_files"), e.get("drop_files")) {
        (Some(spec), _) => (spec, false),
        (None, Some(spec)) => (spec, true),
        (None, None) => return false,
    };
    if !drop && spec == "leave" {
        out.push(Ev { t, kind: Kind::Files(FileStep::Leave), pos: Pos::Rel(0.0, 0.0) });
        return true;
    }
    let name = if drop { "drop_files" } else { "drag_files" };
    let paths: Vec<std::path::PathBuf> = spec["paths"].as_array()
        .unwrap_or_else(|| die(&format!("{name} needs \"paths\": [\"a file\", …]")))
        .iter().map(|p| p.as_str().unwrap_or_else(|| die(&format!("{name}: paths are strings"))).into()).collect();
    let pos = match (spec["at"].as_array(), spec["key"].as_str()) {
        (Some(at), _) if at.len() == 2 && at.iter().all(Value::is_number) =>
            Pos::Xy(at[0].as_f64().unwrap_or(0.0) as f32, at[1].as_f64().unwrap_or(0.0) as f32),
        (None, Some(key)) => Pos::Key(key.to_owned()),
        _ => die(&format!("{name} needs \"at\": [x, y] or \"key\"")),
    };
    out.push(Ev { t, kind: Kind::Files(FileStep::Hover(paths.clone())), pos: pos.clone() });
    if drop {
        let hover = spec["hover"].as_f64().unwrap_or(0.0).max(0.0);
        out.push(Ev { t: t + hover, kind: Kind::Files(FileStep::Drop(paths)), pos });
    }
    true
}
