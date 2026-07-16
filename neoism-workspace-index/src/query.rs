use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{
    default_notes_workspace, linked_project_for_code_dir, load_workspace, NeoismWorkspace,
};
use crate::graph_db::{
    block_on_db, int, migrate, open_note_db, rebuild_note_graph, remove_note_graph_file,
    replace_note_graph_file, text, workspace_graph_db_path, DbRow, NoteDb,
};
use crate::link_repair::{repair_links_for_move, LinkRepairReport};
use crate::notes::WorkspaceNoteIndex;
use crate::watcher::NoteGraphWatcher;

#[derive(Debug, Clone, Copy)]
pub struct NoteQueryLimit(pub usize);

impl Default for NoteQueryLimit {
    fn default() -> Self {
        Self(100)
    }
}

#[derive(Debug, Clone)]
pub struct NoteGraph {
    workspace: NeoismWorkspace,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NoteSummary {
    pub path: String,
    pub title: String,
    pub modified: i64,
    pub indexed_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HeadingSummary {
    pub path: String,
    pub line: i64,
    pub level: i64,
    pub text: String,
    pub slug: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LinkSummary {
    pub source_path: String,
    pub source_line: i64,
    pub raw: String,
    pub target: String,
    pub target_path: Option<String>,
    pub heading: Option<String>,
    pub alias: Option<String>,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TagSummary {
    pub tag: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TagOccurrenceSummary {
    pub tag: String,
    pub path: String,
    pub line: i64,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskSummary {
    pub path: String,
    pub line: i64,
    pub checked: bool,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PropertySummary {
    pub path: String,
    pub key: String,
    pub value: String,
    pub value_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NoteSearchHit {
    pub path: String,
    pub start_line: i64,
    pub end_line: i64,
    pub kind: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NoteGraphNode {
    pub path: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NoteGraphEdge {
    pub source_path: String,
    pub target_path: String,
    pub source_line: i64,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NoteGraphSummary {
    pub nodes: Vec<NoteGraphNode>,
    pub edges: Vec<NoteGraphEdge>,
}

impl NoteGraph {
    /// Open the note graph for a code root, vault-first: the vault the
    /// root LINKS to (vault `project.toml`), else the root's own
    /// workspace config, else the global Default vault. Never requires
    /// (or writes) a per-project `.neoism` marker — Vaults are the only
    /// notes model.
    pub fn open(root: impl AsRef<Path>) -> std::io::Result<Self> {
        let root = root.as_ref();
        let workspace = linked_project_for_code_dir(root)
            .ok()
            .flatten()
            .or_else(|| load_workspace(root).ok().flatten())
            .filter(|workspace| workspace.config.notes.enabled)
            .unwrap_or_else(default_notes_workspace);
        let graph = Self { workspace };
        graph.ensure_indexed()?;
        Ok(graph)
    }

    pub fn from_workspace(workspace: NeoismWorkspace) -> std::io::Result<Self> {
        let graph = Self { workspace };
        graph.reindex()?;
        Ok(graph)
    }

    pub fn workspace(&self) -> &NeoismWorkspace {
        &self.workspace
    }

    pub fn db_path(&self) -> PathBuf {
        workspace_graph_db_path(&self.workspace)
    }

    pub fn reindex(&self) -> std::io::Result<()> {
        let index = WorkspaceNoteIndex::build(&self.workspace)?;
        rebuild_note_graph(&self.workspace, &index)
    }

    pub fn replace_file(&self, path: impl AsRef<Path>) -> std::io::Result<()> {
        replace_note_graph_file(&self.workspace, path)
    }

    pub fn remove_file(&self, path: impl AsRef<Path>) -> std::io::Result<()> {
        remove_note_graph_file(&self.workspace, path)
    }

    pub fn repair_moved_note(
        &self,
        old_path: impl AsRef<Path>,
        new_path: impl AsRef<Path>,
    ) -> std::io::Result<LinkRepairReport> {
        let report = repair_links_for_move(&self.workspace, old_path, new_path)?;
        self.reindex()?;
        Ok(report)
    }

    pub fn create_note(&self, title: &str) -> std::io::Result<NoteSummary> {
        let root = self.workspace.notes_workspace_dir();
        std::fs::create_dir_all(&root)?;
        let base = note_file_stem(title);
        let mut path = root.join(format!("{base}.md"));
        let mut ordinal = 2usize;
        while path.exists() {
            path = root.join(format!("{base} {ordinal}.md"));
            ordinal += 1;
        }
        OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)?;
        self.replace_file(&path)?;
        let rel = self.workspace.note_path_label(&path);
        self.note(&rel)?.ok_or_else(|| {
            std::io::Error::other(format!(
                "created note was not indexed: {}",
                path.display()
            ))
        })
    }

    pub fn toggle_task(
        &self,
        path: impl AsRef<Path>,
        line: usize,
        checked: Option<bool>,
    ) -> std::io::Result<TaskSummary> {
        let path = self.workspace.resolve_note_path(path.as_ref());
        let source = std::fs::read_to_string(&path)?;
        let mut lines = source.lines().map(str::to_string).collect::<Vec<_>>();
        if line == 0 || line > lines.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("line {line} is outside {}", path.display()),
            ));
        }
        let target = &mut lines[line - 1];
        let marker_ix = task_marker_index(target).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("line {line} is not a Markdown task"),
            )
        })?;
        let current = target[marker_ix..marker_ix + 1].eq_ignore_ascii_case("x");
        let next = checked.unwrap_or(!current);
        target.replace_range(marker_ix..marker_ix + 1, if next { "x" } else { " " });
        let mut updated = lines.join("\n");
        if source.ends_with('\n') {
            updated.push('\n');
        }
        std::fs::write(&path, updated)?;
        self.replace_file(&path)?;
        let rel = self.workspace.note_path_label(&path);
        self.tasks(None, NoteQueryLimit(100_000))?
            .into_iter()
            .find(|task| task.path == rel && task.line == line as i64)
            .ok_or_else(|| std::io::Error::other("updated task was not indexed"))
    }

    pub fn note(&self, target: &str) -> std::io::Result<Option<NoteSummary>> {
        let db_path = self.db_path();
        block_on_db(async {
            let db = open_note_db(&db_path).await?;
            migrate(&db).await?;
            let note = resolve_note(&db, target).await?;
            db.close().await;
            Ok(note)
        })
    }

    pub fn notes(&self, limit: NoteQueryLimit) -> std::io::Result<Vec<NoteSummary>> {
        let db_path = self.db_path();
        block_on_db(async {
            let db = open_note_db(&db_path).await?;
            migrate(&db).await?;
            let rows = db
                .fetch_all(
                    "SELECT path, title, modified, indexed_at FROM notes ORDER BY title, path LIMIT ?",
                    vec![int(limit.0 as i64)],
                )
                .await?;
            db.close().await;
            rows.iter().map(note_from_row).collect()
        })
    }

    pub fn headings(
        &self,
        note: Option<&str>,
        limit: NoteQueryLimit,
    ) -> std::io::Result<Vec<HeadingSummary>> {
        let db_path = self.db_path();
        let note = note.map(str::to_string);
        block_on_db(async {
            let db = open_note_db(&db_path).await?;
            migrate(&db).await?;
            let rows = if let Some(note) = note {
                let note = resolve_note(&db, &note).await?;
                let Some(note) = note else {
                    db.close().await;
                    return Ok(Vec::new());
                };
                db.fetch_all(
                    "SELECT path, line, level, text, slug FROM headings WHERE path = ? ORDER BY line LIMIT ?",
                    vec![text(note.path), int(limit.0 as i64)],
                )
                .await?
            } else {
                db.fetch_all(
                    "SELECT path, line, level, text, slug FROM headings ORDER BY path, line LIMIT ?",
                    vec![int(limit.0 as i64)],
                )
                .await?
            };
            db.close().await;
            rows.iter().map(heading_from_row).collect()
        })
    }

    pub fn links(
        &self,
        unresolved_only: bool,
        limit: NoteQueryLimit,
    ) -> std::io::Result<Vec<LinkSummary>> {
        let db_path = self.db_path();
        block_on_db(async {
            let db = open_note_db(&db_path).await?;
            migrate(&db).await?;
            let sql = if unresolved_only {
                r#"
                SELECT links.source_path, links.source_line, links.raw, links.target,
                       notes.path AS target_path, links.heading, links.alias, links.kind
                FROM links
                LEFT JOIN notes ON notes.id = links.target_note_id
                WHERE links.target_note_id IS NULL AND links.kind != 'code_ref'
                ORDER BY links.source_path, links.source_line
                LIMIT ?
                "#
            } else {
                r#"
                SELECT links.source_path, links.source_line, links.raw, links.target,
                       notes.path AS target_path, links.heading, links.alias, links.kind
                FROM links
                LEFT JOIN notes ON notes.id = links.target_note_id
                ORDER BY links.source_path, links.source_line
                LIMIT ?
                "#
            };
            let rows = db.fetch_all(sql, vec![int(limit.0 as i64)]).await?;
            db.close().await;
            rows.iter().map(link_from_row).collect()
        })
    }

    pub fn backlinks(
        &self,
        target: &str,
        limit: NoteQueryLimit,
    ) -> std::io::Result<Vec<LinkSummary>> {
        let db_path = self.db_path();
        let target = target.to_string();
        block_on_db(async {
            let db = open_note_db(&db_path).await?;
            migrate(&db).await?;
            let Some(note) = resolve_db_note(&db, &target).await? else {
                db.close().await;
                return Ok(Vec::new());
            };
            let rows = db
                .fetch_all(
                    r#"
                    SELECT links.source_path, links.source_line, links.raw, links.target,
                           notes.path AS target_path, links.heading, links.alias, links.kind
                    FROM links
                    LEFT JOIN notes ON notes.id = links.target_note_id
                    WHERE links.target_note_id = ?
                    ORDER BY links.source_path, links.source_line
                    LIMIT ?
                    "#,
                    vec![text(note.id), int(limit.0 as i64)],
                )
                .await?;
            db.close().await;
            rows.iter().map(link_from_row).collect()
        })
    }

    pub fn tags(&self, limit: NoteQueryLimit) -> std::io::Result<Vec<TagSummary>> {
        let db_path = self.db_path();
        block_on_db(async {
            let db = open_note_db(&db_path).await?;
            migrate(&db).await?;
            let rows = db
                .fetch_all(
                    "SELECT tag, COUNT(*) AS count FROM tags GROUP BY tag ORDER BY count DESC, tag LIMIT ?",
                    vec![int(limit.0 as i64)],
                )
                .await?;
            db.close().await;
            rows.iter()
                .map(|row| {
                    Ok(TagSummary {
                        tag: row.get_str("tag")?,
                        count: row.get_i64("count")?,
                    })
                })
                .collect()
        })
    }

    pub fn tag_occurrences(
        &self,
        tag: Option<&str>,
        limit: NoteQueryLimit,
    ) -> std::io::Result<Vec<TagOccurrenceSummary>> {
        let db_path = self.db_path();
        let tag = tag.map(str::to_string);
        block_on_db(async {
            let db = open_note_db(&db_path).await?;
            migrate(&db).await?;
            let rows = if let Some(tag) = tag {
                db.fetch_all(
                    r#"
                    SELECT tags.tag, tags.path, tags.line, notes.title
                    FROM tags
                    JOIN notes ON notes.id = tags.note_id
                    WHERE tags.tag = ?
                    ORDER BY tags.path, tags.line
                    LIMIT ?
                    "#,
                    vec![text(tag), int(limit.0 as i64)],
                )
                .await?
            } else {
                db.fetch_all(
                    r#"
                    SELECT tags.tag, tags.path, tags.line, notes.title
                    FROM tags
                    JOIN notes ON notes.id = tags.note_id
                    ORDER BY tags.tag, tags.path, tags.line
                    LIMIT ?
                    "#,
                    vec![int(limit.0 as i64)],
                )
                .await?
            };
            db.close().await;
            rows.iter().map(tag_occurrence_from_row).collect()
        })
    }

    pub fn tasks(
        &self,
        checked: Option<bool>,
        limit: NoteQueryLimit,
    ) -> std::io::Result<Vec<TaskSummary>> {
        let db_path = self.db_path();
        block_on_db(async {
            let db = open_note_db(&db_path).await?;
            migrate(&db).await?;
            let rows = if let Some(checked) = checked {
                db.fetch_all(
                    "SELECT path, line, checked, text FROM tasks WHERE checked = ? ORDER BY checked, path, line LIMIT ?",
                    vec![int(if checked { 1 } else { 0 }), int(limit.0 as i64)],
                )
                .await?
            } else {
                db.fetch_all(
                    "SELECT path, line, checked, text FROM tasks ORDER BY checked, path, line LIMIT ?",
                    vec![int(limit.0 as i64)],
                )
                .await?
            };
            db.close().await;
            rows.iter().map(task_from_row).collect()
        })
    }

    pub fn properties(
        &self,
        note: Option<&str>,
        limit: NoteQueryLimit,
    ) -> std::io::Result<Vec<PropertySummary>> {
        let db_path = self.db_path();
        let note = note.map(str::to_string);
        block_on_db(async {
            let db = open_note_db(&db_path).await?;
            migrate(&db).await?;
            let rows = if let Some(note) = note {
                let Some(note) = resolve_note(&db, &note).await? else {
                    db.close().await;
                    return Ok(Vec::new());
                };
                db.fetch_all(
                    "SELECT path, key, value, value_type FROM note_properties WHERE path = ? ORDER BY key LIMIT ?",
                    vec![text(note.path), int(limit.0 as i64)],
                )
                .await?
            } else {
                db.fetch_all(
                    "SELECT path, key, value, value_type FROM note_properties ORDER BY path, key LIMIT ?",
                    vec![int(limit.0 as i64)],
                )
                .await?
            };
            db.close().await;
            rows.iter().map(property_from_row).collect()
        })
    }

    pub fn search(
        &self,
        query: &str,
        limit: NoteQueryLimit,
    ) -> std::io::Result<Vec<NoteSearchHit>> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        let db_path = self.db_path();
        let fts = fts_query(query);
        // Tokenized LIKE scan — the first-class search path on turso (no
        // FTS5 module there) and the fallback for a failed MATCH on
        // SQLite. Every whitespace term must hit the block, in any order
        // ("docker workspace" finds "workspace ... docker"), which is the
        // useful half of what the FTS query grammar gave us.
        let terms: Vec<String> = query
            .split_whitespace()
            .map(|term| format!("%{term}%"))
            .collect();
        let like_sql = format!(
            "SELECT path, start_line, end_line, kind, text FROM blocks WHERE {} ORDER BY path, start_line LIMIT ?",
            vec!["text LIKE ?"; terms.len().max(1)].join(" AND "),
        );
        let like_params = || {
            let mut params: Vec<_> =
                terms.iter().map(|term| text(term.as_str())).collect();
            params.push(int(limit.0 as i64));
            params
        };
        block_on_db(async {
            let db = open_note_db(&db_path).await?;
            migrate(&db).await?;
            let rows = if db.fts_enabled() {
                match db
                    .fetch_all(
                        r#"
                        SELECT blocks.path, blocks.start_line, blocks.end_line, blocks.kind,
                               snippet(blocks_fts, 4, '[', ']', '...', 16) AS text
                        FROM blocks_fts
                        JOIN blocks ON blocks.id = blocks_fts.block_id
                        WHERE blocks_fts MATCH ?
                        ORDER BY rank
                        LIMIT ?
                        "#,
                        vec![text(fts.as_str()), int(limit.0 as i64)],
                    )
                    .await
                {
                    Ok(rows) => rows,
                    Err(_) => db.fetch_all(&like_sql, like_params()).await?,
                }
            } else {
                db.fetch_all(&like_sql, like_params()).await?
            };
            db.close().await;
            rows.iter().map(search_hit_from_row).collect()
        })
    }

    pub fn graph(&self, limit: NoteQueryLimit) -> std::io::Result<NoteGraphSummary> {
        let db_path = self.db_path();
        block_on_db(async {
            let db = open_note_db(&db_path).await?;
            migrate(&db).await?;
            let nodes = db
                .fetch_all(
                    "SELECT path, title FROM notes ORDER BY title, path LIMIT ?",
                    vec![int(limit.0 as i64)],
                )
                .await?
                .iter()
                .map(|row| {
                    Ok(NoteGraphNode {
                        path: row.get_str("path")?,
                        title: row.get_str("title")?,
                    })
                })
                .collect::<std::io::Result<Vec<_>>>()?;
            let edges = db
                .fetch_all(
                    r#"
                    SELECT links.source_path, notes.path AS target_path, links.source_line, links.kind
                    FROM links
                    JOIN notes ON notes.id = links.target_note_id
                    WHERE links.kind != 'code_ref'
                    ORDER BY links.source_path, links.source_line
                    LIMIT ?
                    "#,
                    vec![int(limit.0 as i64)],
                )
                .await?
                .iter()
                .map(|row| {
                    Ok(NoteGraphEdge {
                        source_path: row.get_str("source_path")?,
                        target_path: row.get_str("target_path")?,
                        source_line: row.get_i64("source_line")?,
                        kind: row.get_str("kind")?,
                    })
                })
                .collect::<std::io::Result<Vec<_>>>()?;
            db.close().await;
            Ok(NoteGraphSummary { nodes, edges })
        })
    }

    pub fn watch(&self) -> std::io::Result<NoteGraphWatcher> {
        NoteGraphWatcher::start(self.clone())
    }

    fn ensure_indexed(&self) -> std::io::Result<()> {
        if self.db_path().is_file() {
            return Ok(());
        }
        self.reindex()
    }
}

#[derive(Debug, Clone)]
struct DbNote {
    id: String,
    path: String,
    title: String,
    modified: i64,
    indexed_at: i64,
}

async fn resolve_note(db: &NoteDb, target: &str) -> std::io::Result<Option<NoteSummary>> {
    let note = resolve_db_note(db, target).await?;
    Ok(note.map(|note| NoteSummary {
        path: note.path,
        title: note.title,
        modified: note.modified,
        indexed_at: note.indexed_at,
    }))
}

async fn resolve_db_note(db: &NoteDb, target: &str) -> std::io::Result<Option<DbNote>> {
    let target = target.trim();
    let rows = db
        .fetch_all(
            "SELECT id, path, title, modified, indexed_at FROM notes",
            Vec::new(),
        )
        .await?;
    let notes = rows
        .iter()
        .map(|row| {
            Ok(DbNote {
                id: row.get_str("id")?,
                path: row.get_str("path")?,
                title: row.get_str("title")?,
                modified: row.get_i64("modified")?,
                indexed_at: row.get_i64("indexed_at")?,
            })
        })
        .collect::<std::io::Result<Vec<_>>>()?;
    Ok(notes
        .iter()
        .find(|note| {
            note.path == target
                || strip_markdown_extension(&note.path)
                    == strip_markdown_extension(target)
                || note.title.eq_ignore_ascii_case(target)
        })
        .cloned())
}

fn note_from_row(row: &DbRow) -> std::io::Result<NoteSummary> {
    Ok(NoteSummary {
        path: row.get_str("path")?,
        title: row.get_str("title")?,
        modified: row.get_i64("modified")?,
        indexed_at: row.get_i64("indexed_at")?,
    })
}

fn heading_from_row(row: &DbRow) -> std::io::Result<HeadingSummary> {
    Ok(HeadingSummary {
        path: row.get_str("path")?,
        line: row.get_i64("line")?,
        level: row.get_i64("level")?,
        text: row.get_str("text")?,
        slug: row.get_str("slug")?,
    })
}

fn link_from_row(row: &DbRow) -> std::io::Result<LinkSummary> {
    Ok(LinkSummary {
        source_path: row.get_str("source_path")?,
        source_line: row.get_i64("source_line")?,
        raw: row.get_str("raw")?,
        target: row.get_str("target")?,
        target_path: row.get_opt_str("target_path")?,
        heading: row.get_opt_str("heading")?,
        alias: row.get_opt_str("alias")?,
        kind: row.get_str("kind")?,
    })
}

fn tag_occurrence_from_row(row: &DbRow) -> std::io::Result<TagOccurrenceSummary> {
    Ok(TagOccurrenceSummary {
        tag: row.get_str("tag")?,
        path: row.get_str("path")?,
        line: row.get_i64("line")?,
        title: row.get_str("title")?,
    })
}

fn task_from_row(row: &DbRow) -> std::io::Result<TaskSummary> {
    Ok(TaskSummary {
        path: row.get_str("path")?,
        line: row.get_i64("line")?,
        checked: row.get_i64("checked")? != 0,
        text: row.get_str("text")?,
    })
}

fn property_from_row(row: &DbRow) -> std::io::Result<PropertySummary> {
    Ok(PropertySummary {
        path: row.get_str("path")?,
        key: row.get_str("key")?,
        value: row.get_str("value")?,
        value_type: row.get_str("value_type")?,
    })
}

fn search_hit_from_row(row: &DbRow) -> std::io::Result<NoteSearchHit> {
    Ok(NoteSearchHit {
        path: row.get_str("path")?,
        start_line: row.get_i64("start_line")?,
        end_line: row.get_i64("end_line")?,
        kind: row.get_str("kind")?,
        text: row.get_str("text")?,
    })
}

fn fts_query(query: &str) -> String {
    query
        .split_whitespace()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

fn note_file_stem(title: &str) -> String {
    let cleaned = title
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, ' ' | '-' | '_') {
                ch
            } else {
                ' '
            }
        })
        .collect::<String>();
    let stem = cleaned
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string();
    if stem.is_empty() {
        "Untitled".to_string()
    } else {
        stem
    }
}

fn task_marker_index(line: &str) -> Option<usize> {
    let trimmed = line.trim_start();
    let indent = line.len() - trimmed.len();
    let body = trimmed
        .strip_prefix("- [")
        .or_else(|| trimmed.strip_prefix("* ["))?;
    let marker = body.chars().next()?;
    let close_ix = marker.len_utf8();
    if body.get(close_ix..close_ix + 2)? != "] " {
        return None;
    }
    (marker == ' ' || marker.eq_ignore_ascii_case(&'x')).then_some(indent + 3)
}

fn strip_markdown_extension(path: &str) -> &str {
    for suffix in [".markdown", ".mdx", ".md"] {
        if let Some(stripped) = path.strip_suffix(suffix) {
            return stripped;
        }
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir()
            .join(format!("neoism-note-query-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        root
    }

    #[test]
    fn note_graph_queries_notes_links_tags_tasks_and_search() {
        let root = temp_root("queries");
        let notes_home = std::env::temp_dir()
            .join(format!("neoism-query-notes-home-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&notes_home);
        unsafe {
            std::env::set_var("NEOISM_NOTES_HOME", &notes_home);
        }
        let graph = NoteGraph::open(&root).unwrap();
        let notes_root = graph.workspace().notes_workspace_dir();
        std::fs::create_dir_all(&notes_root).unwrap();
        std::fs::write(
            notes_root.join("Roadmap.md"),
            "---\nowner: Parker\npriority: 2\n---\n# Roadmap\n\n- [ ] ship #neoism\n\nSee [[Plan]]\n",
        )
        .unwrap();
        std::fs::write(
            notes_root.join("Plan.md"),
            "# Plan\n\nBack to [[Roadmap]].\n",
        )
        .unwrap();
        graph.reindex().unwrap();

        // The default vault is also seeded with welcome pages, so assert on
        // the fixture notes rather than an exact count.
        let notes = graph.notes(NoteQueryLimit(100)).unwrap();
        assert!(notes.iter().any(|note| note.path == "Roadmap.md"));
        assert!(notes.iter().any(|note| note.path == "Plan.md"));
        assert_eq!(
            graph
                .backlinks("Roadmap", NoteQueryLimit(10))
                .unwrap()
                .len(),
            1
        );
        assert!(graph
            .tags(NoteQueryLimit(100))
            .unwrap()
            .iter()
            .any(|tag| tag.tag == "neoism"));
        assert_eq!(
            graph
                .tag_occurrences(Some("neoism"), NoteQueryLimit(10))
                .unwrap()[0]
                .path,
            "Roadmap.md"
        );
        assert!(graph
            .tasks(None, NoteQueryLimit(10))
            .unwrap()
            .iter()
            .any(|task| task.text == "ship #neoism"));
        assert_eq!(
            graph
                .properties(Some("Roadmap"), NoteQueryLimit(10))
                .unwrap()[0]
                .key,
            "owner"
        );
        // Backend-aware: on the SQLite backend this exercises FTS5 MATCH;
        // on turso (no FTS5) it exercises the LIKE fallback path.
        assert!(!graph.search("ship", NoteQueryLimit(10)).unwrap().is_empty());

        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(notes_home);
    }
}
