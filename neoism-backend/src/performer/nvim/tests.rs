    use super::*;

    #[test]
    fn vim_edit_command_quotes_for_lua() {
        // Plain ASCII passes through.
        assert_eq!(vim_edit_command("foo.rs"), r#"lua vim.cmd.edit("foo.rs")"#);
        // Apostrophes survive (lua double-quoted string), backslashes
        // and double-quotes are escaped so the command stays parseable.
        assert_eq!(
            vim_edit_command(r#"a"b\c"#),
            r#"lua vim.cmd.edit("a\"b\\c")"#
        );

        let select_cmd = vim_select_file_command(r#"a"b\c"#);
        assert!(select_cmd.contains(r#"local path = "a\"b\\c""#));
        assert!(select_cmd.contains("vim.o.hidden = true"));
        assert!(select_cmd.contains("vim.api.nvim_set_current_buf(buf)"));
        assert!(select_cmd.contains("vim.cmd.edit({ args = { target } })"));
        assert!(select_cmd.contains("File Open Failed"));
    }

    #[test]
    fn theme_command_surfaces_failures_and_quotes_the_name() {
        let cmd = vim_apply_theme_command(r#"tokyo"night"#);
        assert_eq!(
            cmd,
            r#"lua require('rio.theme').apply("tokyo\"night")"#
        );
        assert!(
            !cmd.contains("pcall"),
            "theme failures must reach the managed RPC error log"
        );
    }

    #[test]
    fn build_command_uses_nvim_by_default() {
        let cfg = NvimSpawnConfig::default();
        let cmd = build_nvim_command(&cfg);
        // tokio::process::Command Debug prints program path; this is
        // brittle but informative — if nvim-rs's launching changes,
        // catch it here.
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("nvim"));
        assert!(dbg.contains("--embed"));
    }

    #[test]
    fn resize_watch_channel_keeps_only_the_latest_geometry() {
        let (tx, mut rx) = tokio_watch::channel((80_u64, 24_u64));
        tx.send((100, 30)).unwrap();
        tx.send((120, 36)).unwrap();
        tx.send((160, 48)).unwrap();

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            rx.changed().await.unwrap();
            assert_eq!(*rx.borrow_and_update(), (160, 48));
        });
    }

    #[test]
    fn skips_transient_bufwrite_message() {
        fn msg_param(kind: &str, text: &str, history: bool) -> Value {
            Value::Array(vec![
                Value::from(kind),
                Value::Array(vec![Value::Array(vec![
                    Value::from(0),
                    Value::from(text),
                    Value::from(0),
                ])]),
                Value::from(false),
                Value::from(history),
                Value::from(false),
                Value::from(1),
                Value::from(""),
            ])
        }

        let raw = Value::Array(vec![
            Value::from("msg_show"),
            msg_param("bufwrite", "\"a.py\" ", false),
            msg_param("bufwrite", "\"a.py\" 1L, 9B [w]", true),
        ]);

        let messages = parse_external_messages(&raw);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].text, "\"a.py\" 1L, 9B [w]");
    }

    #[test]
    fn parses_showcmd_updates() {
        fn showcmd(chunks: &[&str]) -> Value {
            Value::Array(vec![Value::Array(vec![Value::Array(
                chunks
                    .iter()
                    .map(|text| {
                        Value::Array(vec![Value::from(0), Value::from(*text)])
                    })
                    .collect(),
            )])])
        }

        // Pending count "2d" — chunks concatenate.
        let mut raw = vec![Value::from("msg_showcmd")];
        raw.extend(showcmd(&["2", "d"]).as_array().unwrap().iter().cloned());
        assert_eq!(parse_showcmd(&Value::Array(raw)).as_deref(), Some("2d"));

        // Empty content = cleared (count consumed / cancelled).
        let cleared = Value::Array(vec![
            Value::from("msg_showcmd"),
            Value::Array(vec![Value::Array(vec![])]),
        ]);
        assert_eq!(parse_showcmd(&cleared).as_deref(), Some(""));

        // Unrelated events are ignored.
        let other = Value::Array(vec![Value::from("msg_show")]);
        assert_eq!(parse_showcmd(&other), None);
    }
