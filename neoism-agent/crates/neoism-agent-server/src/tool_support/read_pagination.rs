//! Bounded read pagination. Random opaque cursors are process-local and tied
//! to the opened file's identity/version; they are not portable bookmarks.
use anyhow::Context;
use base64::Engine;
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::fs::{File, Metadata};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Debug)]
pub(super) struct Position {
    pub byte: u64,
    pub line: usize,
    pub column: u64,
}

pub(super) fn version(path: &Path, metadata: &Metadata) -> String {
    // Debug's SystemTime includes nanoseconds. Unix ctime also detects writes
    // that restore mtime, and dev/ino detect replacement with same-size content.
    let mut identity = format!(
        "{:?}:{:?}:{:?}:{}",
        path,
        metadata.modified(),
        metadata.created(),
        metadata.len()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        identity.push_str(&format!(
            ":{}:{}:{}:{}",
            metadata.dev(),
            metadata.ino(),
            metadata.ctime(),
            metadata.ctime_nsec()
        ));
    }
    format!("{:x}", Sha256::digest(identity.as_bytes()))
}

pub(super) const CURSOR_LENGTH: usize = 43; // unpadded base64url of 256 random bits
const MAX_CURSORS: usize = 4096;
type CursorCache = VecDeque<(String, String, Position)>;

fn cursors() -> &'static Mutex<CursorCache> {
    static CURSORS: OnceLock<Mutex<CursorCache>> = OnceLock::new();
    CURSORS.get_or_init(|| Mutex::new(VecDeque::new()))
}

pub(super) fn encode(version: &str, position: &Position) -> String {
    let token = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(rand::random::<[u8; 32]>());
    let mut cache = cursors()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    if cache.len() == MAX_CURSORS {
        cache.pop_front();
    }
    cache.push_back((token.clone(), version.to_owned(), position.clone()));
    token
}

pub(super) fn decode(token: &str, expected: &str, size: u64) -> anyhow::Result<Position> {
    anyhow::ensure!(token.len() == CURSOR_LENGTH, "invalid read cursor");
    // No encoded positions or homemade MAC: only a server-generated random
    // capability can select a saved position. The bounded cache retains the
    // most recently used 4096 cursors; restart/eviction requires a fresh read.
    let mut cache = cursors()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let index = cache
        .iter()
        .position(|(saved, _, _)| saved == token)
        .ok_or_else(|| {
            anyhow::anyhow!("invalid or expired read cursor; read again without cursor")
        })?;
    let (_, saved_version, position) = &cache[index];
    anyhow::ensure!(
        saved_version == expected,
        "stale or cross-file read cursor; read the file again without cursor"
    );
    anyhow::ensure!(
        position.line > 0 && position.byte <= size && position.column <= position.byte,
        "invalid read cursor position"
    );
    let entry = cache.remove(index).expect("cursor cache index");
    let position = entry.2.clone();
    cache.push_back(entry);
    Ok(position)
}

#[derive(Debug)]
pub(super) struct Page {
    pub rendered: Vec<String>,
    pub preview: Vec<String>,
    pub start: Position,
    pub next: Position,
    pub more: bool,
    pub byte_capped: bool,
    pub total_lines: usize,
}

/// No allocation depends on a physical line's length. The byte budget includes
/// labels/separators, but not the surrounding tool envelope or instructions.
pub(super) fn page(
    file: File,
    offset: usize,
    limit: usize,
    cursor: Option<Position>,
    cap: usize,
    check: impl Fn() -> anyhow::Result<()>,
) -> anyhow::Result<Page> {
    check()?;
    let mut reader = BufReader::with_capacity(8192, file);
    let mut pos = cursor.unwrap_or(Position {
        byte: 0,
        line: 1,
        column: 0,
    });
    reader.seek(SeekFrom::Start(pos.byte))?;
    // Offset scanning uses fixed-size buffers, and checks cancellation for
    // every buffer, including while skipping a multi-gigabyte single line.
    while pos.line < offset {
        check()?;
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            if pos.column > 0 && pos.line.saturating_add(1) == offset {
                // One-past-EOF is allowed for an unterminated final line too.
                pos.line = offset;
                pos.column = 0;
            }
            anyhow::ensure!(
                pos.line == offset,
                "offset {offset} is out of range ({} lines)",
                pos.line.saturating_sub(usize::from(pos.column == 0))
            );
            break;
        }
        let newline = buffer.iter().position(|b| *b == b'\n');
        let count = newline.map_or(buffer.len(), |i| i + 1);
        reader.consume(count);
        pos.byte += count as u64;
        if newline.is_some() {
            pos.line += 1;
            pos.column = 0;
        } else {
            pos.column += count as u64;
        }
    }
    let start = pos.clone();
    let mut rendered = Vec::new();
    let mut preview = Vec::new();
    let mut used = 0;
    let mut byte_capped = false;
    while rendered.len() < limit {
        check()?;
        if reader.fill_buf()?.is_empty() {
            break;
        }
        let label = format!("{}: ", pos.line);
        let overhead = label.len() + usize::from(!rendered.is_empty());
        let budget = cap.saturating_sub(used + overhead);
        if budget < 4 {
            byte_capped = true;
            break;
        }
        let mut raw = Vec::with_capacity(budget);
        let mut newline = false;
        while raw.len() < budget {
            check()?;
            let buffer = reader.fill_buf()?;
            if buffer.is_empty() {
                break;
            }
            let available = buffer.len().min(budget - raw.len());
            let chunk = &buffer[..available];
            if let Some(index) = chunk.iter().position(|byte| *byte == b'\n') {
                raw.extend_from_slice(&chunk[..index]);
                reader.consume(index + 1);
                newline = true;
                break;
            }
            raw.extend_from_slice(chunk);
            reader.consume(available);
        }
        // If the budget ended immediately before LF, consume it: exact-cap
        // lines must not produce a spurious empty fragment or continuation.
        let mut delimiter_cr = false;
        if !newline && reader.fill_buf()?.first() == Some(&b'\n') {
            reader.consume(1);
            newline = true;
        } else if !newline && reader.fill_buf()?.first() == Some(&b'\r') {
            // CRLF can fall entirely just beyond an exactly full content
            // budget. Probe at most two bytes, restoring a non-delimiter CR.
            reader.consume(1);
            if reader.fill_buf()?.first() == Some(&b'\n') {
                reader.consume(1);
                newline = true;
                delimiter_cr = true;
            } else {
                reader.seek(SeekFrom::Current(-1))?;
            }
        }
        let mut consumed = raw.len();
        if newline && !delimiter_cr && raw.last() == Some(&b'\r') {
            raw.pop();
        }
        if !newline && !reader.fill_buf()?.is_empty() {
            // Preserve a boundary CR for the next page so CRLF is normalized
            // even when the byte budget falls between CR and LF.
            if raw.last() == Some(&b'\r') {
                raw.pop();
            }
            match std::str::from_utf8(&raw) {
                Ok(_) => {}
                Err(error) if error.error_len().is_none() => {
                    raw.truncate(error.valid_up_to());
                }
                Err(error) => {
                    return Err(error)
                        .context(format!("not valid UTF-8 near line {}", pos.line))
                }
            }
            let rewind = consumed - raw.len();
            if rewind > 0 {
                reader.seek(SeekFrom::Current(-(rewind as i64)))?;
                consumed -= rewind;
            }
        }
        let text = std::str::from_utf8(&raw)
            .with_context(|| format!("not valid UTF-8 near line {}", pos.line))?;
        if preview.len() < 20 {
            preview.push(text.to_owned());
        }
        rendered.push(format!("{label}{text}"));
        used += overhead + raw.len();
        pos.byte += consumed as u64 + u64::from(newline) + u64::from(delimiter_cr);
        if newline {
            pos.line += 1;
            pos.column = 0;
        } else {
            pos.column += consumed as u64;
        }
        if !newline {
            byte_capped = !reader.fill_buf()?.is_empty();
            break;
        }
    }
    check()?;
    let more = !reader.fill_buf()?.is_empty();
    let total_lines = pos.line.saturating_sub(usize::from(pos.column == 0));
    Ok(Page {
        rendered,
        preview,
        start,
        next: pos,
        more,
        byte_capped: byte_capped && more,
        total_lines,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn read_context(path: impl AsRef<Path>) -> super::super::ToolContext {
        super::super::ToolContext::new(path.as_ref()).with_permission_rules(vec![
            neoism_agent_core::PermissionRule {
                permission: "read".into(),
                pattern: "*".into(),
                action: neoism_agent_core::PermissionAction::Allow,
            },
        ])
    }

    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new(bytes: &[u8]) -> Self {
            let path = std::env::temp_dir().join(format!(
                "neoism-read-{}-{}",
                std::process::id(),
                rand::random::<u64>()
            ));
            std::fs::write(&path, bytes).unwrap();
            Self(path)
        }
        fn open(&self) -> File {
            File::open(&self.0).unwrap()
        }
        fn version(&self) -> String {
            version(&self.0, &self.open().metadata().unwrap())
        }
        fn page(
            &self,
            offset: usize,
            limit: usize,
            position: Option<Position>,
            cap: usize,
        ) -> Page {
            page(self.open(), offset, limit, position, cap, || Ok(())).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            if self.0.is_dir() {
                let _ = std::fs::remove_dir_all(&self.0);
            } else {
                let _ = std::fs::remove_file(&self.0);
            }
        }
    }

    #[test]
    fn read_long_unicode_cursor_roundtrip_no_gaps_with_unchanged_limit() {
        let text = format!("{}\r\nnext\r\nlast", "aé界🦀".repeat(25000));
        let fixture = Fixture::new(text.as_bytes());
        let version = fixture.version();
        let mut position = None;
        let mut recovered: std::collections::BTreeMap<usize, String> = Default::default();
        let mut pages = 0;
        loop {
            let page = fixture.page(1, 1, position, 50 * 1024);
            assert!(page.rendered.join("\n").len() <= 50 * 1024);
            for line in &page.rendered {
                let (number, fragment) = line.split_once(": ").unwrap();
                recovered
                    .entry(number.parse().unwrap())
                    .or_default()
                    .push_str(fragment);
            }
            pages += 1;
            if !page.more {
                assert_eq!(page.total_lines, 3);
                break;
            }
            assert!(page.next.byte > page.start.byte);
            position = Some(
                decode(&encode(&version, &page.next), &version, text.len() as u64)
                    .unwrap(),
            );
            assert!(pages < 20);
        }
        assert!(pages > 3);
        assert_eq!(
            recovered.into_values().collect::<Vec<_>>().join("\n"),
            text.replace("\r\n", "\n")
        );
    }

    #[test]
    fn read_crlf_exact_cap_and_boundaries() {
        for tail in ["", "\n", "\r\n"] {
            let fixture = Fixture::new(format!("{}{tail}", "x".repeat(61)).as_bytes());
            let page = fixture.page(1, 10, None, 64);
            assert_eq!(page.rendered, vec![format!("1: {}", "x".repeat(61))]);
            assert!(!page.more);
            assert!(!page.byte_capped);
            assert_eq!(page.total_lines, 1);
        }
        let fixture = Fixture::new(format!("{}\r\nend", "x".repeat(60)).as_bytes());
        let first = fixture.page(1, 1, None, 64);
        assert_eq!(first.next.line, 2);
        assert_eq!(first.next.column, 0);
        assert_eq!(
            fixture.page(1, 1, Some(first.next), 64).rendered,
            vec!["2: end"]
        );
    }

    #[test]
    fn read_offsets_empty_lines_and_one_past_eof() {
        let fixture = Fixture::new(b"first\r\n\r\nthird\nlast");
        let page = fixture.page(2, 2, None, 1024);
        assert_eq!(page.rendered, vec!["2: ", "3: third"]);
        assert!(page.more);
        assert!(!page.byte_capped);
        let final_page = fixture.page(1, 2, Some(page.next), 1024);
        assert_eq!(final_page.rendered, vec!["4: last"]);
        assert_eq!(final_page.total_lines, 4);
        assert!(fixture.page(5, 2, None, 1024).rendered.is_empty());
        assert!(page_result(&fixture, 6).is_err());
        let empty = Fixture::new(b"");
        assert_eq!(empty.page(1, 1, None, 1024).total_lines, 0);
        assert!(page_result(&empty, 2).is_err());
    }
    fn page_result(fixture: &Fixture, offset: usize) -> anyhow::Result<Page> {
        page(fixture.open(), offset, 1, None, 1024, || Ok(()))
    }

    #[test]
    fn read_rejects_stale_cross_file_and_tampered_cursors() {
        let fixture = Fixture::new(b"original");
        let other = Fixture::new(b"original");
        let token = encode(
            &fixture.version(),
            &Position {
                byte: 3,
                line: 1,
                column: 3,
            },
        );
        assert!(decode(&token, &other.version(), 8).is_err());
        std::fs::write(&fixture.0, b"changed and longer").unwrap();
        assert!(decode(&token, &fixture.version(), 18).is_err());
        let mut tampered = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&token)
            .unwrap();
        tampered[0] ^= 1;
        assert!(decode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(tampered),
            &fixture.version(),
            18
        )
        .is_err());
        assert!(decode("bad cursor", &fixture.version(), 18).is_err());
    }

    #[test]
    fn read_cancellation_during_scan_and_long_line() {
        let fixture = Fixture::new(&vec![b'x'; 500_000]);
        for offset in [1, 2] {
            let calls = Cell::new(0);
            let result = page(fixture.open(), offset, 1, None, 50 * 1024, || {
                calls.set(calls.get() + 1);
                anyhow::ensure!(calls.get() < 5, "read cancelled");
                Ok(())
            });
            assert!(result.unwrap_err().to_string().contains("cancelled"));
        }
    }

    #[test]
    fn read_tool_cursor_metadata_and_argument_validation() {
        use super::super::read_tool;
        use serde_json::json;
        let fixture = Fixture::new("界".repeat(30000).as_bytes());
        let context = || read_context(fixture.0.parent().unwrap());
        let first =
            read_tool(context(), json!({"filePath": fixture.0, "limit": 1})).unwrap();
        let metadata = first.metadata.unwrap();
        assert_eq!(metadata["truncated"], true);
        assert_eq!(metadata["byteCapped"], true);
        assert_eq!(metadata["partialLine"], true);
        assert!(first.output.contains("Continue with read arguments:"));
        assert!(metadata["nextCursor"].as_str().is_some());
        let last = read_tool(context(), metadata["continuation"].clone()).unwrap();
        for offset in [json!(null), json!(1), json!(42)] {
            let mut args = metadata["continuation"].clone();
            args["offset"] = offset;
            let continued = read_tool(context(), args).unwrap();
            assert_eq!(continued.output, last.output);
            assert_eq!(continued.metadata, last.metadata);
        }
        let last_metadata = last.metadata.unwrap();
        assert_eq!(last_metadata["truncated"], false);
        assert_eq!(last_metadata["lines"], 1);
        assert!(last_metadata["nextCursor"].is_null());
        let cancelled = context().with_cancel(Some(std::sync::Arc::new(
            std::sync::atomic::AtomicBool::new(true),
        )));
        assert!(read_tool(cancelled, json!({"filePath": fixture.0})).is_err());
    }

    #[tokio::test]
    async fn read_accepts_empty_cursor_with_offsets_through_dispatch() {
        use crate::tool;
        use serde_json::json;
        let root = Fixture::new(b"");
        std::fs::remove_file(&root.0).unwrap();
        let directory = root.0.join("data");
        std::fs::create_dir_all(&directory).unwrap();
        let file = directory.join("a.txt");
        std::fs::write(&file, "first\nsecond\nthird\n").unwrap();
        std::fs::write(directory.join("b.txt"), "other").unwrap();
        let state = crate::state::AppState::open_database(root.0.join("agent.db"))
            .await
            .unwrap();
        let context = || read_context(&root.0).with_state(Some(state.clone()));
        for cursor in ["", " \t"] {
            let result = tool::execute(
                "read",
                context(),
                json!({"filePath": file, "offset": 2, "limit": 1, "cursor": cursor}),
            )
            .await
            .unwrap();
            assert!(result.output.contains("2: second"));
            assert!(!result.output.contains("1: first"));
            let mut continuation = result.metadata.unwrap()["continuation"].clone();
            continuation["offset"] = json!(1);
            let next = tool::execute("read", context(), continuation)
                .await
                .unwrap();
            assert!(next.output.contains("3: third"));
            assert!(!next.output.contains("1: first"));
            let entries = tool::execute(
                "read",
                context(),
                json!({"filePath": directory, "offset": 2, "limit": 1, "cursor": cursor}),
            )
            .await
            .unwrap();
            assert!(entries.output.contains("b.txt"));
            assert!(!entries.output.contains("a.txt"));
        }
        let result = super::super::read_tool(
            context(),
            json!({"filePath": file, "offset": 2, "limit": 1, "cursor": null}),
        )
        .unwrap();
        assert!(result.output.contains("2: second"));
    }

    #[test]
    fn read_tool_full_page_fits_outer_budgets_with_instructions() {
        use super::super::super::truncate;
        use super::super::read_tool;
        use serde_json::json;
        let fixture = Fixture::new(b"");
        std::fs::remove_file(&fixture.0).unwrap();
        std::fs::create_dir_all(fixture.0.join("nested")).unwrap();
        let text = "é界🦀".repeat(30000);
        let instructions = "Keep every instruction.\n".repeat(100);
        std::fs::write(fixture.0.join("nested/AGENTS.md"), &instructions).unwrap();
        let path = fixture.0.join("nested/full.txt");
        std::fs::write(&path, &text).unwrap();
        let mut args = json!({"filePath": path});
        let mut recovered = String::new();
        let mut pages = 0;
        loop {
            let result = read_tool(read_context(&fixture.0), args).unwrap();
            assert!(result.output.len() <= truncate::MAX_BYTES);
            assert!(result.output.lines().count() <= truncate::MAX_LINES);
            assert!(!truncate::truncate_output(&result.output).unwrap().truncated);
            assert!(result.output.contains(instructions.trim()));
            let metadata = result.metadata.unwrap();
            assert_eq!(metadata["instructionOverflow"], false);
            assert_eq!(metadata["limit"], 2000);
            // Ordinary paths reserve only actual framing/instruction overhead.
            assert!(metadata["contentByteBudget"].as_u64().unwrap() > 45_000);
            let content = result
                .output
                .split_once("<content>\n")
                .unwrap()
                .1
                .split_once("\n\n(")
                .unwrap()
                .0;
            recovered.push_str(content.strip_prefix("1: ").unwrap());
            pages += 1;
            if metadata["truncated"] == false {
                break;
            }
            args = metadata["continuation"].clone();
            assert!(args.get("offset").is_none());
            assert!(pages < 20);
        }
        assert!(pages > 1);
        assert_eq!(recovered, text);
    }

    #[test]
    fn read_tool_default_line_limit_reserves_envelope_rows_without_loss() {
        use super::super::super::truncate;
        use super::super::read_tool;
        use serde_json::json;
        let fixture = Fixture::new(&vec![b'\n'; 5000]);
        let mut args = json!({"filePath": fixture.0});
        let mut next_expected = 1usize;
        let mut pages = 0;
        loop {
            let result =
                read_tool(read_context(fixture.0.parent().unwrap()), args).unwrap();
            assert!(result.output.len() <= truncate::MAX_BYTES);
            assert!(result.output.lines().count() <= truncate::MAX_LINES);
            assert!(!truncate::truncate_output(&result.output).unwrap().truncated);
            let metadata = result.metadata.unwrap();
            assert_eq!(metadata["limit"], 2000);
            let content = result
                .output
                .split_once("<content>\n")
                .unwrap()
                .1
                .split_once("\n\n(")
                .unwrap()
                .0;
            for row in content.lines() {
                assert_eq!(row, format!("{next_expected}: "));
                next_expected += 1;
            }
            pages += 1;
            if metadata["truncated"] == false {
                assert_eq!(metadata["lines"], 5000);
                break;
            }
            assert_eq!(metadata["rowCapped"], true);
            args = metadata["continuation"].clone();
            assert!(pages < 5);
        }
        assert_eq!(next_expected, 5001);
        assert_eq!(pages, 3);
    }

    #[test]
    fn read_output_budget_handles_escaped_paths_and_oversized_instructions() {
        use super::super::super::truncate;
        use super::super::{read_continuation_footer, read_output_budget};
        use serde_json::json;
        let prefix = "<path>path\nwith newline</path>\n<type>file</type>\n<content>\n";
        let path = "path\nwith \\\"quotes\\\" and \\ backslash";
        let reminder = "\n\n<system-reminder>\nkeep\nall\nthese\n</system-reminder>";
        let (bytes, rows, overflow) =
            read_output_budget(prefix, path, 2000, reminder).unwrap();
        assert!(!overflow);
        let footer = read_continuation_footer(
            &json!({"filePath":path,"limit":2000,"cursor":"x".repeat(CURSOR_LENGTH)}),
            usize::MAX,
            true,
        );
        let output = format!(
            "{prefix}{}{footer}\n</content>{reminder}",
            "x".repeat(bytes)
        );
        assert_eq!(output.len(), truncate::MAX_BYTES);
        let output = format!(
            "{prefix}{}{footer}\n</content>{reminder}",
            vec!["1: "; rows].join("\n")
        );
        assert!(output.lines().count() <= truncate::MAX_LINES);
        for oversized in [
            "x".repeat(truncate::MAX_BYTES),
            "instruction\n".repeat(truncate::MAX_LINES),
        ] {
            let (_, _, overflow) =
                read_output_budget(prefix, path, 2000, &oversized).unwrap();
            assert!(overflow);
        }
    }

    #[test]
    fn read_tool_retains_exceptionally_large_nearby_instructions() {
        use super::super::super::truncate;
        use super::super::read_tool;
        let fixture = Fixture::new(b"");
        std::fs::remove_file(&fixture.0).unwrap();
        std::fs::create_dir_all(fixture.0.join("nested")).unwrap();
        let instructions = "x".repeat(truncate::MAX_BYTES + 1);
        std::fs::write(fixture.0.join("nested/AGENTS.md"), &instructions).unwrap();
        std::fs::write(fixture.0.join("nested/data.txt"), "content").unwrap();
        let result = read_tool(
            read_context(&fixture.0),
            serde_json::json!({"filePath":fixture.0.join("nested/data.txt")}),
        )
        .unwrap();
        assert!(result.output.contains(&instructions));
        assert!(result.output.contains("1: content"));
        assert_eq!(result.metadata.unwrap()["instructionOverflow"], true);
    }

    #[test]
    fn read_directory_keeps_offset_pagination() {
        use super::super::read_tool;
        use serde_json::json;
        let fixture = Fixture::new(b"");
        std::fs::remove_file(&fixture.0).unwrap();
        std::fs::create_dir(&fixture.0).unwrap();
        std::fs::write(fixture.0.join("a"), b"a").unwrap();
        std::fs::write(fixture.0.join("b"), b"b").unwrap();
        let context = || read_context(&fixture.0);
        let first =
            read_tool(context(), json!({"filePath": fixture.0, "limit": 1})).unwrap();
        assert!(first.output.contains("Use offset=2"));
        assert_eq!(first.metadata.unwrap()["truncated"], true);
        let second = read_tool(
            context(),
            json!({"filePath": fixture.0, "limit": 1, "offset": 2}),
        )
        .unwrap();
        assert_eq!(second.metadata.unwrap()["truncated"], false);
        assert!(
            read_tool(context(), json!({"filePath": fixture.0, "cursor": "bad"}))
                .is_err()
        );
    }

    #[test]
    fn read_media_is_bounded_and_keeps_attachment_behavior() {
        use super::super::{read_tool, MAX_MEDIA_READ_BYTES};
        use serde_json::json;
        let fixture = Fixture::new(b"%PDF-small attachment");
        let context = || read_context(fixture.0.parent().unwrap());
        let result = read_tool(context(), json!({"filePath": fixture.0})).unwrap();
        let metadata = result.metadata.unwrap();
        assert_eq!(metadata["mime"], "application/pdf");
        assert_eq!(metadata["truncated"], false);
        assert!(metadata["attachments"][0]["url"]
            .as_str()
            .unwrap()
            .starts_with("data:application/pdf;base64,"));
        std::fs::OpenOptions::new()
            .write(true)
            .open(&fixture.0)
            .unwrap()
            .set_len(MAX_MEDIA_READ_BYTES as u64 + 1)
            .unwrap();
        assert!(read_tool(context(), json!({"filePath": fixture.0})).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn read_rejects_non_regular_files() {
        use super::super::read_tool;
        assert!(read_tool(
            read_context("/dev"),
            serde_json::json!({"filePath": "/dev/null"})
        )
        .is_err());
    }

    #[test]
    fn read_rejects_invalid_utf8_including_incomplete_eof() {
        for bytes in [&b"good\n\xffbad"[..], &b"abc\xf0\x9f"[..], &b"abc\xe2z"[..]] {
            let fixture = Fixture::new(bytes);
            assert!(page(fixture.open(), 1, 10, None, 1024, || Ok(())).is_err());
        }
        let fixture = Fixture::new(&[b'x', b'x', b'x', b'x', b'x', 0xff, b'x', b'x']);
        assert!(page(fixture.open(), 1, 10, None, 10, || Ok(())).is_err());
    }
}
