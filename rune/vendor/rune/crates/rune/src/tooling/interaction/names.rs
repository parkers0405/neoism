//! Names for where an interaction happened, made from element paths: `#0/sidebar/row-x1208`
//! reads `sidebar/row-x*`. Unkeyed segments drop out, digit runs and ids become `*`, and a
//! segment with spaces (prose someone passed as a key) becomes `~`, so the same control on any
//! row shares one name and no user text gets through. Names are interned: after a name's first
//! sighting, naming it again allocates nothing.

use std::hash::Hasher;
use std::sync::Arc;

use rustc_hash::{FxHashMap, FxHasher};

/// Segments a control name keeps, from the end.
const CONTROL_SEGMENTS: usize = 4;
/// Segments a page name keeps, from the end.
const PAGE_SEGMENTS: usize = 3;
/// Characters a segment keeps.
const SEGMENT_CHARS: usize = 32;

/// Interned names, by hash.
#[derive(Default)]
pub(crate) struct Names {
    names: FxHashMap<u64, Arc<str>>,
    scratch: String,
}

/// A name's hash and its text.
#[derive(Clone)]
pub(crate) struct Name {
    pub hash: u64,
    pub text: Arc<str>,
}

impl Names {
    /// The control name for `path`.
    pub(crate) fn control(&mut self, path: &str) -> Name { self.intern(|out| normalize(path, CONTROL_SEGMENTS, out)) }

    /// A route: a page, and the modal open over it.
    pub(crate) fn route(&mut self, page: Option<&str>, modal: Option<&str>) -> Name {
        self.intern(|out| {
            match page {
                Some(page) => normalize(page, PAGE_SEGMENTS, out),
                None => out.push_str("root"),
            }
            if out.is_empty() { out.push_str("root"); }
            if let Some(modal) = modal {
                out.push_str("+modal:");
                let at = out.len();
                normalize(modal, 1, out);
                if out.len() == at { out.push('*'); }
            }
        })
    }

    /// A fixed name (`window` for a key nothing focused took).
    pub(crate) fn fixed(&mut self, name: &str) -> Name { self.intern(|out| out.push_str(name)) }

    fn intern(&mut self, write: impl FnOnce(&mut String)) -> Name {
        let mut scratch = std::mem::take(&mut self.scratch);
        scratch.clear();
        write(&mut scratch);
        let mut h = FxHasher::default();
        h.write(scratch.as_bytes());
        let hash = h.finish();
        let text = self.names.entry(hash).or_insert_with(|| Arc::from(scratch.as_str())).clone();
        self.scratch = scratch;
        Name { hash, text }
    }
}

/// Writes `path`'s last `keep` keyed segments, normalized, joined by `/`.
pub(crate) fn normalize(path: &str, keep: usize, out: &mut String) {
    let keyed = path.split('/').filter(|s| !s.is_empty() && !s.starts_with('#'));
    let skip = keyed.clone().count().saturating_sub(keep);
    for (i, seg) in keyed.skip(skip).enumerate() {
        if i > 0 { out.push('/'); }
        segment(seg, out);
    }
}

fn segment(seg: &str, out: &mut String) {
    if seg.chars().any(char::is_whitespace) {
        out.push('~');
        return;
    }
    let start = out.len();
    let mut rest = seg;
    while let Some(c) = rest.chars().next() {
        if c.is_ascii_alphanumeric() {
            let end = rest.find(|c: char| !c.is_ascii_alphanumeric()).unwrap_or(rest.len());
            word(&rest[..end], out);
            rest = &rest[end..];
        } else {
            out.push(if matches!(c, '-' | '_' | '.' | ':') { c } else { '_' });
            rest = &rest[c.len_utf8()..];
        }
        if out.len() - start >= SEGMENT_CHARS {
            out.truncate(start + SEGMENT_CHARS);
            return;
        }
    }
}

/// An alphanumeric run: an id (8 or more characters with a digit) is `*`; otherwise each digit run is.
fn word(w: &str, out: &mut String) {
    let digits = w.bytes().any(|b| b.is_ascii_digit());
    if digits && w.len() >= 8 {
        star(out);
        return;
    }
    for c in w.chars() {
        if c.is_ascii_digit() { star(out) } else { out.push(c) }
    }
}

fn star(out: &mut String) {
    if !out.ends_with('*') { out.push('*'); }
}
