//! Independent live font slots. Named editor styles retain programming ligatures.

use rune::Family;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

pub const CHAT: &str = "Geist";
pub const MONO: &str = "Geist Mono";

// Rune's named families require static strings. Intern only new choices, never each frame.
pub fn code_family(name: &str) -> Family {
    let list = fallback(name, MONO);
    let name = list.as_str();
    static NAMES: OnceLock<Mutex<HashMap<String, &'static str>>> = OnceLock::new();
    let mut names = NAMES.get_or_init(Default::default).lock().unwrap();
    let name = *names.entry(name.to_owned()).or_insert_with(|| Box::leak(name.to_owned().into_boxed_str()));
    Family::Named(name)
}

fn fallback(name: &str, default: &str) -> String {
    if name == default { name.to_owned() } else { format!("{name}, {default}") }
}

pub fn configured(value: &Value) -> (String, String) {
    let family = |value: &Value, default: &str| value.as_str().map(str::trim).filter(|s| !s.is_empty()).unwrap_or(default).to_owned();
    (family(&value["appearance"]["chat-font-family"], CHAT), family(&value["appearance"]["fonts"]["family"], MONO))
}

impl crate::app::App {
    pub(crate) fn sync_font_families(&mut self, tx: &mut rune::Tx) -> bool {
        if !self.fonts_dirty { return false; }
        self.fonts_dirty = false;
        tx.set_font_families(fallback(&self.font_families.0, CHAT), fallback(&self.font_families.1, MONO));
        true
    }

    pub(crate) fn set_font_families(&mut self, families: (String, String)) {
        if self.font_families == families { return; }
        self.ty.code_family = code_family(&families.1);
        self.font_families = families;
        self.fonts_dirty = true;
    }
}