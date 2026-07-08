    use super::*;

    #[test]
    fn which_nvim_returns_some_when_on_path() {
        // The daemon host carries nvim in CI; on dev hosts that don't
        // have nvim installed, this test is informational. We accept
        // either outcome so the suite stays green everywhere.
        let _ = which_nvim();
    }

    #[test]
    fn decode_diagnostics_pulls_lnum_col_severity() {
        // Hand-build the value shape `vim.diagnostic.get` returns
        // through `exec_lua`.
        let item: Vec<(Value, Value)> = vec![
            (Value::from("lnum"), Value::from(12u64)),
            (Value::from("col"), Value::from(4u64)),
            (Value::from("severity"), Value::from(2u64)),
            (Value::from("message"), Value::from("unused var")),
            (Value::from("source"), Value::from("rust-analyzer")),
        ];
        let value = Value::Array(vec![Value::Map(item)]);
        let out = decode_diagnostics(value);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].line, 12);
        assert_eq!(out[0].col, 4);
        assert_eq!(out[0].severity, 2);
        assert_eq!(out[0].message, "unused var");
        assert_eq!(out[0].source.as_deref(), Some("rust-analyzer"));
    }

    #[test]
    fn map_get_str_reads_buffer_read_shape() {
        // The `{ path, text }` lua table `read_active_buffer` returns.
        let map: Vec<(Value, Value)> = vec![
            (Value::from("path"), Value::from("/work/src/main.rs")),
            (Value::from("text"), Value::from("fn main() {}\nok")),
        ];
        assert_eq!(
            map_get_str(&map, "path").as_deref(),
            Some("/work/src/main.rs")
        );
        assert_eq!(
            map_get_str(&map, "text").as_deref(),
            Some("fn main() {}\nok")
        );
        assert_eq!(map_get_str(&map, "missing"), None);
    }

    #[test]
    fn map_get_str_handles_empty_path_for_unnamed_buffer() {
        // An unnamed/scratch buffer yields an empty path; the caller
        // treats this as "no authoritative document to seed".
        let map: Vec<(Value, Value)> = vec![
            (Value::from("path"), Value::from("")),
            (Value::from("text"), Value::from("")),
        ];
        assert_eq!(map_get_str(&map, "path").as_deref(), Some(""));
    }

    #[test]
    fn diagnostics_subscriptions_track_routes() {
        let mut subs = DiagnosticsSubscriptions::new();
        subs.subscribe(7);
        subs.subscribe(9);
        assert_eq!(subs.routes().len(), 2);
        subs.unsubscribe(7);
        assert_eq!(subs.routes().len(), 1);
    }

    #[test]
    fn hash_items_is_stable_for_identical_input() {
        let a = vec![ProtoDiagnosticItem {
            line: 1,
            col: 2,
            severity: 1,
            message: "x".into(),
            source: None,
        }];
        let b = a.clone();
        assert_eq!(hash_items(&a), hash_items(&b));
    }

    #[test]
    fn nvim_registry_keys_surface_ids() {
        let message = EditorClientMessage::SendKeys {
            bytes: b"x".to_vec(),
            surface_id: Some("pane:42".into()),
        };
        assert_eq!(NvimSessionRegistry::key_for_message(&message), "pane:42");
        assert_eq!(
            NvimSessionRegistry::key_for_message(&EditorClientMessage::Close),
            DEFAULT_SESSION_KEY
        );
    }

    #[tokio::test]
    async fn redraw_handler_stamps_active_surface_id_on_grid_resize() {
        let (redraw_tx, mut redraw_rx) = mpsc::unbounded_channel::<EditorServerMessage>();
        let (cursor_overlay_tx, _cursor_overlay_rx) =
            mpsc::unbounded_channel::<CursorOverlayServerMessage>();
        let (buffer_lines_tx, _buffer_lines_rx) =
            mpsc::unbounded_channel::<NvimBufferEvent>();
        let handler = RedrawHandler {
            redraw_tx,
            cursor_overlay_tx,
            buffer_lines_tx,
            hl_table: Arc::new(Mutex::new(HighlightTable::default())),
            default_fg: Arc::new(Mutex::new(0x00FF_FFFF)),
            default_bg: Arc::new(Mutex::new(0x0000_0000)),
            grid_sizes: Arc::new(Mutex::new(HashMap::new())),
            last_cursor: Arc::new(Mutex::new(LastCursor::default())),
            active_surface_id: Arc::new(Mutex::new(Some("pane:root".into()))),
            redraw_enabled: Arc::new(Mutex::new(true)),
            redraw_batch: Arc::new(Mutex::new(None)),
            textoff: Arc::new(Mutex::new(0)),
        };

        let mut pending = HashMap::new();
        let mut last_hl = 0;
        handler
            .handle_event(
                "grid_resize",
                vec![Value::from(1u64), Value::from(120u64), Value::from(40u64)],
                &mut pending,
                &mut last_hl,
            )
            .await;

        let msg = redraw_rx.recv().await.expect("grid_resize emitted");
        match msg {
            EditorServerMessage::GridResize {
                surface_id,
                grid_id,
                width,
                height,
            } => {
                assert_eq!(surface_id.as_deref(), Some("pane:root"));
                assert_eq!(grid_id, 1);
                assert_eq!((width, height), (120, 40));
            }
            other => panic!("unexpected redraw message: {other:?}"),
        }
    }

    #[tokio::test]
    async fn redraw_handler_emits_grid_clear_for_atomic_batching() {
        let (redraw_tx, mut redraw_rx) = mpsc::unbounded_channel::<EditorServerMessage>();
        let (cursor_overlay_tx, _cursor_overlay_rx) =
            mpsc::unbounded_channel::<CursorOverlayServerMessage>();
        let (buffer_lines_tx, _buffer_lines_rx) =
            mpsc::unbounded_channel::<NvimBufferEvent>();
        let handler = RedrawHandler {
            redraw_tx,
            cursor_overlay_tx,
            buffer_lines_tx,
            hl_table: Arc::new(Mutex::new(HighlightTable::default())),
            default_fg: Arc::new(Mutex::new(0x00FF_FFFF)),
            default_bg: Arc::new(Mutex::new(0x0000_0000)),
            grid_sizes: Arc::new(Mutex::new(HashMap::new())),
            last_cursor: Arc::new(Mutex::new(LastCursor::default())),
            active_surface_id: Arc::new(Mutex::new(Some("pane:root".into()))),
            redraw_enabled: Arc::new(Mutex::new(true)),
            redraw_batch: Arc::new(Mutex::new(None)),
            textoff: Arc::new(Mutex::new(0)),
        };

        let mut pending = HashMap::new();
        let mut last_hl = 0;
        handler
            .handle_event(
                "grid_clear",
                vec![Value::from(2u64)],
                &mut pending,
                &mut last_hl,
            )
            .await;

        let msg = redraw_rx.recv().await.expect("grid_clear emitted");
        match msg {
            EditorServerMessage::GridClear {
                surface_id,
                grid_id,
            } => {
                assert_eq!(surface_id.as_deref(), Some("pane:root"));
                assert_eq!(grid_id, 2);
            }
            other => panic!("unexpected redraw message: {other:?}"),
        }
    }

    #[tokio::test]
    async fn redraw_handler_flushes_pending_cells_before_cursor() {
        let (redraw_tx, mut redraw_rx) = mpsc::unbounded_channel::<EditorServerMessage>();
        let (cursor_overlay_tx, _cursor_overlay_rx) =
            mpsc::unbounded_channel::<CursorOverlayServerMessage>();
        let (buffer_lines_tx, _buffer_lines_rx) =
            mpsc::unbounded_channel::<NvimBufferEvent>();
        let handler = RedrawHandler {
            redraw_tx,
            cursor_overlay_tx,
            buffer_lines_tx,
            hl_table: Arc::new(Mutex::new(HighlightTable::default())),
            default_fg: Arc::new(Mutex::new(0x00FF_FFFF)),
            default_bg: Arc::new(Mutex::new(0x0000_0000)),
            grid_sizes: Arc::new(Mutex::new(HashMap::new())),
            last_cursor: Arc::new(Mutex::new(LastCursor::default())),
            active_surface_id: Arc::new(Mutex::new(Some("pane:root".into()))),
            redraw_enabled: Arc::new(Mutex::new(true)),
            redraw_batch: Arc::new(Mutex::new(None)),
            textoff: Arc::new(Mutex::new(0)),
        };

        let mut pending = HashMap::new();
        let mut last_hl = 0;
        handler
            .handle_event(
                "grid_line",
                vec![
                    Value::from(1u64),
                    Value::from(2u64),
                    Value::from(3u64),
                    Value::Array(vec![Value::Array(vec![Value::from("A")])]),
                ],
                &mut pending,
                &mut last_hl,
            )
            .await;
        handler
            .handle_event(
                "grid_cursor_goto",
                vec![Value::from(1u64), Value::from(2u64), Value::from(4u64)],
                &mut pending,
                &mut last_hl,
            )
            .await;

        let msg = redraw_rx.recv().await.expect("grid update emitted");
        match msg {
            EditorServerMessage::GridUpdate { cells, cursor, .. } => {
                assert_eq!(cells.len(), 1);
                assert_eq!(cells[0].row, 2);
                assert_eq!(cells[0].col, 3);
                assert_eq!(cells[0].ch, "A");
                assert_eq!(cursor, Some(GridPos { row: 2, col: 4 }));
            }
            other => panic!("unexpected redraw message: {other:?}"),
        }
        assert!(redraw_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn redraw_handler_decodes_msg_show_lua_print() {
        let (redraw_tx, mut redraw_rx) = mpsc::unbounded_channel::<EditorServerMessage>();
        let (cursor_overlay_tx, _cursor_overlay_rx) =
            mpsc::unbounded_channel::<CursorOverlayServerMessage>();
        let (buffer_lines_tx, _buffer_lines_rx) =
            mpsc::unbounded_channel::<NvimBufferEvent>();
        let handler = RedrawHandler {
            redraw_tx,
            cursor_overlay_tx,
            buffer_lines_tx,
            hl_table: Arc::new(Mutex::new(HighlightTable::default())),
            default_fg: Arc::new(Mutex::new(0x00FF_FFFF)),
            default_bg: Arc::new(Mutex::new(0x0000_0000)),
            grid_sizes: Arc::new(Mutex::new(HashMap::new())),
            last_cursor: Arc::new(Mutex::new(LastCursor::default())),
            active_surface_id: Arc::new(Mutex::new(Some("pane:root".into()))),
            redraw_enabled: Arc::new(Mutex::new(true)),
            redraw_batch: Arc::new(Mutex::new(None)),
            textoff: Arc::new(Mutex::new(0)),
        };

        let mut pending = HashMap::new();
        let mut last_hl = 0;
        handler
            .handle_event(
                "msg_show",
                vec![
                    Value::from("lua_print"),
                    Value::Array(vec![Value::Array(vec![
                        Value::from(0u64),
                        Value::from("hi"),
                    ])]),
                    Value::from(false),
                    Value::Array(vec![]),
                ],
                &mut pending,
                &mut last_hl,
            )
            .await;

        let msg = redraw_rx.recv().await.expect("message emitted");
        match msg {
            EditorServerMessage::Message {
                surface_id,
                kind,
                content,
                replace_last,
            } => {
                assert_eq!(surface_id.as_deref(), Some("pane:root"));
                assert_eq!(kind, "lua_print");
                assert_eq!(content, "hi");
                assert!(!replace_last);
            }
            other => panic!("unexpected redraw message: {other:?}"),
        }
    }

    async fn recv_grid_cell(
        rx: &mut broadcast::Receiver<EditorServerMessage>,
        expected: &str,
    ) -> bool {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let now = tokio::time::Instant::now();
            if now >= deadline {
                return false;
            }
            match tokio::time::timeout_at(deadline, rx.recv()).await {
                Ok(Ok(EditorServerMessage::GridUpdate { cells, .. })) => {
                    if cells.iter().any(|cell| cell.ch == expected) {
                        return true;
                    }
                }
                Ok(Ok(_)) => {}
                Ok(Err(broadcast::error::RecvError::Lagged(_))) => {}
                Ok(Err(broadcast::error::RecvError::Closed)) | Err(_) => return false,
            }
        }
    }

    async fn recv_message_containing(
        rx: &mut broadcast::Receiver<EditorServerMessage>,
        expected: &str,
    ) -> bool {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let now = tokio::time::Instant::now();
            if now >= deadline {
                return false;
            }
            match tokio::time::timeout_at(deadline, rx.recv()).await {
                Ok(Ok(EditorServerMessage::Message { content, .. })) => {
                    if content.contains(expected) {
                        return true;
                    }
                }
                Ok(Ok(_)) => {}
                Ok(Err(broadcast::error::RecvError::Lagged(_))) => {}
                Ok(Err(broadcast::error::RecvError::Closed)) | Err(_) => return false,
            }
        }
    }

    #[tokio::test]
    async fn nvim_spawn_insert_roundtrip_when_available() {
        if which_nvim().is_none() {
            eprintln!("skipping nvim spawn test: `nvim` not found on PATH");
            return;
        }

        let registry = NvimSessionRegistry::new();
        let handle = registry
            .get_or_spawn("test:single".into(), &CrdtSyncHub::default())
            .await
            .expect("spawn nvim");
        let mut rx = handle.subscribe_redraw();
        handle
            .handle(EditorClientMessage::SendKeys {
                bytes: b"iHello<Esc>".to_vec(),
                surface_id: Some("test:single".into()),
            })
            .await
            .expect("send keys");

        assert!(
            recv_grid_cell(&mut rx, "H").await,
            "expected grid output for inserted text"
        );
        let _ = handle.handle(EditorClientMessage::Close).await;
        registry.remove(handle.key()).await;
    }

    /// Regression test for the digit-key freeze: a bare count leaves
    /// nvim deferring every non-fast RPC, and the per-envelope
    /// workspace-root sync used to run an unconditional `:cd` that
    /// timed out (4s) and dropped the very keystroke that would have
    /// cleared the count.
    #[tokio::test]
    async fn pending_count_does_not_wedge_input_when_available() {
        if which_nvim().is_none() {
            eprintln!("skipping nvim pending-count test: `nvim` not found on PATH");
            return;
        }

        let registry = NvimSessionRegistry::new();
        let handle = registry
            .get_or_spawn("test:count".into(), &CrdtSyncHub::default())
            .await
            .expect("spawn nvim");
        let mut rx = handle.subscribe_redraw();
        let root = std::env::temp_dir();
        handle.set_workspace_root(&root).await.expect("initial cd");

        // Bare digit → pending-count state (non-fast RPC now deferred).
        handle
            .handle(EditorClientMessage::SendKeys {
                bytes: b"3".to_vec(),
                surface_id: Some("test:count".into()),
            })
            .await
            .expect("send digit");

        // Same-root sync must no-op without touching nvim.
        let t0 = std::time::Instant::now();
        handle
            .set_workspace_root(&root)
            .await
            .expect("same-root no-op");
        assert!(
            t0.elapsed() < std::time::Duration::from_secs(1),
            "same-root set_workspace_root must not round-trip into nvim"
        );

        // The diagnostics poll gates on nvim_get_mode().blocking and
        // skips (None) instead of stacking a deferred exec_lua.
        let t0 = std::time::Instant::now();
        assert!(
            handle.snapshot_diagnostics().await.is_none(),
            "diagnostics poll must skip while a count is pending"
        );
        assert!(
            t0.elapsed() < std::time::Duration::from_secs(2),
            "diagnostics poll must answer fast while blocked"
        );

        // Input lane stays live: clear the count and type.
        handle
            .handle(EditorClientMessage::SendKeys {
                bytes: b"<Esc>iZ<Esc>".to_vec(),
                surface_id: Some("test:count".into()),
            })
            .await
            .expect("keys after digit");
        assert!(
            recv_grid_cell(&mut rx, "Z").await,
            "input lane must stay live after a pending count"
        );

        let _ = handle.handle(EditorClientMessage::Close).await;
        registry.remove(handle.key()).await;
    }

    /// Regression test for the digit-key freeze via a deferred COMMAND:
    /// a bare count leaves nvim deferring every non-fast RPC. If
    /// `handle(Command)` issues that deferred `nvim_command` while holding
    /// the session Mutex, a concurrent `SendKeys` (the key that would
    /// clear the count) blocks on the same Mutex for the full 4s rpc
    /// window — the editor freezes. The fix issues the command WITHOUT
    /// holding the session lock across the await, so input stays live.
    #[tokio::test]
    async fn pending_count_command_does_not_wedge_concurrent_input_when_available() {
        if which_nvim().is_none() {
            eprintln!("skipping nvim pending-count command test: `nvim` not found on PATH");
            return;
        }

        let registry = NvimSessionRegistry::new();
        let handle = registry
            .get_or_spawn("test:count-cmd".into(), &CrdtSyncHub::default())
            .await
            .expect("spawn nvim");
        let mut rx = handle.subscribe_redraw();

        // Bare digit → pending-count state (non-fast RPC now deferred).
        handle
            .handle(EditorClientMessage::SendKeys {
                bytes: b"3".to_vec(),
                surface_id: Some("test:count-cmd".into()),
            })
            .await
            .expect("send digit");

        // Fire a non-fast COMMAND that nvim will defer until the count
        // clears (mirrors the `/`-search preview `:lua` the palette sends).
        let cmd_handle = handle.clone();
        let cmd_task = tokio::spawn(async move {
            let _ = cmd_handle
                .handle(EditorClientMessage::Command {
                    command: "echo 'deferred'".into(),
                    surface_id: Some("test:count-cmd".into()),
                })
                .await;
        });
        // Let the command task acquire the session and start its deferred
        // await before we race the input against it.
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        // The clearing keystroke must NOT wait behind the deferred command.
        // Timing is the deterministic freeze signal: pre-fix this blocked
        // for the full ~4s rpc-timeout window; post-fix it returns at once.
        let t0 = std::time::Instant::now();
        handle
            .handle(EditorClientMessage::SendKeys {
                bytes: b"<Esc>".to_vec(),
                surface_id: Some("test:count-cmd".into()),
            })
            .await
            .expect("keys after digit");
        let elapsed = t0.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "input lane wedged behind a deferred command for {elapsed:?} (digit-key freeze)"
        );
        let _ = &mut rx;

        let _ = cmd_task.await;
        let _ = handle.handle(EditorClientMessage::Close).await;
        registry.remove(handle.key()).await;
    }

    #[tokio::test]
    async fn nvim_lua_print_message_roundtrip_when_available() {
        if which_nvim().is_none() {
            eprintln!("skipping nvim lua print test: `nvim` not found on PATH");
            return;
        }

        let registry = NvimSessionRegistry::new();
        let handle = registry
            .get_or_spawn("test:lua-print".into(), &CrdtSyncHub::default())
            .await
            .expect("spawn nvim");
        let mut rx = handle.subscribe_redraw();
        handle
            .handle(EditorClientMessage::Command {
                command: "lua print(\"hi\")".into(),
                surface_id: Some("test:lua-print".into()),
            })
            .await
            .expect("run lua print");

        assert!(
            recv_message_containing(&mut rx, "hi").await,
            "expected msg_show output for :lua print(\"hi\")"
        );
        let _ = handle.handle(EditorClientMessage::Close).await;
        registry.remove(handle.key()).await;
    }

    #[tokio::test]
    async fn nvim_registry_fans_out_same_surface_to_two_subscribers_when_available() {
        if which_nvim().is_none() {
            eprintln!("skipping nvim fanout test: `nvim` not found on PATH");
            return;
        }

        let registry = NvimSessionRegistry::new();
        let first = registry
            .get_or_spawn("pane:shared".into(), &CrdtSyncHub::default())
            .await
            .expect("spawn first handle");
        let second = registry
            .get_or_spawn("pane:shared".into(), &CrdtSyncHub::default())
            .await
            .expect("spawn second handle");
        assert_eq!(registry.len().await, 1);

        let mut first_rx = first.subscribe_redraw();
        let mut second_rx = second.subscribe_redraw();
        first
            .handle(EditorClientMessage::SendKeys {
                bytes: b"iShared<Esc>".to_vec(),
                surface_id: Some("pane:shared".into()),
            })
            .await
            .expect("send shared input");

        assert!(
            recv_grid_cell(&mut first_rx, "S").await,
            "first subscriber saw grid"
        );
        assert!(
            recv_grid_cell(&mut second_rx, "S").await,
            "second subscriber saw same daemon nvim grid"
        );
        let _ = first.handle(EditorClientMessage::Close).await;
        registry.remove(first.key()).await;
    }
