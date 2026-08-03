//! PTY session wire messages.

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum ClientMessage {
    CreatePty {
        cwd: Option<String>,
        cols: u16,
        rows: u16,
        /// Optional explicit shell path. Falls back to `$SHELL` (then
        /// `/bin/sh`) when omitted. `#[serde(default)]` keeps every
        /// pre-existing `CreatePty` envelope on the wire valid — the
        /// daemon's `SessionRegistry::create` already handles `None`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        shell: Option<String>,
    },
    PtyInput {
        session_id: String,
        bytes: Vec<u8>,
    },
    Resize {
        session_id: String,
        cols: u16,
        rows: u16,
    },
    ClosePty {
        session_id: String,
    },
    /// 8C adopt: ask for a one-shot replay of `session_id`'s retained
    /// output backlog, delivered as a `PtyOutput` REPLY on this
    /// connection only. Clients that bind to an already-running
    /// session mid-stream (a desktop adopting a web workspace, a web
    /// tab adopting a desktop one) use this so the terminal carries
    /// its scrollback instead of starting blank.
    AttachPty {
        session_id: String,
    },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum ServerMessage {
    /// Correlated control acknowledgment. This is carried inside a
    /// `PtyReply` service envelope; the envelope's request id identifies the
    /// operation. It keeps successful input/resize requests from lingering in
    /// a reconnect replay queue without inventing a fake output event.
    Ack,
    PtyCreated {
        session_id: String,
        /// Absolute daemon workspace root for this PTY. Older daemons
        /// omitted this, so clients must treat `None` as unknown.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workspace_root: Option<String>,
    },
    PtyOutput {
        session_id: String,
        bytes: Vec<u8>,
    },
    PtyClosed {
        session_id: String,
        exit_code: Option<i32>,
    },
    /// Daemon-tracked working directory of a live PTY's foreground
    /// process group. Pushed unsolicited whenever the shell's cwd
    /// changes — the daemon polls `/proc/<pgrp>/cwd` (the same source the
    /// desktop reads in-process). Clients re-root the file tree / LSP /
    /// git off this so a `cd` in the active terminal moves the workspace
    /// root, and switching workspaces re-roots to that workspace's
    /// terminal cwd. Added after `PtyClosed`, so older clients that don't
    /// know the variant simply ignore it.
    SessionCwd {
        session_id: String,
        cwd: String,
    },
    Error {
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip_client(msg: &ClientMessage) {
        let json = serde_json::to_string(msg).expect("serialize");
        let back: ClientMessage = serde_json::from_str(&json).expect("deserialize");
        // Compare via re-serialized JSON since the message types do not
        // currently derive PartialEq (and we want to keep their shapes
        // exactly as specified).
        let json_back = serde_json::to_string(&back).expect("re-serialize");
        assert_eq!(json, json_back, "roundtrip mismatch: {json}");
    }

    fn roundtrip_server(msg: &ServerMessage) {
        let json = serde_json::to_string(msg).expect("serialize");
        let back: ServerMessage = serde_json::from_str(&json).expect("deserialize");
        let json_back = serde_json::to_string(&back).expect("re-serialize");
        assert_eq!(json, json_back, "roundtrip mismatch: {json}");
    }

    #[test]
    fn client_create_pty_roundtrip() {
        roundtrip_client(&ClientMessage::CreatePty {
            cwd: Some("/tmp".into()),
            cols: 80,
            rows: 24,
            shell: None,
        });
        roundtrip_client(&ClientMessage::CreatePty {
            cwd: None,
            cols: 132,
            rows: 50,
            shell: None,
        });
        // Round-trip with an explicit shell so we cover both
        // `#[serde(default)]` (missing field) and the populated path.
        roundtrip_client(&ClientMessage::CreatePty {
            cwd: Some("/work".into()),
            cols: 100,
            rows: 30,
            shell: Some("/bin/fish".into()),
        });
    }

    #[test]
    fn client_pty_input_roundtrip() {
        roundtrip_client(&ClientMessage::PtyInput {
            session_id: "s-1".into(),
            bytes: b"echo hi\n".to_vec(),
        });
    }

    #[test]
    fn client_resize_roundtrip() {
        roundtrip_client(&ClientMessage::Resize {
            session_id: "s-1".into(),
            cols: 100,
            rows: 40,
        });
    }

    #[test]
    fn client_close_pty_roundtrip() {
        roundtrip_client(&ClientMessage::ClosePty {
            session_id: "s-1".into(),
        });
    }

    #[test]
    fn server_pty_created_roundtrip() {
        roundtrip_server(&ServerMessage::PtyCreated {
            session_id: "s-1".into(),
            workspace_root: None,
        });
        roundtrip_server(&ServerMessage::PtyCreated {
            session_id: "s-2".into(),
            workspace_root: Some("/work".into()),
        });
    }

    #[test]
    fn server_ack_roundtrip() {
        roundtrip_server(&ServerMessage::Ack);
    }

    #[test]
    fn server_pty_output_roundtrip() {
        roundtrip_server(&ServerMessage::PtyOutput {
            session_id: "s-1".into(),
            bytes: vec![0, 1, 2, 255],
        });
    }

    #[test]
    fn server_pty_closed_roundtrip() {
        roundtrip_server(&ServerMessage::PtyClosed {
            session_id: "s-1".into(),
            exit_code: Some(0),
        });
        roundtrip_server(&ServerMessage::PtyClosed {
            session_id: "s-1".into(),
            exit_code: None,
        });
    }

    #[test]
    fn server_error_roundtrip() {
        roundtrip_server(&ServerMessage::Error {
            message: "boom".into(),
        });
    }
}
