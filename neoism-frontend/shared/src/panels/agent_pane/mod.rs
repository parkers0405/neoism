//! Agent pane — shared rendering tree for Neoism's chat / agent UI.
//!
//! `view` is the rendering surface lifted from the desktop fork
//! (`frontends/neoism/src/neoism/view/`). It paints the agent pane via
//! `sugarloaf` and consumes the state types defined in `state`. The
//! desktop binary keeps its own fork in place until the cutover lands.

pub mod api_mapping;
pub mod attachment_policy;
pub mod bridge_policy;

pub mod command_controller;
pub mod icon;
pub mod input_controller;
pub mod interaction_policy;
pub mod message_policy;
pub mod outbound;
pub mod permission_policy;
pub mod protocol_mapping;
pub mod question_policy;
pub mod resolve_link_path;
pub mod session_group;
pub mod state;
pub mod status_policy;
pub mod stream_events;
pub mod timeline_scroll_policy;
pub mod usage_policy;
pub mod view;
