//! Regression tests for parse-time terminal query replies.
//!
//! The gh-auth-login bug: replies to OSC 10/11 color queries and
//! DSR 6 (CPR) were deferred through the host event loop, so a
//! querier like termenv/gh had already left its raw-mode read window
//! by the time the bytes hit the PTY — the kernel echoed them as
//! `^[]11;rgb:…^[\^[[4;1R` junk and gh's prompt library aborted on
//! the stray `ESC ]` left in stdin.
//!
//! These tests pin the corrected contract at the core level: with a
//! host-seeded default palette, a color query produces its reply as a
//! `TerminalEffect::PtyWrite` DURING `Processor::advance` (the host
//! writes those bytes straight to the PTY child), exactly once per
//! query, in parse order, with the reply terminator matching the
//! query's (BEL → BEL, ST → ST) — and the reply bytes never appear in
//! the grid.

use neoism_terminal_core::ansi::CursorShape;
use neoism_terminal_core::colors::term::TermColors;
use neoism_terminal_core::colors::{ColorRgb, NamedColor};
use neoism_terminal_core::handler::Processor;
use neoism_terminal_core::{Crosswords, TerminalEffect, TerminalId};

fn new_terminal() -> Crosswords {
    Crosswords::new((24usize, 80usize), CursorShape::Block, TerminalId::new(0), 100)
}

/// Seed fg/bg (and palette slot 5) the way a host seeds its resolved
/// theme. Background is the color from the original bug screenshot
/// (`rgb:0f0f/0d0d/0e0e` — #0f0d0e).
fn seed_theme(term: &mut Crosswords) {
    let mut defaults = TermColors::default();
    defaults[NamedColor::Foreground as usize] = Some(
        ColorRgb {
            r: 0xd0,
            g: 0xd0,
            b: 0xd0,
        }
        .to_arr(),
    );
    defaults[NamedColor::Background as usize] = Some(
        ColorRgb {
            r: 0x0f,
            g: 0x0d,
            b: 0x0e,
        }
        .to_arr(),
    );
    defaults[5] = Some(
        ColorRgb {
            r: 0xaa,
            g: 0xbb,
            b: 0xcc,
        }
        .to_arr(),
    );
    term.set_default_colors(defaults);
}

fn feed(term: &mut Crosswords, bytes: &[u8]) -> Vec<TerminalEffect> {
    let mut parser: Processor = Processor::new();
    parser.advance(term, bytes);
    term.drain_effects().collect()
}

fn pty_writes(effects: &[TerminalEffect]) -> Vec<Vec<u8>> {
    effects
        .iter()
        .filter_map(|e| match e {
            TerminalEffect::PtyWrite(bytes) => Some(bytes.clone()),
            _ => None,
        })
        .collect()
}

fn assert_grid_untouched(term: &Crosswords) {
    let snap = term.snapshot();
    assert_eq!(snap.cursor.col, 0, "reply must not move the cursor");
    assert_eq!(snap.cursor.row, 0, "reply must not move the cursor");
    for (row_idx, row) in snap.viewport.iter().enumerate() {
        for (col_idx, cell) in row.iter().enumerate() {
            assert!(
                cell.c == '\0' || cell.c == ' ',
                "reply bytes leaked into the grid at row {row_idx} col {col_idx}: {:?}",
                cell.c
            );
        }
    }
}

#[test]
fn osc11_st_query_gets_immediate_st_reply_and_clean_grid() {
    let mut term = new_terminal();
    seed_theme(&mut term);

    let effects = feed(&mut term, b"\x1b]11;?\x1b\\");
    let writes = pty_writes(&effects);

    assert_eq!(
        writes,
        vec![b"\x1b]11;rgb:0f0f/0d0d/0e0e\x1b\\".to_vec()],
        "exactly one reply, emitted during the same advance() as the query"
    );
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, TerminalEffect::ColorRequest { .. })),
        "an answered query must not ALSO raise the deferred ColorRequest"
    );
    assert_grid_untouched(&term);
}

#[test]
fn bel_terminated_query_gets_bel_terminated_reply() {
    let mut term = new_terminal();
    seed_theme(&mut term);

    let writes = pty_writes(&feed(&mut term, b"\x1b]11;?\x07"));
    assert_eq!(writes, vec![b"\x1b]11;rgb:0f0f/0d0d/0e0e\x07".to_vec()]);
}

#[test]
fn two_queries_get_two_replies_in_parse_order() {
    let mut term = new_terminal();
    seed_theme(&mut term);

    let writes = pty_writes(&feed(&mut term, b"\x1b]11;?\x1b\\\x1b]10;?\x1b\\"));
    assert_eq!(
        writes,
        vec![
            b"\x1b]11;rgb:0f0f/0d0d/0e0e\x1b\\".to_vec(),
            b"\x1b]10;rgb:d0d0/d0d0/d0d0\x1b\\".to_vec(),
        ]
    );
}

/// The exact termenv/gh handshake: OSC 11 query immediately followed
/// by DSR 6. Both replies must come out of the same parse pass, in
/// order, so the querier's single raw-mode read window sees both.
#[test]
fn termenv_osc11_plus_dsr6_pair_replies_in_order() {
    let mut term = new_terminal();
    seed_theme(&mut term);

    let effects = feed(&mut term, b"\x1b]11;?\x1b\\\x1b[6n");
    let writes = pty_writes(&effects);

    assert_eq!(
        writes,
        vec![
            b"\x1b]11;rgb:0f0f/0d0d/0e0e\x1b\\".to_vec(),
            b"\x1b[1;1R".to_vec(),
        ]
    );
    assert_grid_untouched(&term);
}

#[test]
fn guest_osc_override_beats_seeded_default() {
    let mut term = new_terminal();
    seed_theme(&mut term);

    // Guest sets its own background, then queries it back.
    let effects = feed(&mut term, b"\x1b]11;#102030\x07\x1b]11;?\x07");
    let writes = pty_writes(&effects);
    assert_eq!(writes, vec![b"\x1b]11;rgb:1010/2020/3030\x07".to_vec()]);
}

#[test]
fn osc4_palette_query_replies_from_seed_with_index_prefix() {
    let mut term = new_terminal();
    seed_theme(&mut term);

    let writes = pty_writes(&feed(&mut term, b"\x1b]4;5;?\x1b\\"));
    assert_eq!(writes, vec![b"\x1b]4;5;rgb:aaaa/bbbb/cccc\x1b\\".to_vec()]);
}

#[test]
fn out_of_range_osc4_query_is_ignored_without_panic() {
    let mut term = new_terminal();
    seed_theme(&mut term);

    let effects = feed(&mut term, b"\x1b]4;9999;?\x1b\\");
    assert!(
        pty_writes(&effects).is_empty(),
        "no reply for an out-of-range palette slot"
    );
    assert!(!effects
        .iter()
        .any(|e| matches!(e, TerminalEffect::ColorRequest { .. })));
}

/// Un-seeded hosts keep the historical deferred behavior: the query
/// surfaces as a ColorRequest effect (event-loop fallback), never as
/// a fabricated PtyWrite.
#[test]
fn unseeded_slot_falls_back_to_deferred_color_request() {
    let mut term = new_terminal();

    let effects = feed(&mut term, b"\x1b]11;?\x1b\\");
    assert!(pty_writes(&effects).is_empty());
    match effects
        .iter()
        .find(|e| matches!(e, TerminalEffect::ColorRequest { .. }))
    {
        Some(TerminalEffect::ColorRequest {
            prefix,
            index,
            terminator,
        }) => {
            assert_eq!(prefix, "11");
            assert_eq!(*index, NamedColor::Background as usize);
            assert_eq!(terminator, "\x1b\\");
        }
        _ => panic!("expected deferred ColorRequest for un-seeded slot"),
    }
}

/// OSC 12 (cursor color) with neither a guest override nor a seeded
/// default stays unanswered-at-parse-time (legacy deferred effect);
/// with a guest override it replies like any other slot.
#[test]
fn cursor_query_replies_only_with_override_when_cursor_not_seeded() {
    let mut term = new_terminal();
    seed_theme(&mut term); // seed_theme leaves Cursor unseeded

    let effects = feed(&mut term, b"\x1b]12;?\x07");
    assert!(pty_writes(&effects).is_empty());
    assert!(effects
        .iter()
        .any(|e| matches!(e, TerminalEffect::ColorRequest { .. })));

    let effects = feed(&mut term, b"\x1b]12;#ff0000\x07\x1b]12;?\x07");
    assert_eq!(
        pty_writes(&effects),
        vec![b"\x1b]12;rgb:ffff/0000/0000\x07".to_vec()]
    );
}
