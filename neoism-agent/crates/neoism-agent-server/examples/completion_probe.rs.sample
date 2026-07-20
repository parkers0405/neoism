// ─────────────────────────────────────────────────────────────────────────
// LSP COMPLETION PROBE  —  this file IS a real crate member.
//
// It lives under `neoism-agent-server/examples/`, so Cargo auto-detects it as
// an example target and rust-analyzer analyses it as a genuine crate (edition,
// cfg, sysroot all bound). That is the ONE thing the old root-level
// COMPLETION_TEST.rs lacked — that file was an orphan (member of no crate), so
// rust-analyzer returned `null` for every request. This file will NOT.
//
// HOW TO TEST
//   1. Open this file in Neoism.
//   2. Wait until the pill says Rust/Attached and `progress[rust]` logs stop.
//   3. For each probe: enter insert mode, put the cursor EXACTLY at the caret
//      marker described, and type. A popup should appear.
//   4. Paste the ONE `ENGINE completion RESULT:` log line. If it now says
//      `raw_items=Some(N)`, completion works end-to-end.
// ─────────────────────────────────────────────────────────────────────────

fn main() {
    let text = String::from("hello world");
    let numbers = vec![1, 2, 3, 4, 5];

    // PROBE 1 — METHOD COMPLETION
    // Click at the very end of the next line (right after `text.`), stay in
    // insert mode, type `l`. Expected: len, push_str, chars, split, ...
    let _a = text.

    // PROBE 2 — ASSOCIATED FUNCTION
    // Click right after `String::` on the next line, type `n`.
    // Expected: new, from, with_capacity, ...
    let _b = String::

    // PROBE 3 — ITERATOR METHODS
    // Click right after `numbers.iter().`, type `m`.
    // Expected: map, filter, count, sum, collect, ...
    let _c = numbers.iter().

    // Keep the sample values used so rust-analyzer doesn't fold them away.
    let _keep = (&text, &numbers, &_a, &_b, &_c);

    // PROBE 4 — DIAGNOSTICS (optional)
    // Un-comment the next line. rust-analyzer should flag a type mismatch
    // and the status pill should show an error count.
    // let _bad: i32 = "not a number";
}
