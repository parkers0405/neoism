//! Paint-only streaming identity. Only recent live bodies are retained; history
//! is supplied as a sharp baseline by ingestion, never inferred from visibility.
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, VecDeque},
    ops::Range,
    rc::Rc,
    time::Duration,
};
use sugarloaf::text::TextReveal;
use web_time::Instant;

pub const REVEAL_DURATION: Duration = Duration::from_millis(180);
#[derive(Clone)]
pub struct SourceUpdate {
    pub text: String,
    pub at: Option<Instant>,
}
#[derive(Clone)]
struct Age {
    range: Range<usize>,
    at: Instant,
}
#[derive(Default)]
pub struct Projection {
    pub canonical: String,
    pub lines: HashMap<usize, usize>,
    runs: RefCell<HashMap<usize, Rc<RunMap>>>,
    layout_owner: Option<Rc<dyn std::any::Any>>,
}
struct RunMap {
    text: String,
    chars: Vec<(usize, usize, usize)>,
    canonical_len: usize,
}
impl RunMap {
    fn new(text: &str) -> Self {
        let mut canonical_len = 0;
        let chars = text
            .char_indices()
            .filter_map(|(byte, ch)| {
                if ch.is_whitespace() {
                    return None;
                }
                let relative = canonical_len;
                canonical_len += ch.len_utf8();
                Some((byte, ch.len_utf8(), relative))
            })
            .collect();
        Self {
            text: text.to_owned(),
            chars,
            canonical_len,
        }
    }
}
impl Projection {
    pub fn retain_layout<T: std::any::Any>(&mut self, owner: Rc<T>) {
        self.layout_owner = Some(owner);
    }
    pub fn push(&mut self, source: &str, display: &str) {
        self.lines
            .insert(source.as_ptr() as usize, self.canonical.len());
        self.canonical
            .extend(display.chars().filter(|c| !c.is_whitespace()));
    }
}
#[derive(Default)]
struct Body {
    pending: VecDeque<SourceUpdate>,
    source: String,
    canonical: String,
    has_canonical_baseline: bool,
    ages: Vec<Age>,
    projection: Option<(usize, Rc<Projection>)>,
    latest: Option<Instant>,
}
pub struct TextRevealState {
    enabled: bool,
    session: Option<String>,
    bodies: HashMap<String, Body>,
    visible_until: Rc<Cell<Option<Instant>>>,
}
impl Default for TextRevealState {
    fn default() -> Self {
        Self {
            enabled: true,
            session: None,
            bodies: HashMap::new(),
            visible_until: Rc::new(Cell::new(None)),
        }
    }
}
impl TextRevealState {
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.clear();
        }
    }
    pub fn clear(&mut self) {
        self.bodies.clear();
        self.visible_until.set(None);
    }
    pub fn scope(&mut self, session: Option<&str>) {
        if self.session.as_deref() != session {
            self.clear();
            self.session = session.map(str::to_owned);
        }
    }
    pub fn begin_frame(&mut self) {
        self.visible_until.set(None);
        self.prune(Instant::now());
    }
    pub fn is_animating_for(&self, session: Option<&str>) -> bool {
        self.session.as_deref() == session && self.is_animating()
    }
    pub fn is_animating(&self) -> bool {
        self.enabled
            && self
                .visible_until
                .get()
                .is_some_and(|end| Instant::now() < end)
    }
    fn prune(&mut self, now: Instant) {
        self.bodies.retain(|_, b| {
            b.latest
                .is_some_and(|at| now.saturating_duration_since(at) < REVEAL_DURATION)
        });
    }
    pub fn should_record(&self, id: &str) -> bool {
        self.enabled && !id.is_empty()
    }
    pub fn record(&mut self, id: &str, before: &str, after: &str) {
        self.record_at(id, before, after, Instant::now());
    }
    fn record_at(&mut self, id: &str, before: &str, after: &str, now: Instant) {
        self.prune(now);
        if !self.enabled || id.is_empty() || before == after {
            return;
        }
        // Keep one initial sharp baseline and ONE latest live revision, not
        // every token's full source. Tokens arriving between paints share the
        // latest arrival timestamp; ages from the previous paint stay intact.
        if self.bodies.len() >= 128 && !self.bodies.contains_key(id) {
            self.bodies.clear();
        }
        let body = self.bodies.entry(id.to_owned()).or_default();
        if !body.has_canonical_baseline && body.pending.is_empty() {
            body.pending.push_back(SourceUpdate {
                text: before.to_owned(),
                at: None,
            });
        }
        if let Some(latest) = body.pending.back_mut().filter(|u| u.at.is_some()) {
            latest.text.clear();
            latest.text.push_str(after);
            latest.at = Some(now);
        } else {
            body.pending.push_back(SourceUpdate {
                text: after.to_owned(),
                at: Some(now),
            });
        }
        body.source.clear();
        body.source.push_str(after);
        body.latest = Some(now);
        body.projection = None;
    }
    pub fn reconcile_history<'a>(
        &mut self,
        rows: impl Iterator<Item = (&'a str, &'a str)>,
    ) {
        let rows: HashMap<&str, &str> = rows.collect();
        self.bodies.retain(|id, body| {
            rows.get(id.as_str())
                .is_some_and(|text| *text == body.source)
        });
        // A new frame will re-register the still-visible, identical live rows.
        self.visible_until.set(None);
    }
    pub fn pending(&mut self, id: &str) -> Vec<SourceUpdate> {
        self.prune(Instant::now());
        self.bodies
            .get_mut(id)
            .map(|b| b.pending.drain(..).collect())
            .unwrap_or_default()
    }
    pub fn apply_projection(
        &mut self,
        id: &str,
        projection: &Projection,
        at: Option<Instant>,
    ) {
        if let Some(b) = self.bodies.get_mut(id) {
            b.has_canonical_baseline = true;
            if let Some(at) = at {
                update_identity(&mut b.canonical, &mut b.ages, &projection.canonical, at);
            } else {
                b.canonical.clone_from(&projection.canonical);
                b.ages.clear();
            }
        }
    }
    pub fn invalidate_projections(&mut self) {
        for body in self.bodies.values_mut() {
            body.projection = None;
        }
    }
    pub fn has_body(&self, id: &str) -> bool {
        self.bodies.contains_key(id)
    }
    pub fn cached_projection(&self, id: &str, identity: usize) -> Option<Rc<Projection>> {
        self.bodies
            .get(id)?
            .projection
            .as_ref()
            .filter(|(key, projection)| {
                *key == identity && projection.layout_owner.is_some()
            })
            .map(|(_, p)| p.clone())
    }
    pub fn paint(
        &mut self,
        id: &str,
        identity: usize,
        projection: Rc<Projection>,
    ) -> PaintScope {
        let now = Instant::now();
        let context = self.bodies.get_mut(id).and_then(|b| {
            // A view-only transformation (e.g. raw/diagram toggle) has no live
            // update timestamp. Never apply old positional ages to new ink.
            if b.canonical != projection.canonical {
                b.ages.clear();
                b.canonical.clone_from(&projection.canonical);
            }
            b.ages
                .retain(|age| now.saturating_duration_since(age.at) < REVEAL_DURATION);
            b.projection = Some((identity, projection.clone()));
            (!b.ages.is_empty()).then(|| PaintContext {
                projection,
                ages: b.ages.clone(),
                offset: None,
                aliases: HashMap::new(),
                now,
                visible: self.visible_until.clone(),
            })
        });
        PaintScope(PAINT.with(|p| p.replace(context)))
    }
}
fn update_identity(old: &mut String, ages: &mut Vec<Age>, new: &str, at: Instant) {
    if old == new {
        return;
    }
    let prefix = old
        .chars()
        .zip(new.chars())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .sum::<usize>();
    let suffix = old[prefix..]
        .chars()
        .rev()
        .zip(new[prefix..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .sum::<usize>();
    let old_end = old.len() - suffix;
    let new_end = new.len() - suffix;
    let mut equal = Vec::new();
    if prefix > 0 {
        equal.push(EqualSpan {
            old: 0..prefix,
            new_start: 0,
        });
    }
    // Append and pure deletion stay on the prefix/suffix fast path: no
    // character vectors, edit matrix, or internal matching work.
    if prefix < old_end && prefix < new_end {
        if let Some(interior) =
            bounded_equal_spans(&old[prefix..old_end], &new[prefix..new_end])
        {
            equal.extend(interior.into_iter().map(|span| EqualSpan {
                old: (prefix + span.old.start)..(prefix + span.old.end),
                new_start: prefix + span.new_start,
            }));
        }
        // Over budget: retain only the known prefix/suffix, conservatively
        // revealing the whole changed span rather than guessing provenance.
    }
    if suffix > 0 {
        equal.push(EqualSpan {
            old: old_end..old.len(),
            new_start: new_end,
        });
    }
    let mut kept = Vec::new();
    let mut new_cursor = 0;
    for span in equal {
        if new_cursor < span.new_start {
            kept.push(Age {
                range: new_cursor..span.new_start,
                at,
            });
        }
        // Equal characters with no old Age remain sharp. Intersect only the
        // sorted, non-overlapping recent age ranges, rebasing UTF-8 bytes.
        let start = ages.partition_point(|age| age.range.end <= span.old.start);
        for age in &ages[start..] {
            if age.range.start >= span.old.end {
                break;
            }
            if at.saturating_duration_since(age.at) >= REVEAL_DURATION {
                continue;
            }
            let left = age.range.start.max(span.old.start);
            let right = age.range.end.min(span.old.end);
            if left < right {
                kept.push(Age {
                    range: (span.new_start + left - span.old.start)
                        ..(span.new_start + right - span.old.start),
                    at: age.at,
                });
            }
        }
        new_cursor = span.new_start + span.old.len();
    }
    if new_cursor < new.len() {
        kept.push(Age {
            range: new_cursor..new.len(),
            at,
        });
    }
    kept.sort_by_key(|a| a.range.start);
    // Preserve exact recent timestamps, but never a permanent per-character
    // timestamp array, even under a pathological update firehose.
    if kept.len() > 4096 {
        kept.drain(..kept.len() - 4096);
    }
    *ages = kept;
    *old = new.to_owned();
}
// Fixed bounds apply only to the internally changed span, after common
// prefix/suffix trimming. DP storage is <=128 KiB; at most 65,536 cells are
// evaluated and reconstruction takes <=1,024 character steps. Character
// collection itself stops after 513 scalars per side, never scanning a long
// replacement in full a second time. No dependencies or unbounded edit trace.
const INTERNAL_DIFF_MAX_CHARS: usize = 512;
const INTERNAL_DIFF_MAX_CELLS: usize = 65_536;
struct EqualSpan {
    old: Range<usize>,
    new_start: usize,
}
fn bounded_equal_spans(old: &str, new: &str) -> Option<Vec<EqualSpan>> {
    if old.len() > INTERNAL_DIFF_MAX_CHARS * 4 || new.len() > INTERNAL_DIFF_MAX_CHARS * 4
    {
        return None;
    }
    let old_chars: Vec<_> = old
        .char_indices()
        .take(INTERNAL_DIFF_MAX_CHARS + 1)
        .collect();
    let new_chars: Vec<_> = new
        .char_indices()
        .take(INTERNAL_DIFF_MAX_CHARS + 1)
        .collect();
    let n = old_chars.len();
    let m = new_chars.len();
    if n > INTERNAL_DIFF_MAX_CHARS || m > INTERNAL_DIFF_MAX_CHARS {
        return None;
    }
    let columns = m + 1;
    let cells = (n + 1) * columns;
    if cells > INTERNAL_DIFF_MAX_CELLS {
        return None;
    }
    // Suffix LCS lengths. u16 is sufficient under the scalar limit. Equal
    // scalars match immediately; ties skip an old scalar first, making repeated
    // text alignment deterministic without substring/token searches.
    let mut lengths = vec![0u16; cells];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lengths[i * columns + j] = if old_chars[i].1 == new_chars[j].1 {
                1 + lengths[(i + 1) * columns + j + 1]
            } else {
                lengths[(i + 1) * columns + j].max(lengths[i * columns + j + 1])
            };
        }
    }
    let mut spans: Vec<EqualSpan> = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if old_chars[i].1 == new_chars[j].1 {
            let (old_byte, ch) = old_chars[i];
            let new_byte = new_chars[j].0;
            if let Some(last) = spans.last_mut().filter(|last| {
                last.old.end == old_byte && last.new_start + last.old.len() == new_byte
            }) {
                last.old.end += ch.len_utf8();
            } else {
                spans.push(EqualSpan {
                    old: old_byte..old_byte + ch.len_utf8(),
                    new_start: new_byte,
                });
            }
            i += 1;
            j += 1;
        } else if lengths[(i + 1) * columns + j] >= lengths[i * columns + j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    Some(spans)
}

struct PaintContext {
    projection: Rc<Projection>,
    ages: Vec<Age>,
    offset: Option<usize>,
    aliases: HashMap<usize, usize>,
    now: Instant,
    visible: Rc<Cell<Option<Instant>>>,
}
thread_local! { static PAINT: RefCell<Option<PaintContext>> = const { RefCell::new(None) }; }
/// Synchronous, nest-safe paint scope; no pane or shaper references are held.
pub struct PaintScope(Option<PaintContext>);
impl Drop for PaintScope {
    fn drop(&mut self) {
        PAINT.with(|p| {
            p.replace(self.0.take());
        });
    }
}
pub fn begin_line(line: &str) {
    begin_line_at(line, 0);
}
pub fn begin_line_at(source: &str, canonical_offset: usize) {
    PAINT.with(|p| {
        if let Some(p) = p.borrow_mut().as_mut() {
            let key = source.as_ptr() as usize;
            p.offset = p
                .aliases
                .get(&key)
                .or_else(|| p.projection.lines.get(&key))
                .map(|base| base + canonical_offset);
        }
    });
}
pub fn alias_line(source: &str, line: &str, offset: usize) {
    PAINT.with(|p| {
        if let Some(p) = p.borrow_mut().as_mut() {
            if let Some(base) =
                p.projection.lines.get(&(source.as_ptr() as usize)).copied()
            {
                p.aliases.clear();
                p.aliases.insert(line.as_ptr() as usize, base + offset);
            }
        }
    });
}
pub fn end_line() {
    PAINT.with(|p| {
        if let Some(p) = p.borrow_mut().as_mut() {
            p.offset = None;
        }
    });
}
pub fn ranges(text: &str) -> Vec<TextReveal> {
    PAINT.with(|p| {
        let mut slot = p.borrow_mut();
        let Some(p) = slot.as_mut() else {
            return Vec::new();
        };
        let Some(mut offset) = p.offset else {
            return Vec::new();
        };
        let mut result: Vec<TextReveal> = Vec::new();
        let run = {
            let mut cache = p.projection.runs.borrow_mut();
            let key = text.as_ptr() as usize;
            if cache.len() >= 4096 {
                cache.clear();
            }
            let entry = cache
                .entry(key)
                .or_insert_with(|| Rc::new(RunMap::new(text)));
            if entry.text != text {
                *entry = Rc::new(RunMap::new(text));
            }
            entry.clone()
        };
        for &(byte, len, relative) in &run.chars {
            let canonical = offset + relative;
            let index = p.ages.partition_point(|a| a.range.end <= canonical);
            if let Some(age) = p.ages.get(index).filter(|a| a.range.contains(&canonical))
            {
                let t = (p.now.saturating_duration_since(age.at).as_secs_f32()
                    / REVEAL_DURATION.as_secs_f32())
                .clamp(0.0, 1.0);
                let opacity = 1.0 - (1.0 - t).powi(3);
                if let Some(last) = result
                    .last_mut()
                    .filter(|r| r.range.end == byte && r.opacity == opacity)
                {
                    last.range.end += len;
                } else {
                    result.push(TextReveal {
                        range: byte..byte + len,
                        opacity,
                        blur_radius: 4.0 * (1.0 - t),
                    });
                }
            }
        }
        offset += run.canonical_len;
        p.offset = Some(offset);
        result
    })
}
pub fn painted_visible(reveals: &[TextReveal]) {
    PAINT.with(|p| {
        if let Some(p) = p.borrow().as_ref() {
            if let Some(end) = p
                .ages
                .iter()
                .filter(|a| {
                    let t = (p.now.saturating_duration_since(a.at).as_secs_f32()
                        / REVEAL_DURATION.as_secs_f32())
                    .clamp(0.0, 1.0);
                    let opacity = 1.0 - (1.0 - t).powi(3);
                    reveals.iter().any(|r| r.opacity == opacity)
                })
                .map(|a| a.at + REVEAL_DURATION)
                .max()
            {
                p.visible
                    .set(Some(p.visible.get().map_or(end, |old| old.max(end))));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    fn projection(text: &str) -> Projection {
        let mut p = Projection::default();
        p.push(text, text);
        p
    }
    #[test]
    fn history_baseline_is_sharp_and_identical_live_snapshot_does_not_replay() {
        let now = Instant::now();
        let mut state = TextRevealState::default();
        state.record_at("row", "history", "history", now);
        assert!(state.bodies.is_empty());
        state.record_at("row", "history", "history!", now);
        state.apply_projection("row", &projection("history"), None);
        state.apply_projection("row", &projection("history!"), Some(now));
        assert_eq!(state.bodies["row"].ages[0].range, 7..8);
        state.record_at(
            "row",
            "history!",
            "history!",
            now + Duration::from_millis(50),
        );
        assert_eq!(state.bodies["row"].latest, Some(now));
        assert_eq!(state.bodies["row"].ages[0].at, now);
    }
    #[test]
    fn first_live_contents_reveal_but_idless_rows_do_not() {
        let now = Instant::now();
        let mut state = TextRevealState::default();
        state.record_at("", "", "skip", now);
        assert!(state.bodies.is_empty());
        state.record_at("row", "", "hello", now);
        state.apply_projection("row", &projection(""), None);
        state.apply_projection("row", &projection("hello"), Some(now));
        assert_eq!(state.bodies["row"].ages[0].range, 0..5);
    }
    #[test]
    fn replacement_preserves_unchanged_suffix_ages() {
        let now = Instant::now();
        let mut old = "abc-tail".to_owned();
        let mut ages = vec![Age {
            range: 4..8,
            at: now,
        }];
        update_identity(
            &mut old,
            &mut ages,
            "XYZ-tail",
            now + Duration::from_millis(20),
        );
        assert!(ages.iter().any(|a| a.range == (4..8) && a.at == now));
        assert!(ages.iter().any(|a| a.range == (0..3) && a.at != now));
    }
    #[test]
    fn markdown_completion_removes_delimiters_without_reanimating_suffix() {
        let now = Instant::now();
        let mut old = "**hello".to_owned();
        let mut ages = Vec::new();
        update_identity(&mut old, &mut ages, "hello", now);
        assert!(ages.is_empty());
    }
    #[test]
    fn unicode_replacements_use_character_boundaries_not_byte_prefixes() {
        let now = Instant::now();
        let mut old = "é猫🙂".to_owned();
        let mut ages = Vec::new();
        update_identity(&mut old, &mut ages, "ê犬🙂", now);
        assert_eq!(ages[0].range, 0..5);
        assert!(old.is_char_boundary(ages[0].range.end));
        assert_eq!(&old[ages[0].range.clone()], "ê犬");
    }
    #[test]
    fn repeated_words_are_positional_not_substring_matches() {
        let now = Instant::now();
        let mut old = "samesamesame".to_owned();
        let mut ages = Vec::new();
        update_identity(&mut old, &mut ages, "sameNEWsame", now);
        assert_eq!(ages[0].range, 4..7);
        let mut p = Projection::default();
        let first = "same".to_owned();
        let second = "NEW".to_owned();
        let third = "same".to_owned();
        p.push(&first, &first);
        p.push(&second, &second);
        p.push(&third, &third);
        assert_eq!(p.lines[&(third.as_ptr() as usize)], 7);
    }
    #[test]
    fn reflow_and_whitespace_changes_have_same_canonical_identity() {
        let now = Instant::now();
        let mut old = projection("a b\nc é").canonical;
        let mut ages = Vec::new();
        update_identity(&mut old, &mut ages, &projection("ab c\né").canonical, now);
        assert!(ages.is_empty());
    }
    #[test]
    fn offscreen_pending_updates_expire_without_ever_being_painted() {
        let now = Instant::now();
        let mut state = TextRevealState::default();
        state.record_at("row", "", "unseen", now);
        assert!(!state.is_animating());
        state.prune(now + REVEAL_DURATION);
        assert!(state.bodies.is_empty());
    }
    #[test]
    fn session_switch_and_disable_clear_all_animation_state() {
        let now = Instant::now();
        let mut state = TextRevealState::default();
        state.scope(Some("a"));
        state.record_at("row", "", "live", now);
        state.visible_until.set(Some(now + REVEAL_DURATION));
        assert!(!state.is_animating_for(Some("b")));
        state.scope(Some("b"));
        assert!(state.bodies.is_empty());
        state.record_at("row", "", "live", now);
        state.set_enabled(false);
        assert!(state.bodies.is_empty());
        assert!(!state.is_animating());
        state.record_at("row", "", "disabled", now);
        assert!(state.bodies.is_empty());
    }
    #[test]
    fn run_ranges_are_rebased_utf8_and_only_visible_paint_owns_redraw() {
        let now = Instant::now();
        let mut state = TextRevealState::default();
        state.record_at("row", "sharp", "sharp猫🙂", now);
        state.apply_projection("row", &projection("sharp"), None);
        state.apply_projection("row", &projection("sharp猫🙂"), Some(now));
        let line = "sharp猫🙂";
        let p = Rc::new(projection(line));
        let _scope = state.paint("row", 1, p);
        begin_line(line);
        assert!(ranges("sharp").is_empty());
        let active = ranges("猫🙂");
        assert_eq!(active[0].range, 0..7);
        assert!(!state.is_animating());
        painted_visible(&active);
        assert!(state.is_animating());
        state.begin_frame();
        assert!(!state.is_animating());
    }
    #[test]
    fn history_reconciliation_keeps_identical_live_rows_but_not_replaced_history() {
        let now = Instant::now();
        let mut state = TextRevealState::default();
        state.record_at("row", "", "live", now);
        state.reconcile_history(std::iter::once(("row", "live")));
        assert!(state.has_body("row"));
        state.reconcile_history(std::iter::once(("row", "history")));
        assert!(!state.has_body("row"));
    }
    #[test]
    fn coalesced_updates_preserve_previous_paint_ages_and_use_latest_arrival() {
        let now = Instant::now();
        let later = now + Duration::from_millis(10);
        let latest = now + Duration::from_millis(20);
        let mut state = TextRevealState::default();
        state.record_at("row", "old", "oldA", now);
        for u in state.pending("row") {
            state.apply_projection("row", &projection(&u.text), u.at);
        }
        state.record_at("row", "oldA", "oldAB", later);
        state.record_at("row", "oldAB", "oldABC", latest);
        let updates = state.pending("row");
        // The previously painted canonical identity is the baseline. No old
        // source snapshot or intermediate B-only projection is needed.
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].text, "oldABC");
        assert_eq!(updates[0].at, Some(latest));
        state.apply_projection("row", &projection("oldABC"), updates[0].at);
        assert!(state.bodies["row"]
            .ages
            .iter()
            .any(|a| a.range == (3..4) && a.at == now));
        assert!(state.bodies["row"]
            .ages
            .iter()
            .any(|a| a.range == (4..6) && a.at == latest));
    }
    #[test]
    fn width_only_projection_change_preserves_ages_and_maps_offscreen_lines() {
        let now = Instant::now();
        let mut state = TextRevealState::default();
        state.record_at("row", "", "same same", now);
        state.apply_projection("row", &projection(""), None);
        state.apply_projection("row", &projection("same same"), Some(now));
        let first = String::from("same");
        let offscreen = String::from("same");
        let mut reflow = Projection::default();
        reflow.push(&first, &first);
        reflow.push(&offscreen, &offscreen);
        let _scope = state.paint("row", 2, Rc::new(reflow));
        assert_eq!(state.bodies["row"].ages[0].at, now);
        begin_line(&offscreen);
        let r = ranges(&offscreen);
        assert_eq!(r[0].range, 0..4);
        assert!(!state.is_animating());
    }
    #[test]
    fn temporary_table_lines_use_cell_position_not_repeated_word_matching() {
        let now = Instant::now();
        let mut state = TextRevealState::default();
        state.record_at("row", "same same", "same NEW", now);
        state.apply_projection("row", &projection("same same"), None);
        state.apply_projection("row", &projection("same NEW"), Some(now));
        let cell = String::from("same\nNEW");
        let p = Rc::new(projection(&cell));
        let _scope = state.paint("row", 1, p);
        let first = String::from("same");
        let second = String::from("NEW");
        alias_line(&cell, &first, 0);
        begin_line(&first);
        assert!(ranges(&first).is_empty());
        alias_line(&cell, &second, 4);
        begin_line(&second);
        assert_eq!(ranges(&second)[0].range, 0..3);
    }
    #[test]
    fn hundreds_of_updates_keep_only_initial_history_and_one_final_live_projection() {
        let now = Instant::now();
        let mut state = TextRevealState::default();
        for n in 0..300 {
            state.record_at(
                "row",
                &format!("history{}", "x".repeat(n)),
                &format!("history{}", "x".repeat(n + 1)),
                now + Duration::from_micros(n as u64 + 1),
            );
            assert!(state.bodies["row"].pending.len() <= 2);
        }
        let updates = state.pending("row");
        assert_eq!(updates.len(), 2);
        assert_eq!(updates[0].text, "history");
        assert!(updates[0].at.is_none());
        let final_text = format!("history{}", "x".repeat(300));
        let latest = now + Duration::from_micros(300);
        assert_eq!(updates[1].text, final_text);
        assert_eq!(updates[1].at, Some(latest));
        // Mirrors painting: initialize history once, then project ONLY the
        // already-laid-out final display, applying one live identity diff.
        state.apply_projection("row", &projection(&updates[0].text), None);
        let final_display = projection(&final_text);
        let mut live_applications = 0;
        for update in updates.iter().filter(|u| u.at.is_some()) {
            state.apply_projection("row", &final_display, update.at);
            live_applications += 1;
        }
        assert_eq!(live_applications, 1);
        assert_eq!(state.bodies["row"].canonical, final_text);
        assert_eq!(state.bodies["row"].ages.len(), 1);
        assert_eq!(state.bodies["row"].ages[0].range, 7..307);
        assert_eq!(state.bodies["row"].ages[0].at, latest);
        state.record_at(
            "row",
            &final_text,
            &final_text,
            now + Duration::from_millis(10),
        );
        assert!(state.pending("row").is_empty());
        assert_eq!(state.bodies["row"].latest, Some(latest));
        assert_eq!(state.bodies["row"].ages[0].at, latest);
    }
    #[test]
    fn removing_multiple_interior_markers_keeps_existing_letters_sharp() {
        let now = Instant::now();
        for (before, after) in [("|x|y|", "xy"), ("**alpha**beta**", "alphabeta")] {
            let mut old = before.to_owned();
            let mut ages = Vec::new();
            update_identity(&mut old, &mut ages, after, now);
            assert!(
                ages.is_empty(),
                "existing letters reanimated: {before} -> {after}"
            );
            assert_eq!(old, after);
        }
    }
    #[test]
    fn insertion_during_marker_completion_reveals_only_added_unicode_scalars() {
        let now = Instant::now();
        let mut old = "|é|猫|".to_owned();
        let mut ages = Vec::new();
        update_identity(&mut old, &mut ages, "é🙂猫", now);
        assert_eq!(ages.len(), 1);
        assert_eq!(ages[0].range, 2..6);
        assert_eq!(&old[ages[0].range.clone()], "🙂");
        assert_eq!(ages[0].at, now);
    }
    #[test]
    fn internal_equal_subsequences_preserve_individual_active_ages() {
        let now = Instant::now();
        let x_at = now - Duration::from_millis(20);
        let y_at = now - Duration::from_millis(40);
        let mut old = "|x|y|".to_owned();
        let mut ages = vec![
            Age {
                range: 1..2,
                at: x_at,
            },
            Age {
                range: 3..4,
                at: y_at,
            },
        ];
        update_identity(&mut old, &mut ages, "xzy", now);
        assert_eq!(ages.len(), 3);
        assert!(ages.iter().any(|a| a.range == (0..1) && a.at == x_at));
        assert!(ages.iter().any(|a| a.range == (1..2) && a.at == now));
        assert!(ages.iter().any(|a| a.range == (2..3) && a.at == y_at));
    }
    #[test]
    fn internal_matching_does_not_revive_expired_equal_text() {
        let now = Instant::now();
        let mut old = "|x|y|".to_owned();
        let mut ages = vec![Age {
            range: 0..5,
            at: now - REVEAL_DURATION,
        }];
        update_identity(&mut old, &mut ages, "xzy", now);
        assert_eq!(ages.len(), 1);
        assert_eq!(ages[0].range, 1..2);
        assert_eq!(ages[0].at, now);
    }
    #[test]
    fn retry_replacement_only_inherits_age_for_equal_unicode_subsequences() {
        let now = Instant::now();
        let earlier = now - Duration::from_millis(30);
        let mut old = "AB猫CD🙂".to_owned();
        let mut ages = vec![Age {
            range: 0..old.len(),
            at: earlier,
        }];
        update_identity(&mut old, &mut ages, "XY猫ZW🙂", now);
        let preserved: Vec<_> = ages
            .iter()
            .filter(|a| a.at == earlier)
            .map(|a| &old[a.range.clone()])
            .collect();
        let fresh: Vec<_> = ages
            .iter()
            .filter(|a| a.at == now)
            .map(|a| &old[a.range.clone()])
            .collect();
        assert_eq!(preserved, vec!["猫", "🙂"]);
        assert_eq!(fresh, vec!["XY", "ZW"]);
        update_identity(
            &mut old,
            &mut ages,
            "12345",
            now + Duration::from_millis(10),
        );
        assert_eq!(ages.len(), 1);
        assert_eq!(ages[0].range, 0..5);
        assert_eq!(ages[0].at, now + Duration::from_millis(10));
    }
    #[test]
    fn repeated_characters_have_deterministic_delete_old_first_alignment() {
        let now = Instant::now();
        let earlier = now - Duration::from_millis(10);
        for _ in 0..3 {
            let mut old = "ABA".to_owned();
            let mut ages = vec![Age {
                range: 1..2,
                at: earlier,
            }];
            update_identity(&mut old, &mut ages, "BAB", now);
            assert_eq!(ages.len(), 2);
            assert_eq!(ages[0].range, 0..1); // Old B, not an arbitrary token find.
            assert_eq!(ages[0].at, earlier);
            assert_eq!(ages[1].range, 2..3); // Only the new final B is inserted.
            assert_eq!(ages[1].at, now);
        }
    }
    #[test]
    fn internal_diff_limits_fall_back_conservatively() {
        assert!(bounded_equal_spans(&"猫".repeat(513), "猫").is_none());
        assert!(bounded_equal_spans(&"猫".repeat(10_000), "猫").is_none());
        // Exactly the matrix budget is allowed; exceeding it is rejected.
        assert!(bounded_equal_spans(&"a".repeat(255), &"b".repeat(255)).is_some());
        let before = format!("{}x", "|".repeat(255));
        let after = format!("x{}", "y".repeat(255));
        assert!(bounded_equal_spans(&before, &after).is_none());
        let now = Instant::now();
        let mut old = before;
        let mut ages = Vec::new();
        update_identity(&mut old, &mut ages, &after, now);
        assert_eq!(ages.len(), 1);
        assert_eq!(ages[0].range, 0..after.len());
        assert_eq!(ages[0].at, now); // No guessed internal provenance on fallback.
    }
    #[test]
    fn large_append_and_deletion_preserve_fast_path_ages() {
        let now = Instant::now();
        let earlier = now - Duration::from_millis(10);
        let baseline = "猫".repeat(10_000);
        let mut old = baseline.clone();
        let mut ages = vec![Age {
            range: 0..3,
            at: earlier,
        }];
        let appended = format!("{baseline}🙂");
        update_identity(&mut old, &mut ages, &appended, now);
        assert!(ages.iter().any(|a| a.range == (0..3) && a.at == earlier));
        assert!(ages
            .iter()
            .any(|a| a.range == (baseline.len()..appended.len()) && a.at == now));
        update_identity(
            &mut old,
            &mut ages,
            &baseline,
            now + Duration::from_millis(1),
        );
        assert_eq!(ages.len(), 1);
        assert_eq!(ages[0].range, 0..3);
        assert_eq!(ages[0].at, earlier);
    }
    #[test]
    fn exhaustive_small_unicode_diff_maps_only_equal_ordered_subsequences() {
        fn lcs(a: &[char], b: &[char]) -> usize {
            match (a.first(), b.first()) {
                (Some(x), Some(y)) if x == y => 1 + lcs(&a[1..], &b[1..]),
                (Some(_), Some(_)) => lcs(&a[1..], b).max(lcs(a, &b[1..])),
                _ => 0,
            }
        }
        let mut samples = vec![String::new()];
        let mut frontier = samples.clone();
        for _ in 0..3 {
            frontier = frontier
                .iter()
                .flat_map(|prefix| {
                    ['é', '猫', '|']
                        .into_iter()
                        .map(move |ch| format!("{prefix}{ch}"))
                })
                .collect();
            samples.extend(frontier.clone());
        }
        let now = Instant::now();
        for before in &samples {
            for after in &samples {
                let spans = bounded_equal_spans(before, after).unwrap();
                let (mut old_end, mut new_end) = (0, 0);
                let mut matched = 0;
                for span in spans {
                    let end = span.new_start + span.old.len();
                    assert!(span.old.start >= old_end && span.new_start >= new_end);
                    assert_eq!(&before[span.old.clone()], &after[span.new_start..end]);
                    matched += before[span.old.clone()].chars().count();
                    old_end = span.old.end;
                    new_end = end;
                }
                let a: Vec<_> = before.chars().collect();
                let b: Vec<_> = after.chars().collect();
                assert_eq!(matched, lcs(&a, &b));
                let mut old = before.clone();
                let mut ages = Vec::new();
                update_identity(&mut old, &mut ages, after, now);
                let inserted: usize = ages
                    .iter()
                    .map(|age| after[age.range.clone()].chars().count())
                    .sum();
                assert_eq!(inserted, b.len() - matched);
            }
        }
    }
}
