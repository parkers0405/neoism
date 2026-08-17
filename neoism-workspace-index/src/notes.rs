use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::frontmatter::parse_frontmatter;

use super::config::{NeoismWorkspace, NotesConfig};

pub const SCAN_LIMIT: usize = 5000;

const BUILT_IN_IGNORES: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    ".direnv",
    ".next",
    ".claude",
    ".codex",
    ".neoism/cache",
    "node_modules",
    "target",
    "dist",
    "build",
];

#[derive(Debug, Clone, Default)]
pub struct WorkspaceNoteIndex {
    pub notes: Vec<NoteEntry>,
    pub blocks: Vec<BlockEntry>,
    pub headings: Vec<HeadingEntry>,
    pub links: Vec<LinkEntry>,
    pub tags: Vec<TagEntry>,
    pub tasks: Vec<TaskEntry>,
    pub properties: Vec<PropertyEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteEntry {
    pub path: PathBuf,
    pub relative_path: String,
    pub title: String,
    pub modified: i64,
    pub hash: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockEntry {
    pub note_path: String,
    pub kind: BlockKind,
    pub start_line: usize,
    pub end_line: usize,
    pub ordinal: usize,
    pub text: String,
    pub anchor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockKind {
    Heading,
    Paragraph,
    Task,
    Code,
    Quote,
    Divider,
}

impl BlockKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Heading => "heading",
            Self::Paragraph => "paragraph",
            Self::Task => "task",
            Self::Code => "code",
            Self::Quote => "quote",
            Self::Divider => "divider",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadingEntry {
    pub note_path: String,
    pub line: usize,
    pub level: u8,
    pub text: String,
    pub slug: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkKind {
    Note,
    CodeRef,
    Embed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkEntry {
    pub source_path: String,
    pub source_line: usize,
    pub raw: String,
    pub target: String,
    pub heading: Option<String>,
    pub alias: Option<String>,
    pub kind: LinkKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagEntry {
    pub note_path: String,
    pub line: usize,
    pub tag: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskEntry {
    pub note_path: String,
    pub line: usize,
    pub checked: bool,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyEntry {
    pub note_path: String,
    pub key: String,
    pub value: String,
    pub value_type: String,
}

impl WorkspaceNoteIndex {
    pub fn build(workspace: &NeoismWorkspace) -> std::io::Result<Self> {
        let mut index = Self::default();
        if !workspace.config.notes.enabled {
            return Ok(index);
        }
        let mut files = Vec::new();
        collect_note_files(workspace, &mut files, SCAN_LIMIT)?;
        for path in files {
            index_note_file(workspace, &path, &mut index)?;
        }
        index
            .notes
            .sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
        Ok(index)
    }

    pub fn build_file(
        workspace: &NeoismWorkspace,
        path: impl AsRef<Path>,
    ) -> std::io::Result<Option<Self>> {
        if !workspace.config.notes.enabled {
            return Ok(None);
        }
        let path = workspace.resolve_note_path(path.as_ref());
        if !path.is_file()
            || !is_markdown_path(&path)
            || !is_in_note_roots(workspace, &path)
            || is_ignored(&workspace.root, &path, &workspace.config.notes)
        {
            return Ok(None);
        }
        let mut index = Self::default();
        index_note_file(workspace, &path, &mut index)?;
        Ok(Some(index))
    }

    pub fn link_suggestions(
        &self,
        base_dir: &Path,
        current_doc: &Path,
        query: &str,
        limit: usize,
    ) -> Vec<String> {
        let query = markdown_link_match_query(query);
        let query_lower = query.to_ascii_lowercase();
        let mut scored = Vec::new();
        for note in &self.notes {
            if same_path(&note.path, current_doc) {
                continue;
            }
            let target = relative_link_target(base_dir, &note.path);
            if target.is_empty() {
                continue;
            }
            let target_lower = target.to_ascii_lowercase();
            let title_lower = note.title.to_ascii_lowercase();
            let file_lower = note
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(target.as_str())
                .to_ascii_lowercase();
            let score = if query_lower.is_empty() {
                20
            } else if title_lower.starts_with(&query_lower) {
                0
            } else if file_lower.starts_with(&query_lower) {
                1
            } else if target_lower.starts_with(&query_lower) {
                2
            } else if title_lower.contains(&query_lower) {
                3
            } else if file_lower.contains(&query_lower) {
                4
            } else if target_lower.contains(&query_lower) {
                6
            } else {
                continue;
            };
            scored.push((score, target.len(), target));
        }
        scored.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        scored
            .into_iter()
            .take(limit)
            .map(|(_, _, target)| target)
            .collect()
    }

    pub fn heading_suggestions(
        &self,
        base_dir: &Path,
        current_doc: &Path,
        target: Option<&str>,
        query: &str,
        limit: usize,
    ) -> Vec<String> {
        let target = target.unwrap_or_default().trim();
        let Some(note) = self.note_for_link_target(base_dir, current_doc, target) else {
            return Vec::new();
        };
        let query = query.trim();
        let query_lower = query.to_ascii_lowercase();
        let target_prefix = if target.is_empty() && same_path(&note.path, current_doc) {
            String::new()
        } else {
            relative_link_target(base_dir, &note.path)
        };
        let mut scored = self
            .headings
            .iter()
            .filter(|heading| heading.note_path == note.relative_path)
            .filter_map(|heading| {
                let text_lower = heading.text.to_ascii_lowercase();
                let slug_lower = heading.slug.to_ascii_lowercase();
                let score = if query_lower.is_empty() {
                    heading.level as usize + 20
                } else if text_lower.starts_with(&query_lower) {
                    heading.level as usize
                } else if slug_lower.starts_with(&query_lower) {
                    heading.level as usize + 2
                } else if text_lower.contains(&query_lower) {
                    heading.level as usize + 4
                } else if slug_lower.contains(&query_lower) {
                    heading.level as usize + 6
                } else {
                    return None;
                };
                let target = if target_prefix.is_empty() {
                    format!("#{}", heading.text)
                } else {
                    format!("{}#{}", target_prefix, heading.text)
                };
                Some((score, heading.line, target))
            })
            .collect::<Vec<_>>();
        scored.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        scored
            .into_iter()
            .take(limit)
            .map(|(_, _, target)| target)
            .collect()
    }

    fn note_for_link_target(
        &self,
        base_dir: &Path,
        current_doc: &Path,
        target: &str,
    ) -> Option<&NoteEntry> {
        if target.is_empty() {
            return self
                .notes
                .iter()
                .find(|note| same_path(&note.path, current_doc));
        }
        let base = if Path::new(target).is_absolute() {
            PathBuf::from(target)
        } else {
            base_dir.join(target)
        };
        let mut candidates = vec![base.clone()];
        if base.extension().is_none() {
            for ext in ["md", "markdown", "mdx"] {
                let mut with_ext = base.clone();
                with_ext.set_extension(ext);
                candidates.push(with_ext);
            }
        }
        self.notes.iter().find(|note| {
            candidates
                .iter()
                .any(|candidate| same_path(&note.path, candidate))
        })
    }
}

#[derive(Debug, Default)]
struct ParsedNote {
    blocks: Vec<BlockEntry>,
    headings: Vec<HeadingEntry>,
    links: Vec<LinkEntry>,
    tags: Vec<TagEntry>,
    tasks: Vec<TaskEntry>,
    properties: Vec<PropertyEntry>,
}

fn parse_note(relative_path: &str, source: &str) -> ParsedNote {
    let mut parsed = ParsedNote::default();
    let frontmatter_end_line =
        parse_frontmatter(source).map(|frontmatter| {
            parsed
                .properties
                .extend(frontmatter.properties.into_iter().map(|property| {
                    PropertyEntry {
                        note_path: relative_path.to_string(),
                        key: property.key,
                        value: property.value,
                        value_type: property.value_type,
                    }
                }));
            frontmatter.end_line
        });
    let mut in_code = false;
    let mut code_start_line = 0usize;
    let mut code_lines = Vec::new();
    let mut ordinal = 0usize;
    for (line_ix, line) in source.lines().enumerate() {
        let line_no = line_ix + 1;
        if frontmatter_end_line.is_some_and(|end_line| line_no <= end_line) {
            continue;
        }
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            if in_code {
                code_lines.push(line.to_string());
                push_block(
                    &mut parsed.blocks,
                    relative_path,
                    BlockKind::Code,
                    code_start_line,
                    line_no,
                    &mut ordinal,
                    code_lines.join("\n"),
                );
                code_lines.clear();
                in_code = false;
            } else {
                in_code = true;
                code_start_line = line_no;
                code_lines.clear();
                code_lines.push(line.to_string());
            }
            continue;
        }
        if in_code {
            code_lines.push(line.to_string());
            continue;
        }
        if let Some((kind, text)) = block_from_line(trimmed) {
            push_block(
                &mut parsed.blocks,
                relative_path,
                kind,
                line_no,
                line_no,
                &mut ordinal,
                text,
            );
        }
        if let Some((level, text)) = parse_heading(trimmed) {
            parsed.headings.push(HeadingEntry {
                note_path: relative_path.to_string(),
                line: line_no,
                level,
                slug: heading_slug(&text),
                text,
            });
        }
        if let Some((checked, text)) = parse_task(trimmed) {
            parsed.tasks.push(TaskEntry {
                note_path: relative_path.to_string(),
                line: line_no,
                checked,
                text,
            });
        }
        parsed
            .links
            .extend(collect_wiki_links(line).into_iter().map(|link| LinkEntry {
                source_path: relative_path.to_string(),
                source_line: line_no,
                raw: link.raw,
                target: link.target,
                heading: link.heading,
                alias: link.alias,
                kind: link.kind,
            }));
        parsed
            .tags
            .extend(collect_tags(line).into_iter().map(|tag| TagEntry {
                note_path: relative_path.to_string(),
                line: line_no,
                tag,
            }));
    }
    if in_code {
        let end_line = source.lines().count().max(code_start_line);
        push_block(
            &mut parsed.blocks,
            relative_path,
            BlockKind::Code,
            code_start_line,
            end_line,
            &mut ordinal,
            code_lines.join("\n"),
        );
    }
    parsed
}

fn block_from_line(trimmed: &str) -> Option<(BlockKind, String)> {
    if trimmed.is_empty() {
        return None;
    }
    if let Some((_level, text)) = parse_heading_raw(trimmed) {
        return Some((BlockKind::Heading, text));
    }
    if let Some((_checked, text)) = parse_task_raw(trimmed) {
        return Some((BlockKind::Task, text));
    }
    if trimmed.starts_with('>') {
        return Some((
            BlockKind::Quote,
            trimmed.trim_start_matches('>').trim_start().to_string(),
        ));
    }
    if is_divider(trimmed) {
        return Some((BlockKind::Divider, trimmed.to_string()));
    }
    Some((BlockKind::Paragraph, trimmed.to_string()))
}

fn push_block(
    blocks: &mut Vec<BlockEntry>,
    relative_path: &str,
    kind: BlockKind,
    start_line: usize,
    end_line: usize,
    ordinal: &mut usize,
    text: String,
) {
    let (text, anchor) = split_trailing_block_anchor(text);
    let current = *ordinal;
    *ordinal += 1;
    blocks.push(BlockEntry {
        note_path: relative_path.to_string(),
        kind,
        start_line,
        end_line,
        ordinal: current,
        text,
        anchor,
    });
}

fn strip_trailing_block_anchor(text: &str) -> String {
    split_trailing_block_anchor(text.to_string()).0
}

fn split_trailing_block_anchor(text: String) -> (String, Option<String>) {
    let trimmed = text.trim_end();
    let Some((body, token)) = trimmed.rsplit_once(char::is_whitespace) else {
        return (text, None);
    };
    let Some(anchor) = parse_block_anchor_token(token) else {
        return (text, None);
    };
    (body.trim_end().to_string(), Some(anchor))
}

fn parse_block_anchor_token(token: &str) -> Option<String> {
    let anchor = token.strip_prefix('^')?;
    (!anchor.is_empty()
        && anchor
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_')))
    .then(|| anchor.to_string())
}

fn collect_note_files(
    workspace: &NeoismWorkspace,
    out: &mut Vec<PathBuf>,
    limit: usize,
) -> std::io::Result<()> {
    for start in workspace.note_roots() {
        if out.len() >= limit {
            break;
        }
        if start.is_file() {
            if is_markdown_path(&start)
                && !is_ignored(&workspace.root, &start, &workspace.config.notes)
            {
                out.push(start);
            }
            continue;
        }
        collect_note_files_from_dir(
            &workspace.root,
            &start,
            &workspace.config.notes,
            out,
            limit,
        )?;
    }
    Ok(())
}

fn collect_note_files_from_dir(
    root: &Path,
    dir: &Path,
    notes: &NotesConfig,
    out: &mut Vec<PathBuf>,
    limit: usize,
) -> std::io::Result<()> {
    if out.len() >= limit || is_ignored(root, dir, notes) {
        return Ok(());
    }
    let Ok(read_dir) = fs::read_dir(dir) else {
        return Ok(());
    };
    let mut entries = read_dir.filter_map(Result::ok).collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        if out.len() >= limit {
            return Ok(());
        }
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            collect_note_files_from_dir(root, &path, notes, out, limit)?;
        } else if file_type.is_file()
            && is_markdown_path(&path)
            && !is_ignored(root, &path, notes)
        {
            out.push(path);
        }
    }
    Ok(())
}

fn is_ignored(root: &Path, path: &Path, notes: &NotesConfig) -> bool {
    let rel = relative_path_string(root, path);
    BUILT_IN_IGNORES
        .iter()
        .copied()
        .chain(notes.ignore.iter().map(String::as_str))
        .any(|ignore| ignore_matches(root, path, &rel, ignore))
}

fn ignore_matches(root: &Path, path: &Path, rel: &str, ignore: &str) -> bool {
    let ignore = ignore.trim().trim_matches('/');
    if ignore.is_empty() {
        return false;
    }
    rel == ignore
        || rel.starts_with(&format!("{ignore}/"))
        || path == root.join(ignore)
        || path
            .components()
            .filter_map(component_name)
            .any(|component| component == ignore)
}

fn is_in_note_roots(workspace: &NeoismWorkspace, path: &Path) -> bool {
    workspace
        .note_roots()
        .into_iter()
        .any(|note_root| path == note_root || path.starts_with(note_root))
}

pub fn is_markdown_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "md" | "markdown" | "mdx"
            )
        })
}

fn index_note_file(
    workspace: &NeoismWorkspace,
    path: &Path,
    index: &mut WorkspaceNoteIndex,
) -> std::io::Result<()> {
    let source = fs::read_to_string(path)?;
    let relative_path = workspace.note_path_label(path);
    let metadata = fs::metadata(path).ok();
    let modified = metadata
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);
    let parsed = parse_note(&relative_path, &source);
    let title = parsed
        .headings
        .iter()
        .find(|heading| heading.level == 1)
        .map(|heading| heading.text.clone())
        .or_else(|| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| relative_path.clone());
    index.notes.push(NoteEntry {
        path: path.to_path_buf(),
        relative_path,
        title,
        modified,
        hash: hash_source(&source),
    });
    index.blocks.extend(parsed.blocks);
    index.headings.extend(parsed.headings);
    index.links.extend(parsed.links);
    index.tags.extend(parsed.tags);
    index.tasks.extend(parsed.tasks);
    index.properties.extend(parsed.properties);
    Ok(())
}

fn parse_heading(trimmed: &str) -> Option<(u8, String)> {
    parse_heading_raw(trimmed)
        .map(|(level, text)| (level, strip_trailing_block_anchor(text.trim())))
}

fn parse_heading_raw(trimmed: &str) -> Option<(u8, String)> {
    let level = trimmed.chars().take_while(|ch| *ch == '#').count();
    if !(1..=6).contains(&level)
        || !trimmed.chars().nth(level).is_some_and(|ch| ch == ' ')
    {
        return None;
    }
    Some((level as u8, trimmed[level..].trim().to_string()))
}

fn parse_task(trimmed: &str) -> Option<(bool, String)> {
    parse_task_raw(trimmed)
        .map(|(checked, text)| (checked, strip_trailing_block_anchor(&text)))
}

fn parse_task_raw(trimmed: &str) -> Option<(bool, String)> {
    let body = trimmed
        .strip_prefix("- [")
        .or_else(|| trimmed.strip_prefix("* ["))?;
    let mut chars = body.chars();
    let marker = chars.next()?;
    if chars.next()? != ']' || chars.next()? != ' ' {
        return None;
    }
    let checked = matches!(marker, 'x' | 'X');
    if !checked && marker != ' ' {
        return None;
    }
    Some((checked, chars.as_str().to_string()))
}

fn is_divider(trimmed: &str) -> bool {
    trimmed.len() >= 3 && trimmed.chars().all(|ch| matches!(ch, '-' | '*' | '_'))
}

struct WikiLinkParts {
    raw: String,
    target: String,
    heading: Option<String>,
    alias: Option<String>,
    kind: LinkKind,
}

fn collect_wiki_links(line: &str) -> Vec<WikiLinkParts> {
    let mut out = Vec::new();
    let mut search_from = 0usize;
    while let Some(open_rel) = line[search_from..].find("[[") {
        let open = search_from + open_rel;
        let embed = open > 0 && line.as_bytes().get(open - 1) == Some(&b'!');
        let inner_start = open + 2;
        let Some(close_rel) = line[inner_start..].find("]]") else {
            break;
        };
        let close = inner_start + close_rel;
        let raw = &line[inner_start..close];
        if let Some(parts) = parse_wiki_link_inner(raw, embed) {
            out.push(parts);
        }
        search_from = close + 2;
    }
    out
}

fn parse_wiki_link_inner(raw: &str, embed: bool) -> Option<WikiLinkParts> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let (target_part, alias) = raw
        .split_once('|')
        .map(|(target, alias)| (target.trim(), Some(alias.trim().to_string())))
        .unwrap_or((raw, None));
    let kind = if target_part.trim_start().starts_with('@') {
        LinkKind::CodeRef
    } else if embed {
        LinkKind::Embed
    } else {
        LinkKind::Note
    };
    let target_part = target_part.trim_start_matches('@').trim();
    let (target, heading) = target_part
        .split_once('#')
        .map(|(target, heading)| (target.trim(), Some(heading.trim().to_string())))
        .unwrap_or((target_part, None));
    if target.is_empty() {
        return None;
    }
    Some(WikiLinkParts {
        raw: raw.to_string(),
        target: target.to_string(),
        heading,
        alias,
        kind,
    })
}

fn collect_tags(line: &str) -> Vec<String> {
    let mut tags = Vec::new();
    let bytes = line.as_bytes();
    let mut ix = 0usize;
    while ix < bytes.len() {
        if bytes[ix] != b'#' {
            ix += 1;
            continue;
        }
        let prev_ok = ix == 0 || !is_tag_char(bytes[ix - 1] as char);
        let mut end = ix + 1;
        while end < bytes.len() && is_tag_char(bytes[end] as char) {
            end += 1;
        }
        if prev_ok && end > ix + 1 {
            tags.push(line[ix + 1..end].to_string());
        }
        ix = end.max(ix + 1);
    }
    tags
}

fn is_tag_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '/')
}

fn heading_slug(text: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    slug.trim_matches('-').to_string()
}

fn markdown_link_match_query(query: &str) -> &str {
    let query = query.trim();
    if let Some((target, line)) = query.rsplit_once('-') {
        if !target.trim().is_empty() && line.chars().all(|ch| ch.is_ascii_digit()) {
            return target.trim();
        }
    }
    query
}

pub fn relative_link_target(base_dir: &Path, path: &Path) -> String {
    let from = path_components_for_relative(base_dir);
    let to = path_components_for_relative(path);
    let mut common = 0usize;
    while common < from.len() && common < to.len() && from[common] == to[common] {
        common += 1;
    }

    let mut parts = Vec::new();
    for _ in common..from.len() {
        parts.push("..".to_string());
    }
    parts.extend(to.into_iter().skip(common));
    if parts.is_empty() {
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string()
    } else {
        parts.join("/")
    }
}

fn path_components_for_relative(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            Component::ParentDir => Some("..".to_string()),
            _ => None,
        })
        .collect()
}

fn relative_path_string(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .filter_map(component_name)
        .collect::<Vec<_>>()
        .join("/")
}

fn component_name(component: Component<'_>) -> Option<String> {
    match component {
        Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
        Component::ParentDir => Some("..".to_string()),
        _ => None,
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    a.canonicalize().unwrap_or_else(|_| a.to_path_buf())
        == b.canonicalize().unwrap_or_else(|_| b.to_path_buf())
}

fn hash_source(source: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    source.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WorkspaceConfig;

    #[test]
    fn parses_note_links_headings_tags_and_tasks() {
        let parsed = parse_note(
            "notes/plan.md",
            "---\nowner: Parker\npriority: 2\n---\n# Plan ^plan\n- [ ] ship workspace #neoism/workspace ^task1\nSee [[Roadmap#Now|roadmap]] and [[@src/main.rs-10]].\n```rust\n# not heading\n```\n",
        );

        assert_eq!(parsed.headings[0].slug, "plan");
        assert_eq!(parsed.tasks[0].text, "ship workspace #neoism/workspace");
        assert_eq!(parsed.blocks[0].anchor.as_deref(), Some("plan"));
        assert_eq!(parsed.blocks[1].anchor.as_deref(), Some("task1"));
        assert_eq!(parsed.properties.len(), 2);
        assert_eq!(parsed.tags[0].tag, "neoism/workspace");
        assert_eq!(parsed.links.len(), 2);
        assert_eq!(parsed.links[0].kind, LinkKind::Note);
        assert_eq!(parsed.links[0].target, "Roadmap");
        assert_eq!(parsed.links[0].heading.as_deref(), Some("Now"));
        assert_eq!(parsed.links[0].alias.as_deref(), Some("roadmap"));
        assert_eq!(parsed.links[1].kind, LinkKind::CodeRef);
    }

    #[test]
    fn relative_targets_match_markdown_link_style() {
        let base = Path::new("/workspace/docs");
        let note = Path::new("/workspace/notes/Plan.md");

        assert_eq!(relative_link_target(base, note), "../notes/Plan.md");
    }

    #[test]
    fn built_in_ignores_skip_generated_worktrees() {
        let dir = std::env::temp_dir().join(format!(
            "neoism-note-built-in-ignore-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let notes_root = dir.join("Neoism/Vaults/Personal");
        fs::create_dir_all(notes_root.join(".claude/worktrees/agent/docs")).unwrap();
        fs::create_dir_all(notes_root.join("docs")).unwrap();
        fs::write(
            notes_root.join(".claude/worktrees/agent/docs/Copy.md"),
            "# Copy\n",
        )
        .unwrap();
        fs::write(notes_root.join("docs/Real.md"), "# Real\n").unwrap();

        let workspace = NeoismWorkspace {
            root: dir.clone(),
            config: WorkspaceConfig {
                version: 1,
                id: "test".to_string(),
                name: "test".to_string(),
                notes: NotesConfig {
                    enabled: true,
                    workspace: notes_root.display().to_string(),
                    vault_id: None,
                    scope: PathBuf::from("."),
                    ignore: Vec::new(),
                },
            },
        };
        let index = WorkspaceNoteIndex::build(&workspace).unwrap();

        assert_eq!(index.notes.len(), 1);
        assert_eq!(index.notes[0].relative_path, "docs/Real.md");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn incremental_file_indexing_stays_inside_note_roots() {
        let dir = std::env::temp_dir().join(format!(
            "neoism-note-file-root-filter-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("docs")).unwrap();
        let notes_root = dir.join("Neoism/Vaults/Personal");
        fs::create_dir_all(&notes_root).unwrap();
        fs::write(dir.join("docs/Spec.md"), "# Spec\n").unwrap();
        fs::write(notes_root.join("Plan.md"), "# Plan\n").unwrap();

        let workspace = NeoismWorkspace {
            root: dir.clone(),
            config: WorkspaceConfig {
                version: 1,
                id: "test".to_string(),
                name: "test".to_string(),
                notes: NotesConfig {
                    enabled: true,
                    workspace: notes_root.display().to_string(),
                    vault_id: None,
                    scope: PathBuf::from("."),
                    ignore: Vec::new(),
                },
            },
        };

        assert!(
            WorkspaceNoteIndex::build_file(&workspace, dir.join("docs/Spec.md"))
                .unwrap()
                .is_none()
        );
        assert_eq!(
            WorkspaceNoteIndex::build_file(&workspace, notes_root.join("Plan.md"))
                .unwrap()
                .unwrap()
                .notes[0]
                .relative_path,
            "Plan.md"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn heading_suggestions_complete_existing_note_target() {
        let dir = std::env::temp_dir().join(format!(
            "neoism-note-heading-suggestions-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let notes_root = dir.join("Neoism/Vaults/Personal");
        fs::create_dir_all(&notes_root).unwrap();
        fs::write(dir.join(".neoism-placeholder"), "").unwrap();
        fs::write(
            notes_root.join("Roadmap.md"),
            "# Roadmap\n\n## Now\n\n## Later\n",
        )
        .unwrap();

        let workspace = NeoismWorkspace {
            root: dir.clone(),
            config: WorkspaceConfig {
                version: 1,
                id: "test".to_string(),
                name: "test".to_string(),
                notes: NotesConfig {
                    enabled: true,
                    workspace: notes_root.display().to_string(),
                    vault_id: None,
                    scope: PathBuf::from("."),
                    ignore: Vec::new(),
                },
            },
        };
        let index = WorkspaceNoteIndex::build(&workspace).unwrap();
        let suggestions = index.heading_suggestions(
            &notes_root,
            &notes_root.join("Roadmap.md"),
            Some("Roadmap"),
            "No",
            5,
        );

        assert_eq!(suggestions, vec!["Roadmap.md#Now".to_string()]);
        let _ = fs::remove_dir_all(&dir);
    }
}
