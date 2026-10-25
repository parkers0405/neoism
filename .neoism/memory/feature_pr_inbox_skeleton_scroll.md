---
name: "PR inbox skeletons and full-height scrolling"
description: "Skeleton-only pending PR surfaces; whole detail chrome scrolls away so Code gets full height; residual chain and resize fixes"
type: "feature"
scope: "project"
origin: "User rejected Loading x text and pinned Summary/Timeline/Code taking Code viewport"
created: "2026-10-23"
updated: "2026-10-23"
---

User explicitly requested skeleton loading, not 'Loading x...' prose, for the PR inbox, and requested that detail title/actions/Summary Timeline Code tabs scroll away to free the tiny Code viewport. Implemented in shared panels/git_pull_request_inbox{,_render,_list,_tests}.rs, with PR-only missing-diff fallback in git_diff_view.rs. Static muted skeleton bars are clipped to viewport/occlusions, have no hit targets, and do not add animation/redraw ownership. They replace repository/list/pagination/read/body/action loading placeholders. Completed empty and real error states remain textual; a loaded immutable PR file omitted from textual diffs must say no text diff available, NOT show a permanent loading skeleton.

The inbox owns detail_scroll plus measured prelude_height and a visible child_rect. Summary's entire document including header scrolls. For Code/Timeline, positive input consumes prelude collapse first then forwards only residual delta to child; reverse exhausts child scroll before revealing prelude. Child viewport top=max(natural_y, detail.y), bottom=detail.bottom; as chrome scrolls out it grows to full detail height. Never render a negative-origin enlarged child rect because it is both layout and clip. Track child max-scroll changes as viewport grows, preserve original mono font ID, and rebase detail_scroll to a shorter prelude on resize before residual accounting (otherwise reverse wheel incorrectly becomes forward child motion). Open review sheets own scrolling exclusively and get full detail viewport. Header underline, disabled actions, buttons, skeletons and hits must remain clipped. Child normal pointer/horizontal routing only within child_rect, captured drag/up still delivered; keyboard page/home/end and rail drag integrate collapse. Cached children/drafts and read/mutation reply ownership are unchanged.

Regression tests cover scroll chain/residual/reverse/viewport growth, sheet priority, tiny/narrow/split geometry, hidden header hits, list-vs-detail ownership, skeleton loading-vs-empty/error state, and resized prelude not reversing input. Parent initial final verification passed 34 inbox tests, 39 PR tests, 157 Git-filter tests, native cargo check --tests and WASM sugarloaf/web check; native test bodies typechecked, no live GPU/app launch. Existing warnings outside scope untouched. See feature_pr_inbox.md and docs/pull-requests.md for base feature.
