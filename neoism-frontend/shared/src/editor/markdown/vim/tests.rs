use std::path::PathBuf;

use super::super::helpers::source_from_lines;
use super::*;

fn pane(source: &str) -> MarkdownPane {
    MarkdownPane::from_source(PathBuf::from("vim-test.md"), source)
}

fn pane_at(source: &str, line: usize, col: usize) -> MarkdownPane {
    let mut p = pane(source);
    p.cursor_line = line;
    p.cursor_col = col;
    p
}

/// Feed a key sequence through the resolver + applier the way the
/// host dispatch does, with a `String` standing in for the host
/// clipboard (the unnamed register).
fn feed(pane: &mut MarkdownPane, register: &mut String, keys: &str) {
    for ch in keys.chars() {
        let visual = matches!(pane.mode, MarkdownMode::Visual);
        if let VimKeyFeed::Action(action) = pane.vim.feed(ch, visual) {
            let paste = action.wants_paste().then(|| register.clone());
            let applied = pane.apply_vim_action(&action, paste.as_deref());
            if let Some(text) = applied.register {
                *register = text;
            }
        }
    }
}

fn run(source: &str, line: usize, col: usize, keys: &str) -> MarkdownPane {
    let mut p = pane_at(source, line, col);
    let mut register = String::new();
    feed(&mut p, &mut register, keys);
    p
}

fn run_reg(source: &str, line: usize, col: usize, keys: &str) -> (MarkdownPane, String) {
    let mut p = pane_at(source, line, col);
    let mut register = String::new();
    feed(&mut p, &mut register, keys);
    (p, register)
}

fn text(pane: &MarkdownPane) -> String {
    source_from_lines(&pane.lines)
}

fn cursor(pane: &MarkdownPane) -> (usize, usize) {
    (pane.cursor_line, pane.cursor_col)
}

// -- Resolver -----------------------------------------------------------

#[test]
fn vim_resolver_accumulates_counts_around_operator() {
    let mut state = VimState::default();
    assert_eq!(state.feed('2', false), VimKeyFeed::Pending);
    assert_eq!(state.feed('d', false), VimKeyFeed::Pending);
    assert_eq!(state.feed('3', false), VimKeyFeed::Pending);
    assert_eq!(
        state.feed('w', false),
        VimKeyFeed::Action(VimAction::Operate {
            op: VimOperator::Delete,
            target: VimTarget::Motion(VimMotion::WordForward { big: false }),
            count: 6,
        })
    );
    assert!(state.pending.is_empty());
}

#[test]
fn vim_resolver_zero_is_motion_unless_count_started() {
    let mut state = VimState::default();
    assert_eq!(
        state.feed('0', false),
        VimKeyFeed::Action(VimAction::Move {
            motion: VimMotion::LineStart,
            count: 1,
        })
    );
    assert_eq!(state.feed('1', false), VimKeyFeed::Pending);
    assert_eq!(state.feed('0', false), VimKeyFeed::Pending);
    assert_eq!(
        state.feed('G', false),
        VimKeyFeed::Action(VimAction::Move {
            motion: VimMotion::GotoLine(10),
            count: 10,
        })
    );
}

#[test]
fn vim_resolver_doubled_and_mismatched_operators() {
    let mut state = VimState::default();
    state.feed('d', false);
    assert_eq!(
        state.feed('d', false),
        VimKeyFeed::Action(VimAction::Operate {
            op: VimOperator::Delete,
            target: VimTarget::Lines,
            count: 1,
        })
    );
    state.feed('d', false);
    assert_eq!(state.feed('y', false), VimKeyFeed::Cancelled);
    assert!(state.pending.is_empty());
}

#[test]
fn vim_resolver_escape_clears_pending() {
    let mut state = VimState::default();
    state.feed('2', false);
    state.feed('d', false);
    assert!(state.clear_pending());
    assert_eq!(
        state.feed('w', false),
        VimKeyFeed::Action(VimAction::Move {
            motion: VimMotion::WordForward { big: false },
            count: 1,
        })
    );
}

#[test]
fn vim_resolver_gg_and_ge() {
    let mut state = VimState::default();
    state.feed('g', false);
    assert_eq!(
        state.feed('g', false),
        VimKeyFeed::Action(VimAction::Move {
            motion: VimMotion::GotoLine(1),
            count: 1,
        })
    );
    state.feed('5', false);
    state.feed('g', false);
    assert_eq!(
        state.feed('g', false),
        VimKeyFeed::Action(VimAction::Move {
            motion: VimMotion::GotoLine(5),
            count: 5,
        })
    );
    state.feed('g', false);
    assert_eq!(
        state.feed('e', false),
        VimKeyFeed::Action(VimAction::Move {
            motion: VimMotion::WordEndBack { big: false },
            count: 1,
        })
    );
}

#[test]
fn vim_resolver_unknown_key_falls_through_only_when_idle() {
    let mut state = VimState::default();
    assert_eq!(state.feed('?', false), VimKeyFeed::Unhandled);
    state.feed('d', false);
    assert_eq!(state.feed('q', false), VimKeyFeed::Cancelled);
    assert!(state.pending.is_empty());
}

// -- Word motions ---------------------------------------------------------

#[test]
fn vim_word_forward_words_punctuation_and_unicode() {
    let p = run("foo(bar) baz", 0, 0, "w");
    assert_eq!(cursor(&p), (0, 3));
    let p = run("foo(bar) baz", 0, 3, "w");
    assert_eq!(cursor(&p), (0, 4));
    let p = run("foo(bar) baz", 0, 4, "w");
    assert_eq!(cursor(&p), (0, 7));
    let p = run("foo(bar) baz", 0, 0, "W");
    assert_eq!(cursor(&p), (0, 9));
    let p = run("héllo wörld", 0, 0, "w");
    assert_eq!(cursor(&p), (0, 7));
    let p = run("one two three four", 0, 0, "3w");
    assert_eq!(cursor(&p), (0, 14));
}

#[test]
fn vim_word_forward_crosses_lines_and_stops_on_empty_lines() {
    let p = run("one\n\ntwo", 0, 0, "w");
    assert_eq!(cursor(&p), (1, 0));
    let p = run("one\n\ntwo", 0, 0, "2w");
    assert_eq!(cursor(&p), (2, 0));
}

#[test]
fn vim_word_back_and_ends() {
    let p = run("héllo wörld", 0, 7, "b");
    assert_eq!(cursor(&p), (0, 0));
    let p = run("alpha beta", 0, 0, "e");
    assert_eq!(cursor(&p), (0, 4));
    let p = run("alpha beta", 0, 8, "ge");
    assert_eq!(cursor(&p), (0, 4));
    let p = run("foo(bar", 0, 4, "ge");
    assert_eq!(cursor(&p), (0, 3));
    let p = run("one two\nthree", 1, 0, "b");
    assert_eq!(cursor(&p), (0, 4));
}

// -- Operators + motions ----------------------------------------------------

#[test]
fn vim_dw_deletes_word_and_counts_multiply() {
    let (p, register) = run_reg("one two three", 0, 0, "dw");
    assert_eq!(text(&p), "two three");
    assert_eq!(register, "one ");
    let p = run("one two three", 0, 0, "d2w");
    assert_eq!(text(&p), "three");
    let p = run("one two three", 0, 0, "2dw");
    assert_eq!(text(&p), "three");
}

#[test]
fn vim_dw_on_last_word_stops_at_line_end() {
    let p = run("one two\nnext", 0, 4, "dw");
    assert_eq!(text(&p), "one \nnext");
}

#[test]
fn vim_cw_behaves_like_ce_on_a_word() {
    let p = run("foo bar", 0, 0, "cw");
    assert_eq!(text(&p), " bar");
    assert!(matches!(p.mode, MarkdownMode::Insert));
    assert_eq!(cursor(&p), (0, 0));
    let p = run("foo bar baz", 0, 0, "c2w");
    assert_eq!(text(&p), " baz");
}

#[test]
fn vim_doubled_operators_with_counts() {
    let (p, register) = run_reg("a\nb\nc\nd", 0, 0, "2dd");
    assert_eq!(text(&p), "c\nd");
    assert_eq!(register, "a\nb\n");
    let (_, register) = run_reg("c\nd", 0, 0, "2yy");
    assert_eq!(register, "c\nd\n");
    let p = run("a\nb", 0, 0, "cc");
    assert_eq!(text(&p), "\nb");
    assert!(matches!(p.mode, MarkdownMode::Insert));
    let p = run("only", 0, 0, "dd");
    assert_eq!(text(&p), "");
}

#[test]
fn vim_capital_shortcuts() {
    let p = run("hello", 0, 2, "D");
    assert_eq!(text(&p), "he");
    let p = run("hello", 0, 2, "C");
    assert_eq!(text(&p), "he");
    assert!(matches!(p.mode, MarkdownMode::Insert));
    let (_, register) = run_reg("hello", 0, 2, "Y");
    assert_eq!(register, "hello\n");
    let p = run("a\nb", 0, 0, "S");
    assert_eq!(text(&p), "\nb");
    assert!(matches!(p.mode, MarkdownMode::Insert));
    let p = run("hello", 0, 0, "2s");
    assert_eq!(text(&p), "llo");
    assert!(matches!(p.mode, MarkdownMode::Insert));
}

#[test]
fn vim_x_and_shift_x_are_count_aware() {
    let (p, register) = run_reg("hello", 0, 0, "3x");
    assert_eq!(text(&p), "lo");
    assert_eq!(register, "hel");
    let p = run("hello", 0, 2, "X");
    assert_eq!(text(&p), "hllo");
    assert_eq!(cursor(&p), (0, 1));
    // x never joins lines.
    let p = run("a\nb", 0, 1, "x");
    assert_eq!(text(&p), "a\nb");
}

#[test]
fn vim_replace_char_count_aware() {
    let p = run("hello", 0, 0, "rx");
    assert_eq!(text(&p), "xello");
    assert_eq!(cursor(&p), (0, 0));
    let p = run("hello", 0, 0, "3rz");
    assert_eq!(text(&p), "zzzlo");
    assert_eq!(cursor(&p), (0, 2));
    let p = run("hello", 0, 0, "9rz");
    assert_eq!(text(&p), "hello");
}

#[test]
fn vim_tilde_toggles_case_and_advances() {
    let p = run("aBc", 0, 0, "~");
    assert_eq!(text(&p), "ABc");
    assert_eq!(cursor(&p), (0, 1));
    let p = run("aBc", 0, 0, "3~");
    assert_eq!(text(&p), "AbC");
}

#[test]
fn vim_join_lines_single_space_and_counts() {
    let p = run("one \n  two\nthree", 0, 0, "J");
    assert_eq!(text(&p), "one two\nthree");
    assert_eq!(cursor(&p), (0, 3));
    let p = run("a\nb\nc\nd", 0, 0, "3J");
    assert_eq!(text(&p), "a b c\nd");
    let p = run("a\n\nb", 0, 0, "J");
    assert_eq!(text(&p), "a\nb");
}

// -- Find motions -------------------------------------------------------------

#[test]
fn vim_find_till_and_repeats() {
    let p = run("abcabcabc", 0, 0, "fc");
    assert_eq!(cursor(&p), (0, 2));
    let p = run("abcabcabc", 0, 0, "fc;");
    assert_eq!(cursor(&p), (0, 5));
    let p = run("abcabcabc", 0, 0, "fc;,");
    assert_eq!(cursor(&p), (0, 2));
    let p = run("abcabcabc", 0, 0, "2fc");
    assert_eq!(cursor(&p), (0, 5));
    let p = run("abcabcabc", 0, 0, "tc");
    assert_eq!(cursor(&p), (0, 1));
    // `;` after `t` skips the adjacent match instead of sticking.
    let p = run("abcabcabc", 0, 0, "tc;");
    assert_eq!(cursor(&p), (0, 4));
    let p = run("abcabc", 0, 5, "Fa");
    assert_eq!(cursor(&p), (0, 3));
    let p = run("abcabc", 0, 5, "Ta");
    assert_eq!(cursor(&p), (0, 4));
}

#[test]
fn vim_find_as_operator_target() {
    let (p, register) = run_reg("abcabc", 0, 0, "dfc");
    assert_eq!(text(&p), "abc");
    assert_eq!(register, "abc");
    let p = run("say \"hi\" now", 0, 0, "ct\"");
    assert_eq!(text(&p), "\"hi\" now");
    assert!(matches!(p.mode, MarkdownMode::Insert));
}

// -- Line motions ---------------------------------------------------------------

#[test]
fn vim_gg_and_shift_g_counts() {
    let p = run("a\nb\nc\nd\ne", 0, 0, "G");
    assert_eq!(cursor(&p).0, 4);
    let p = run("a\nb\nc\nd\ne", 4, 0, "gg");
    assert_eq!(cursor(&p).0, 0);
    let p = run("a\nb\nc\nd\ne", 0, 0, "3G");
    assert_eq!(cursor(&p).0, 2);
    let p = run("a\nb\nc\nd\ne", 0, 0, "2gg");
    assert_eq!(cursor(&p).0, 1);
    let p = run("a\nb\nc\nd", 2, 0, "dG");
    assert_eq!(text(&p), "a\nb");
    let p = run("a\nb\nc\nd", 2, 0, "dgg");
    assert_eq!(text(&p), "d");
}

#[test]
fn vim_first_non_blank_and_line_steps() {
    let p = run("  foo", 0, 4, "^");
    assert_eq!(cursor(&p), (0, 2));
    let p = run("a\n  b", 0, 0, "+");
    assert_eq!(cursor(&p), (1, 2));
    let p = run("  a\nb", 1, 0, "-");
    assert_eq!(cursor(&p), (0, 2));
}

#[test]
fn vim_paragraph_motions() {
    let p = run("a\nb\n\nc\nd\n\ne", 0, 0, "}");
    assert_eq!(cursor(&p).0, 2);
    let p = run("a\nb\n\nc\nd\n\ne", 2, 0, "}");
    assert_eq!(cursor(&p).0, 5);
    let p = run("a\nb\n\nc\nd\n\ne", 6, 0, "{");
    assert_eq!(cursor(&p).0, 5);
    let p = run("a\nb\n\nc\nd\n\ne", 5, 0, "{");
    assert_eq!(cursor(&p).0, 2);
}

#[test]
fn vim_percent_matches_brackets() {
    let p = run("fn foo(bar[baz{q}])", 0, 6, "%");
    assert_eq!(cursor(&p), (0, 18));
    let p = run("fn foo(bar[baz{q}])", 0, 18, "%");
    assert_eq!(cursor(&p), (0, 6));
    // From before any bracket, % uses the first bracket on the line.
    let p = run("fn foo(bar)", 0, 0, "%");
    assert_eq!(cursor(&p), (0, 10));
    let p = run("fn foo(bar)", 0, 6, "d%");
    assert_eq!(text(&p), "fn foo");
}

// -- Text objects ----------------------------------------------------------------

#[test]
fn vim_word_objects() {
    let p = run("foo bar baz", 0, 5, "diw");
    assert_eq!(text(&p), "foo  baz");
    let p = run("foo bar baz", 0, 5, "daw");
    assert_eq!(text(&p), "foo baz");
    let p = run("foo,,bar", 0, 3, "diw");
    assert_eq!(text(&p), "foobar");
    let p = run("foo(bar) baz", 0, 4, "diW");
    assert_eq!(text(&p), " baz");
}

#[test]
fn vim_quote_objects_inside_and_before() {
    let p = run("say \"hello\" now", 0, 6, "di\"");
    assert_eq!(text(&p), "say \"\" now");
    assert_eq!(cursor(&p), (0, 5));
    // Cursor before the quoted span selects the next pair.
    let p = run("say \"hello\" now", 0, 0, "di\"");
    assert_eq!(text(&p), "say \"\" now");
    let p = run("say \"hello\" now", 0, 6, "da\"");
    assert_eq!(text(&p), "say now");
    let p = run("it 'is' fine", 0, 4, "ci'");
    assert_eq!(text(&p), "it '' fine");
    assert!(matches!(p.mode, MarkdownMode::Insert));
}

#[test]
fn vim_pair_objects_nested_and_on_delimiter() {
    let p = run("a(b(c)d)e", 0, 4, "di(");
    assert_eq!(text(&p), "a(b()d)e");
    let p = run("a(b(c)d)e", 0, 4, "da(");
    assert_eq!(text(&p), "a(bd)e");
    let p = run("a(b(c)d)e", 0, 1, "di(");
    assert_eq!(text(&p), "a()e");
    let p = run("a(b(c)d)e", 0, 2, "dib");
    assert_eq!(text(&p), "a()e");
    let p = run("x[a\nb]y", 0, 2, "di[");
    assert_eq!(text(&p), "x[]y");
    let p = run("s{t}u", 0, 2, "diB");
    assert_eq!(text(&p), "s{}u");
    let p = run("a <tag> b", 0, 4, "di<");
    assert_eq!(text(&p), "a <> b");
}

#[test]
fn vim_paragraph_objects() {
    let p = run("a\nb\n\nc", 0, 0, "dip");
    assert_eq!(text(&p), "\nc");
    let p = run("a\nb\n\nc", 0, 0, "dap");
    assert_eq!(text(&p), "c");
    // `ap` with no trailing blanks swallows the leading run instead.
    let p = run("a\n\nb\nc", 2, 0, "dap");
    assert_eq!(text(&p), "a");
}

// -- Registers + paste --------------------------------------------------------------

#[test]
fn vim_linewise_vs_charwise_paste() {
    let p = run("one\ntwo", 0, 0, "yyp");
    assert_eq!(text(&p), "one\none\ntwo");
    assert_eq!(cursor(&p).0, 1);
    let p = run("one\ntwo", 1, 0, "yyP");
    assert_eq!(text(&p), "one\ntwo\ntwo");
    assert_eq!(cursor(&p).0, 1);
    let p = run("abc", 0, 0, "ywp");
    assert_eq!(text(&p), "aabcbc");
    assert_eq!(cursor(&p), (0, 3));
    let p = run("x", 0, 0, "yy3p");
    assert_eq!(text(&p), "x\nx\nx\nx");
}

#[test]
fn vim_delete_fills_register_linewise_and_charwise() {
    let (_, register) = run_reg("a\nb", 0, 0, "dd");
    assert_eq!(register, "a\n");
    let (_, register) = run_reg("one two", 0, 0, "dw");
    assert_eq!(register, "one ");
}

// -- Visual mode -----------------------------------------------------------------------

#[test]
fn vim_visual_charwise_extend_and_operate() {
    let p = run("one two", 0, 0, "ved");
    assert_eq!(text(&p), " two");
    assert!(matches!(p.mode, MarkdownMode::Normal));
    let (p, register) = run_reg("one two", 0, 0, "vey");
    assert_eq!(register, "one");
    assert!(matches!(p.mode, MarkdownMode::Normal));
    let p = run("one two", 0, 0, "vec");
    assert_eq!(text(&p), " two");
    assert!(matches!(p.mode, MarkdownMode::Insert));
}

#[test]
fn vim_visual_linewise_operations() {
    let (p, register) = run_reg("a\nb\nc", 0, 0, "Vjd");
    assert_eq!(text(&p), "c");
    assert_eq!(register, "a\nb\n");
    let p = run("a\nb", 0, 0, "Vc");
    assert_eq!(text(&p), "\nb");
    assert!(matches!(p.mode, MarkdownMode::Insert));
    let p = run("a\nb", 0, 0, "V>");
    assert_eq!(text(&p), "  a\nb");
    let (_, register) = run_reg("a\nb", 0, 0, "Vy");
    assert_eq!(register, "a\n");
}

#[test]
fn vim_visual_swap_ends_counts_and_objects() {
    let mut p = pane_at("alpha beta", 0, 0);
    let mut register = String::new();
    feed(&mut p, &mut register, "v2lo");
    assert_eq!(p.visual_anchor.map(|a| a.col), Some(2));
    assert_eq!(cursor(&p), (0, 0));

    let (_, register) = run_reg("foo bar", 0, 5, "viwy");
    assert_eq!(register, "bar");
    let p = run("a(b c)d", 0, 3, "vi(d");
    assert_eq!(text(&p), "a()d");
}

#[test]
fn vim_visual_tilde_and_replace() {
    let p = run("abc", 0, 0, "vll~");
    assert_eq!(text(&p), "ABC");
    assert!(matches!(p.mode, MarkdownMode::Normal));
    let p = run("abc", 0, 0, "vlrx");
    assert_eq!(text(&p), "xxc");
    let p = run("ab\ncd", 0, 0, "Vjrz");
    assert_eq!(text(&p), "zz\nzz");
}

#[test]
fn vim_visual_v_toggles_and_switches_kind() {
    let mut p = pane_at("a\nb", 0, 0);
    let mut register = String::new();
    feed(&mut p, &mut register, "v");
    assert!(matches!(p.mode, MarkdownMode::Visual));
    assert!(!p.vim.visual_linewise);
    feed(&mut p, &mut register, "V");
    assert!(matches!(p.mode, MarkdownMode::Visual));
    assert!(p.vim.visual_linewise);
    feed(&mut p, &mut register, "V");
    assert!(matches!(p.mode, MarkdownMode::Normal));
}

// -- Indent operators ----------------------------------------------------------------------

#[test]
fn vim_indent_and_outdent() {
    let p = run("a\nb\nc", 0, 0, ">>");
    assert_eq!(text(&p), "  a\nb\nc");
    let p = run("a\nb\nc", 0, 0, "3>>");
    assert_eq!(text(&p), "  a\n  b\n  c");
    let p = run("a\nb\nc", 0, 0, ">j");
    assert_eq!(text(&p), "  a\n  b\nc");
    let p = run("  a\n\tb", 0, 0, "<j");
    assert_eq!(text(&p), "a\nb");
}

// -- Search -----------------------------------------------------------------------------------

#[test]
fn vim_star_and_n_wrap_with_word_boundaries() {
    let mut p = pane_at("alpha beta\ngamma alpha\nalphabet alpha", 0, 0);
    let mut register = String::new();
    feed(&mut p, &mut register, "*");
    assert_eq!(cursor(&p), (1, 6));
    // Whole-word: "alphabet" is skipped.
    feed(&mut p, &mut register, "n");
    assert_eq!(cursor(&p), (2, 9));
    feed(&mut p, &mut register, "n");
    assert_eq!(cursor(&p), (0, 0));
    feed(&mut p, &mut register, "N");
    assert_eq!(cursor(&p), (2, 9));
    let p = run("beta\nalpha beta", 1, 6, "#");
    assert_eq!(cursor(&p), (0, 0));
}

// -- Dot repeat ----------------------------------------------------------------------------------

#[test]
fn vim_dot_repeats_simple_edits_and_operators() {
    let p = run("abcdef", 0, 0, "x.");
    assert_eq!(text(&p), "cdef");
    let p = run("abcdef", 0, 0, "x3.");
    assert_eq!(text(&p), "ef");
    let p = run("one two three", 0, 0, "dw.");
    assert_eq!(text(&p), "three");
    let p = run("aaaa", 0, 0, "rbl.");
    assert_eq!(text(&p), "bbaa");
    let p = run("a\nb\nc\nd", 0, 0, "dd.");
    assert_eq!(text(&p), "c\nd");
    let p = run("x\ny", 0, 0, "yyp.");
    assert_eq!(text(&p), "x\nx\nx\ny");
}

// -- Undo integration ------------------------------------------------------------------------------

#[test]
fn vim_paragraph_and_backward_operator_targets() {
    let p = run("one two\n\nthree", 0, 4, "d}");
    assert_eq!(text(&p), "one \n\nthree");
    // `ge` is inclusive at both ends: the cursor char joins the cut.
    let p = run("alpha beta", 0, 8, "dge");
    assert_eq!(text(&p), "alpha");
    let p = run("one two", 0, 4, "db");
    assert_eq!(text(&p), "two");
    let p = run("abc", 0, 2, "ywP");
    assert_eq!(text(&p), "abcc");
    assert_eq!(cursor(&p), (0, 2));
    let (p, _) = run_reg("ab", 0, 1, "ylP");
    assert_eq!(text(&p), "abb");
}

#[test]
fn vim_operator_edits_are_undoable() {
    let mut p = pane_at("one two three", 0, 0);
    let mut register = String::new();
    feed(&mut p, &mut register, "d2w");
    assert_eq!(text(&p), "three");
    assert!(p.undo());
    assert_eq!(text(&p), "one two three");
    let mut p = pane_at("a\nb\nc", 0, 0);
    feed(&mut p, &mut register, "2dd");
    assert_eq!(text(&p), "c");
    assert!(p.undo());
    assert_eq!(text(&p), "a\nb\nc");
}
