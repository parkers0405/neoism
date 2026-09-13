//! Pure original-map planning. No device creation and no map rewriting.
use anyhow::{ensure, Result};
use xkbcommon::xkb;

// Evdev + 8 positions with ordinary printable DomCodes. XKB also exposes
// synthetic/consumer aliases (e.g. XF86NumericPound at 531) whose UTF-8 lookup
// looks printable but is not a text key in GTK/Chromium. Never select those.
const TEXT_POSITIONS:&[u32]=&[
    10,11,12,13,14,15,16,17,18,19,20,21,
    24,25,26,27,28,29,30,31,32,33,34,35,
    38,39,40,41,42,43,44,45,46,47,48,49,
    51,52,53,54,55,56,57,58,59,60,61,65,
    94,97,132, // ISO 102nd, JIS Ro and Yen.
];
const MODIFIER_POSITIONS:&[u32]=&[37,50,62,64,66,105,108,133,134,135];
fn direct_printable(sym:u32,ch:char)->bool {
    // Legacy printable keysyms or an exact directly encoded Unicode keysym.
    // In particular, this excludes XF86/phone aliases, dead/compose and controls.
    matches!(sym,0x20..=0x7e | 0xa0..=0x20ff) || sym==(0x01000000 | u32::from(ch))
}

#[derive(Debug)]
pub(crate) struct KeyPlan {
    pub(super) units: Vec<Vec<u32>>,
    pub(super) source: String,
    pub(super) group: u32,
    pub(crate) units_kind: &'static str,
}
impl KeyPlan { pub(crate) fn unit_count(&self) -> usize { self.units.len() } }

fn state(map: &xkb::Keymap, group: u32) -> xkb::State {
    let mut s = xkb::State::new(map);
    s.update_mask(0, 0, 0, 0, 0, group);
    s
}
fn transient(s: &xkb::State, group: u32) -> bool {
    s.serialize_mods(xkb::STATE_MODS_LATCHED | xkb::STATE_MODS_LOCKED) == 0
        && s.serialize_layout(xkb::STATE_LAYOUT_EFFECTIVE) == group
        && s.serialize_layout(xkb::STATE_LAYOUT_DEPRESSED | xkb::STATE_LAYOUT_LATCHED) == 0
}
fn neutral(s: &xkb::State, group: u32) -> bool {
    transient(s, group) && s.serialize_mods(xkb::STATE_MODS_EFFECTIVE) == 0
}
fn forbidden(map: &xkb::Keymap) -> u32 {
    ["Control", "Mod1", "Mod4", "Alt", "Meta", "Super", "Hyper"].iter().fold(0, |mask, name| {
        let index = map.mod_get_index(name);
        mask | if index < 32 { 1u32 << index } else { 0 }
    })
}
fn selector(map: &xkb::Keymap, group: u32, code: u32, sym: u32) -> bool {
    let mut s = state(map, group);
    let key = xkb::Keycode::new(code);
    if s.key_get_one_sym(key).raw() != sym { return false; }
    s.update_key(key, xkb::KeyDirection::Down);
    let mask = s.serialize_mods(xkb::STATE_MODS_EFFECTIVE);
    let valid = mask != 0 && mask & forbidden(map) == 0 && transient(&s, group);
    s.update_key(key, xkb::KeyDirection::Up);
    valid && neutral(&s, group)
}
fn candidate(map: &xkb::Keymap, group: u32, modifiers: &[u32], code: u32) -> Option<char> {
    if modifiers.contains(&code) { return None; }
    let mut s = state(map, group);
    for modifier in modifiers {
        s.update_key(xkb::Keycode::new(*modifier), xkb::KeyDirection::Down);
        if !transient(&s, group) || s.serialize_mods(xkb::STATE_MODS_EFFECTIVE) & forbidden(map) != 0 { return None; }
    }
    let key = xkb::Keycode::new(code);
    let sym = s.key_get_one_sym(key).raw();
    let output=s.key_get_utf8(key);
    let mut chars=output.chars();
    let ch=chars.next()?;
    if chars.next().is_some() || ch.is_control() || u32::from(ch)>0xffff {return None;}
    // Dead keys and Multi_key must never initiate an application compose state.
    if !direct_printable(sym,ch) || s.key_get_utf8(key) != output { return None; }
    let before = s.serialize_mods(xkb::STATE_MODS_EFFECTIVE);
    s.update_key(key, xkb::KeyDirection::Down);
    if !transient(&s, group) || s.serialize_mods(xkb::STATE_MODS_EFFECTIVE) != before || s.key_get_utf8(key) != output { return None; }
    s.update_key(key, xkb::KeyDirection::Up);
    if !transient(&s, group) || s.serialize_mods(xkb::STATE_MODS_EFFECTIVE) != before { return None; }
    for modifier in modifiers.iter().rev() {
        s.update_key(xkb::Keycode::new(*modifier), xkb::KeyDirection::Up);
        if !transient(&s, group) || s.serialize_mods(xkb::STATE_MODS_EFFECTIVE) & !before != 0 { return None; }
    }
    neutral(&s, group).then_some(ch)
}
pub(super) fn text(map: &xkb::Keymap, group: u32, text: &str) -> Result<Option<KeyPlan>> {
    super::validate_baseline(map)?;
    ensure!(group < map.num_layouts(), "Invalid original layout group");
    ensure!(text.chars().count() <= 512, "Text exceeds 512 Unicode characters");
    ensure!(!text.chars().any(char::is_control), "Text cannot contain control characters; use explicit key actions");
    if text.chars().any(|ch| u32::from(ch) > 0xffff) { return Ok(None); }
    let codes = TEXT_POSITIONS.iter().copied().filter(|code|map.key_get_name(xkb::Keycode::new(*code)).is_some()).collect::<Vec<_>>();
    let mut selector_codes=codes.clone();
    selector_codes.extend_from_slice(MODIFIER_POSITIONS);
    selector_codes.sort_unstable();selector_codes.dedup();
    let shifts = selector_codes.iter().copied().filter(|c| selector(map, group, *c, 0xffe1) || selector(map, group, *c, 0xffe2)).collect::<Vec<_>>();
    let level3 = selector_codes.iter().copied().filter(|c| selector(map, group, *c, 0xfe03)).collect::<Vec<_>>();
    let mut modifiers = vec![vec![]];
    modifiers.extend(shifts.iter().chain(&level3).map(|c| vec![*c]));
    modifiers.extend(shifts.iter().flat_map(|s| level3.iter().map(move |l| vec![*s, *l])));
    modifiers.sort_by(|a,b| a.len().cmp(&b.len()).then(a.cmp(b)));
    // Enumerate every safe chord once in deterministic cost/keycode order.
    // Whole strings (including unsupported suffixes) are then pure lookups,
    // rather than constructing an XKB state for each scalar × candidate pair.
    let mut candidates=std::collections::HashMap::<char,Vec<u32>>::new();
    for mods in &modifiers {
        for code in &codes {
            if let Some(ch)=candidate(map,group,mods,*code) {
                candidates.entry(ch).or_insert_with(|| {let mut unit=mods.clone();unit.push(*code);unit});
            }
        }
    }
    let mut units=Vec::with_capacity(text.chars().count());
    for ch in text.chars() {
        let Some(unit)=candidates.get(&ch) else {return Ok(None);};
        units.push(unit.clone());
    }
    Ok(Some(KeyPlan { units, units_kind: "unicode_scalar", source: map.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1), group }))
}
pub(super) fn keys(map: &xkb::Keymap, group: u32, keys: &[enigo::Key]) -> Result<KeyPlan> {
    super::validate_baseline(map)?;
    ensure!(group < map.num_layouts(), "Invalid original layout group");
    let codes = super::super::shortcuts::resolve_in_map(map, group, keys)?.into_iter().map(u32::from).collect::<Vec<_>>();
    let mut s = state(map, group);
    for code in &codes { s.update_key(xkb::Keycode::new(*code), xkb::KeyDirection::Down); ensure!(transient(&s, group), "Shortcut changes locked/latched modifiers or layout"); }
    for code in codes.iter().rev() { s.update_key(xkb::Keycode::new(*code), xkb::KeyDirection::Up); }
    ensure!(neutral(&s, group), "Shortcut does not release to neutral state");
    Ok(KeyPlan { units_kind: "chord", units: if codes.is_empty() { vec![] } else { vec![codes] }, source: map.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1), group })
}
