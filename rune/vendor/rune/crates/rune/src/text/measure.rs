//! Measuring text from a view, for views that place things themselves (table columns, diagram
//! nodes). A view has no handle on the runtime's text system, so each thread keeps one of its own
//! built from the fonts the most recent [`TextSystem`] was given: the app's `AppSpec::fonts` in
//! an app, the built-in Inter in a test runtime. Same shaper, same fonts, same numbers.

use std::cell::RefCell;
use std::marker::PhantomData;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use super::{RichLayoutOptions, Span, TextSystem};

mod ink;
pub use ink::{TextInk, ink};
use crate::tok::TextStyle;

struct Fonts {
    /// Bumped by any change, of fonts or families.
    generation: u64,
    /// The `generation` that last changed the fonts.
    fonts_generation: u64,
    fonts: Vec<&'static [u8]>,
    families: (String, String),
    /// Zero for the process-wide state; a [`with_families`] scope's own number otherwise.
    scope: u64,
}

static FONTS: Mutex<Option<Fonts>> = Mutex::new(None);
static SCOPES: AtomicU64 = AtomicU64::new(1);

/// Layouts kept between evictions (each eviction keeps those used since the one before).
const EVICT_EVERY: u32 = 512;

struct Measurer {
    generation: u64,
    fonts_generation: u64,
    text: TextSystem,
    calls: u32,
}

thread_local! {
    static MEASURER: RefCell<Option<Measurer>> = const { RefCell::new(None) };
    /// This thread's [`with_families`] state: a private copy of `FONTS` that notes land in.
    static OVERRIDE: RefCell<Option<Fonts>> = const { RefCell::new(None) };
}

/// Runs `f` on the state this thread measures with: its override when a [`with_families`]
/// guard is alive, else the process-wide one.
fn with_state<R>(f: impl FnOnce(&mut Option<Fonts>) -> R) -> R {
    let scoped = OVERRIDE.with_borrow_mut(Option::take);
    match scoped {
        Some(fonts) => {
            let mut slot = Some(fonts);
            let r = f(&mut slot);
            OVERRIDE.with_borrow_mut(|o| *o = slot);
            r
        }
        None => match FONTS.lock() {
            Ok(mut slot) => f(&mut slot),
            Err(_) => f(&mut None),
        },
    }
}

fn apply_fonts(slot: &mut Option<Fonts>, fonts: &[&'static [u8]], families: (&str, &str)) {
    let same_fonts = slot.as_ref().is_some_and(|f| f.fonts == fonts);
    if same_fonts && slot.as_ref().is_some_and(|f| f.families.0 == families.0 && f.families.1 == families.1) {
        return;
    }
    let generation = slot.as_ref().map_or(1, |f| f.generation + 1);
    let fonts_generation = match slot.as_ref() { Some(f) if same_fonts => f.fonts_generation, _ => generation };
    let scope = slot.as_ref().map_or(0, |f| f.scope);
    let families = (families.0.to_owned(), families.1.to_owned());
    *slot = Some(Fonts { generation, fonts_generation, fonts: fonts.to_vec(), families, scope });
}

/// Called by every [`TextSystem::new`]; the next measurement uses these fonts.
pub(super) fn note_fonts(fonts: &[&'static [u8]], families: (&str, &str)) {
    with_state(|slot| apply_fonts(slot, fonts, families));
}

/// Called when a web app's full faces arrive: the next measurement shapes with them.
#[cfg(all(target_arch = "wasm32", feature = "embedded-fonts"))]
pub(super) fn note_faces_arrived() {
    with_state(|slot| {
        if let Some(f) = slot.as_mut() {
            f.generation += 1;
            f.fonts_generation = f.generation;
        }
    });
}

/// Called by [`TextSystem::set_families`]; the next measurement uses these families.
pub(super) fn note_families(sans: &str, mono: &str) {
    with_state(|slot| {
        let Some(f) = slot.as_mut() else { return };
        if f.families.0 != sans || f.families.1 != mono {
            f.generation += 1;
            f.families = (sans.to_owned(), mono.to_owned());
        }
    });
}

/// Changes whenever the fonts or families [`measure`] uses change, as when an app calls
/// `Tx::set_font_families`. A cache of measured sizes mixes it into its key.
///
/// Under a [`with_families`] guard it is a different number from the one outside, so a cache
/// keyed on it never mixes the scoped families with the others.
pub fn font_epoch() -> u64 {
    with_state(|f| f.as_ref().map(epoch_of)).unwrap_or(0)
}

fn epoch_of(f: &Fonts) -> u64 {
    if f.scope == 0 { f.generation } else { 1 << 63 | f.scope << 32 | (f.generation & 0xFFFF_FFFF) }
}

/// Holds a thread-scoped family override; see [`with_families`].
#[must_use = "the override ends when the guard is dropped"]
pub struct FamiliesGuard {
    previous: Option<Fonts>,
    /// The override lives on one thread, so the guard must drop there.
    _not_send: PhantomData<*const ()>,
}

impl Drop for FamiliesGuard {
    fn drop(&mut self) {
        let previous = self.previous.take();
        OVERRIDE.with_borrow_mut(|o| *o = previous);
    }
}

/// Makes [`measure`], [`advance`], [`ink`] and [`font_epoch`] on this thread use `sans` and
/// `mono` (CSS family lists, as `Tx::set_font_families` takes them) for as long as the returned
/// guard lives, whatever the rest of the process has chosen. It is for tests: ones that switch
/// families run in parallel threads without seeing each other's.
///
/// ```ignore
/// let _families = rune::text::with_families("Inter", "IBM Plex Mono");
/// let (w, _) = rune::text::measure("let x = 1;", &[], style.mono(), None);
/// ```
///
/// - The fonts are the process's current ones; only the families are replaced.
/// - Dropping the guard restores what was in force before it, so guards nest. Drop them in the
///   reverse order they were made.
/// - While it lives, anything on this thread that would change what `measure` uses lands in the
///   override and nowhere else: a headless window's `Tx::set_font_families` (or a
///   `TextSystem::new` or `set_families`) moves this thread's `measure` and `font_epoch`, and
///   no other thread's. The window itself follows the families chosen for windows as it
///   always does. Outside a guard, one switch still moves every thread.
/// - `font_epoch` differs from the unscoped epoch, and from that of every other guard, so
///   caches keyed on it stay apart.
///
/// Prefer a guard over a closure so a test can hold it for its whole body, and it restores on
/// a panic.
pub fn with_families(sans: &str, mono: &str) -> FamiliesGuard {
    // Make sure there is state to copy: a thread that never measured has none yet.
    if with_state(|f| f.is_none()) {
        with_text(|_| {});
    }
    let scope = SCOPES.fetch_add(1, Ordering::Relaxed);
    let seeded = with_state(|f| {
        f.as_ref().map(|f| Fonts {
            generation: f.generation + 1,
            fonts_generation: f.fonts_generation,
            fonts: f.fonts.clone(),
            families: (sans.to_owned(), mono.to_owned()),
            scope,
        })
    });
    let previous = OVERRIDE.with_borrow_mut(|o| std::mem::replace(o, seeded));
    FamiliesGuard { previous, _not_send: PhantomData }
}

/// The size in points of `text` (with `spans`, as [`crate::rich`] takes them) set in `style`,
/// wrapped at `max_width` points when given, as the runtime will lay it out. Widths round up
/// to whole points; heights are the lines' own, as a text lays out unless it asks for
/// [`El::rounded_lines`](crate::El::rounded_lines). `max_width: Some(0.0)` gives the widest unbreakable run (CSS min-content).
///
/// The first call on a thread loads the fonts (a few milliseconds); later calls cost a hash
/// lookup when the same text was measured recently, or one shaping pass when not.
pub fn measure(text: &str, spans: &[Span], style: TextStyle, max_width: Option<f32>) -> (f32, f32) {
    measure_with(text, spans, style, max_width, RichLayoutOptions::default())
}

/// [`measure`] laid out with [`RichLayoutOptions`], so text that an [`El`](crate::El) draws with
/// [`wrap_anywhere`](crate::El::wrap_anywhere) measures to the lines it will actually take:
/// `measure_with(text, &[], style, Some(w), RichLayoutOptions::new().wrap_anywhere())`.
pub fn measure_with(
    text: &str,
    spans: &[Span],
    style: TextStyle,
    max_width: Option<f32>,
    options: RichLayoutOptions,
) -> (f32, f32) {
    with_text(|t| {
        let l = t.layout_rich_with(text, spans, style, 1.0, max_width, options);
        // Spaces hanging past a wrapped line's end take no room, as the runtime measures them.
        let w = if max_width.is_some() { l.width() } else { l.full_width() };
        (w.ceil(), super::line_metrics::spec_height(&l, style, 1.0, true))
    })
}

/// CSS's min-content and max-content widths of `text` (with `spans`) in `style`, in points and
/// unrounded, as a browser's automatic table layout reads a cell: its widest unbreakable run,
/// and its width on one line, neither counting the spaces that end a line.
pub fn content_widths(text: &str, spans: &[Span], style: TextStyle) -> (f32, f32) {
    with_text(|t| {
        let min = t.layout_rich_with(text, spans, style, 1.0, Some(0.0), RichLayoutOptions::default()).width();
        let max = t.layout_rich_with(text, spans, style, 1.0, None, RichLayoutOptions::default()).width();
        (min, max)
    })
}

/// The advance in points of `text` set on one line in `style`, unrounded: how much wider a row
/// of [`El::advance_in`](crate::El::advance_in) cells gets for it, to within a device pixel.
pub fn advance(text: &str, style: TextStyle) -> f32 {
    with_text(|t| t.layout_rich(text, &[], style, 1.0, None).full_width())
}

/// Runs `f` on this thread's text system, built from the fonts the app was given.
pub(super) fn with_text<R>(f: impl FnOnce(&mut TextSystem) -> R) -> R {
    MEASURER.with_borrow_mut(|m| {
        let current = font_epoch();
        if m.as_ref().is_none_or(|m| m.generation != current) {
            let wanted = with_state(|f| f.as_ref().map(|f| (f.fonts_generation, f.fonts.clone(), f.families.clone())));
            match (m.as_mut(), &wanted) {
                // Only the families changed: the loaded fonts stay.
                (Some(m), Some((fonts_generation, _, (sans, mono)))) if m.fonts_generation == *fonts_generation => {
                    m.text.set_families(sans, mono);
                    m.generation = font_epoch();
                }
                _ => {
                    let text = match &wanted {
                        Some((_, fonts, (sans, mono))) => TextSystem::new(fonts, (sans, mono)),
                        None => TextSystem::default(),
                    };
                    // Building a text system notes its fonts, so read the generation it settled on.
                    let fonts_generation = with_state(|f| f.as_ref().map(|f| f.fonts_generation)).unwrap_or(0);
                    *m = Some(Measurer { generation: font_epoch(), fonts_generation, text, calls: 0 });
                }
            }
        }
        let m = m.as_mut().expect("set above");
        m.calls += 1;
        if m.calls % EVICT_EVERY == 0 {
            m.text.end_commit();
        }
        f(&mut m.text)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_text_and_wraps() {
        let style = TextStyle::new(14.0, 400.0);
        let (w, h) = measure("Hello world", &[], style, None);
        assert!(w > 50.0 && h >= 17.0, "{w} {h}");
        let (min, tall) = measure("Hello world", &[], style, Some(0.0));
        assert!(min < w && tall > h, "min-content breaks at every opportunity: {min} {tall}");
        let mut bold = Span::new(0, 5);
        bold.weight = Some(700.0);
        assert!(measure("Hello world", &[bold], style, None).0 > w);
    }

    #[test]
    fn wrap_anywhere_measures_a_long_token_onto_more_lines() {
        let style = TextStyle::new(14.0, 400.0);
        let link = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let (plain_w, plain_h) = measure(link, &[], style, Some(120.0));
        let (w, h) = measure_with(link, &[], style, Some(120.0), RichLayoutOptions::new().wrap_anywhere());
        assert!(plain_w > 120.0, "without wrap_anywhere the token runs past the width: {plain_w}");
        assert!(w <= 120.0 && h > plain_h * 2.0, "{w} {h} vs {plain_w} {plain_h}");
        assert_eq!(measure(link, &[], style, Some(120.0)), measure_with(link, &[], style, Some(120.0), RichLayoutOptions::new()));
    }
}
