---
name: "Native block RPC declared workspace scope"
description: "Native exact-block RPC honors declared nested workspace, preserves completion state through provider transitions, invalidates stale endpoint/workspace replies"
type: "feature"
scope: "project"
origin: "Final native review block routing verification"
created: "2026-10-22"
updated: "2026-10-22"
---

Final native follow-up in screen/git_state.rs: dispatch_review_block calls sync_host_git before capturing operation, sends RPC envelope root from daemon_host_workspace_root(current_workspace_tree_id()), and fails closed when declaration unavailable. ApplyBlock.path remains repository-relative. NativeBlockScope tracks endpoint/connection/workspace ID/declared workspace root/repository root and invalidates on any change. Subscription resets preserve independently validated native blocks plus completion channel; stable local frames do not reset them. cargo check -p neoism --tests --quiet and final git diff --check passed. Native test bodies were typechecked, not executed. See feature_git_exact_block_review.md for full feature/restore invariants.
