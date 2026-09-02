---
name: feature-prompt-picker
description: "Agent prompt picker — permissions + model questions rendered as the inline-picker card popping out of the input island (replaced timeline permission card), 2026-07-14"
metadata: 
  node_type: memory
  type: project
  originSessionId: b38427bd-f724-4a0c-a271-2f226f3736c2
---

**Prompt picker (2026-07-14, uncommitted):** permissions AND `question`-tool requests render as the same `inline_picker` card the "/" menu uses, anchored to the agent input island. Old timeline permission card deleted.

**Shared (neoism-ui):**
- `agent_pane/question_policy.rs` — `NeoismAgentPendingQuestion` state machine (visible_rows = filtered options + synthetic "Answer: <typed>" custom row; sequential multi-question → `QuestionCommit::Finished(Vec<Vec<String>>)`; lenient `question_request_from_event` parse: text from question|label|title, options from options|choices, string or {label,description}).
- `view/prompt_picker.rs` — `render_prompt_picker` (permission precedence over question) → returns card rect; registers row hit rects via new `inline_picker::row_rect(card, ix, s)`. Permission rows Always/Yes/No in `VISUAL_SELECTION_ORDER` [1,0,2]; meta line rides `search_placeholder` (query="", no caret); question typed buffer IS the query row (caret on).
- `view/mod.rs`: prompt renders in the picker slot; `/` picker suppressed while prompt pending; `pane.set_prompt_picker_rect` feeds occlusion (desktop `picker_card_rect()` unions it).
- stream_events: `question.asked` → `SessionEventUpdate::QuestionAsked` (was a System msg pointing at /answer), `question.replied|rejected` → `QuestionRemoved` (requestID).
- `AgentUserInputPane` trait + macro grew: pending_question / clear+register question rects / set_prompt_picker_rect / clear_permission_choice_hit_rects.

**Desktop:** pane fields pending_question(+queue)/question_option_hit_rects/prompt_picker_rect; `pane/questions.rs` mirrors permissions via permission_policy generics; outbound `ReplyQuestion{id,answers}`/`RejectQuestion{id}` → POST `/question/{id}/reply|reject` (ingest.rs executors); key bridge block after permissions: arrows/Tab move, typing filters+free-answers, Enter commits, Esc rejects (run resumes with "rejected" error); click routing `respond_question_at` after `respond_permission_at`.

**Server facts:** `question` tool parks run on oneshot until `/question/{id}/reply` `{answers:[[..]]}`; events carry serialized QuestionRequestInfo (camelCase). Web bridge: protocol_mapping maps Reply/RejectQuestion → Unsupported (no ws envelope yet — TODO if web needs it).

**Also fixed in passing:** other session's sugarloaf emoji work broke wasm (`neoism-grapheme-width` in not-wasm dep table but `prefers_color_emoji` un-gated) — moved dep to main `[dependencies]` (pure-data crate, wasm-safe).

Related: [[feature-agent-input-bar]], [[feature_agent_gui_and_git_panel]].
