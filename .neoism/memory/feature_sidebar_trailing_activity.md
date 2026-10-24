---
name: "Right-side conversation activity indicators"
description: "Conversation status moved to trailing column; titles left-aligned, timestamps clear; current sidebar UX overrides"
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-15"
updated: "2026-10-15"
---

User requested conversation green current-session dots and animated square activity indicators move from the LEFT gutter to the RIGHT so conversation titles start at the normal left inset. Implemented in shared view/side_panel/draw.rs, affecting native and web conversation rows only; right detail/subagent status styling remains unchanged.

conversation_row_columns(left,width,scale) reserves an 18px-scaled trailing status column, returns text width and status center, clamps narrow widths. Title/icon/source metadata, timestamps, excerpts and skeleton alignment now begin at text_x with no leading activity gutter. Both green dot and square spinner are centered in the trailing slot. Timestamp/source text and title hover-marquee clipping reserve that right column, so text cannot overlap status. Fixed header/caption indentation remains unchanged. Activity colors, scoped SSE/live overrides, clipping/occlusion, animation visibility ownership, pagination and hit targets are unchanged. Do not restore the old 18px leading dot gutter.

Added pure geometry regression covering status/text separation at 1x/1.5x/2x and narrow widths, alongside existing visible-nonoccluded square and typography/time tests. Final shared/native-web check result is in the completing session. No release build or app restart.

Latest preceding sidebar UX (supersedes older P2/project-picker memories): root selector label and root option are ~ (display only; actual authorized root path unchanged); verified root directory chat group also ~ and first among projects if it exists, below GLOBAL Pinned. No Recent heading in root; scoped Pinned/Recent use regular body font. Search row and folder-plus are hidden/removed; Add workspace remains in dropdown. Keyboard New chat -> Projects/~ -> selectable rows, Enter on selector opens it. Pending loading uses static clipped skeletons. Pointing cursor uses catalog_clickable_at, composer_pointer_at and detail_sidebar_pointer_at for real controls/rows, plus file tree hit tests. New chat transfers focus to actual composer via existing native/shared focus chain, with lazy creation and stale-reply route protection. Workspace registry expected GET404 is quiet when management API disabled; authoritative chat-directory summaries/current root still populate picker. These preceding changes passed 124 sidebar tests, 9 picker, 8 navigator, 5 new-chat, 56 tree; 49 browser tests/typecheck and native/server/daemon/WASM checks before this activity-position-only followup. Broader known baseline failures remain separate.
