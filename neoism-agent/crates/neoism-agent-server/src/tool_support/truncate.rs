use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub(crate) const MAX_LINES: usize = 2000;
pub(crate) const MAX_BYTES: usize = 51_200;
const RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TruncatedOutput {
    pub(crate) output: String,
    pub(crate) truncated: bool,
    pub(crate) output_path: Option<PathBuf>,
}

pub(crate) fn truncate_output(text: &str) -> std::io::Result<TruncatedOutput> {
    if fits(text) {
        return Ok(TruncatedOutput {
            output: text.to_string(),
            truncated: false,
            output_path: None,
        });
    }

    let preview = head_tail(text, MAX_LINES, MAX_BYTES);
    let output_path = write_full_output(text)?;
    let output = format!(
        "...output truncated...\n\nThe tool call succeeded but the output was truncated. Full output saved to: {}\nUse Grep to search the full content or Read with offset/limit to view specific sections.\n\n{}",
        output_path.display(),
        preview
    );

    Ok(TruncatedOutput {
        output,
        truncated: true,
        output_path: Some(output_path),
    })
}

fn fits(text: &str) -> bool {
    text.lines().count() <= MAX_LINES && text.len() <= MAX_BYTES
}

fn head_tail(text: &str, max_lines: usize, max_bytes: usize) -> String {
    let lines = text.lines().collect::<Vec<_>>();
    let head_target = (max_lines / 2).max(1);
    let tail_target = max_lines.saturating_sub(head_target).max(1);
    let byte_half = (max_bytes / 2).max(1);
    let head = take_head(&lines, head_target, byte_half);
    let tail = take_tail(
        &lines,
        tail_target,
        max_bytes.saturating_sub(bytes_len(&head)),
    );
    if head.len() + tail.len() >= lines.len() {
        return lines.join("\n");
    }
    let omitted = lines.len().saturating_sub(head.len() + tail.len());
    format!(
        "{}\n\n[... omitted {omitted} lines ...]\n\n{}",
        head.join("\n"),
        tail.join("\n")
    )
}

fn take_head<'a>(lines: &'a [&str], max_lines: usize, max_bytes: usize) -> Vec<&'a str> {
    let mut output = Vec::new();
    let mut bytes = 0;
    for line in lines {
        let size = line.len() + usize::from(!output.is_empty());
        if output.len() >= max_lines || bytes + size > max_bytes {
            break;
        }
        output.push(*line);
        bytes += size;
    }
    output
}

fn take_tail<'a>(lines: &'a [&str], max_lines: usize, max_bytes: usize) -> Vec<&'a str> {
    let mut output = Vec::new();
    let mut bytes = 0;
    for line in lines.iter().rev() {
        let size = line.len() + usize::from(!output.is_empty());
        if output.len() >= max_lines || bytes + size > max_bytes {
            break;
        }
        output.push(*line);
        bytes += size;
    }
    output.reverse();
    output
}

fn bytes_len(lines: &[&str]) -> usize {
    lines.iter().map(|line| line.len()).sum::<usize>() + lines.len().saturating_sub(1)
}

fn write_full_output(text: &str) -> std::io::Result<PathBuf> {
    let _ = cleanup_old_outputs(truncation_dir());
    write_full_output_in(truncation_dir(), text).or_else(|_| {
        write_full_output_in(
            std::env::temp_dir()
                .join("neoism-agent-state")
                .join("tool-output"),
            text,
        )
    })
}

fn write_full_output_in(dir: PathBuf, text: &str) -> std::io::Result<PathBuf> {
    fs::create_dir_all(&dir)?;
    let id = neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event);
    let path = dir.join(format!("tool-{id}.txt"));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    file.write_all(text.as_bytes())?;
    Ok(path)
}

fn truncation_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache"))
        })
        .unwrap_or_else(|| std::env::temp_dir().join("neoism-agent-cache"))
        .join("neoism")
        .join("tool-output")
}

fn cleanup_old_outputs(dir: PathBuf) -> std::io::Result<()> {
    let cutoff = SystemTime::now()
        .checked_sub(RETENTION)
        .unwrap_or(UNIX_EPOCH);
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        if modified < cutoff {
            let _ = fs::remove_file(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_small_output_unchanged() {
        let output = truncate_output("hello").unwrap();
        assert_eq!(output.output, "hello");
        assert!(!output.truncated);
        assert!(output.output_path.is_none());
    }

    #[test]
    fn truncates_large_output_and_saves_full_text() {
        let text = (0..=MAX_LINES + 5)
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let output = truncate_output(&text).unwrap();
        assert!(output.truncated);
        assert!(output.output.contains("...output truncated..."));
        assert!(output.output.contains("0"));
        assert!(output.output.contains(&(MAX_LINES + 5).to_string()));
        let path = output.output_path.expect("full output path");
        assert_eq!(fs::read_to_string(&path).unwrap(), text);
        let _ = fs::remove_file(path);
    }
}
