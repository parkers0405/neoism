//! `scroll-commit`: an app running its whole view on most frames of a scroll. Rune moves a
//! scroll's content, mounts a virtual list's rows and paints hover with no commit, so a scroll
//! that commits on most frames is the app asking: its `poll`, a handler or a layout read. Debug
//! builds name the site in a `rune:` warning as it happens.

use super::{Finding, Recording, rule};
use crate::runtime::CommitWhy;

/// Committing frames a scroll may take before the share counts: a list loading its next page
/// commits a few times.
const COMMITS: usize = 6;

pub(super) fn scroll_commits(rec: &Recording, out: &mut Vec<Finding>) {
    let share = rule("scroll-commit");
    let mut k = 0;
    while k < rec.scrolled.len() {
        if !rec.scrolled[k].0 {
            k += 1;
            continue;
        }
        let from = k;
        while k < rec.scrolled.len() && rec.scrolled[k].0 { k += 1; }
        let run = &rec.scrolled[from..k];
        let committed: Vec<usize> = (from..k).filter(|&f| rec.scrolled[f].1 != 0).collect();
        if committed.len() < COMMITS || (committed.len() as f32) < share * run.len() as f32 { continue; }
        let why = run.iter().fold(0, |b, f| b | f.1);
        out.push(Finding {
            kind: "scroll-commit",
            element: "frame".into(),
            frame: committed[0],
            detail: format!("ran the view on {} of the {} frames of a scroll ({}): the debug build's `rune:` warning names the site",
                committed.len(), run.len(), CommitWhy::names(why).join(", ")),
        });
    }
}
