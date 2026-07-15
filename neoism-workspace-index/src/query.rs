use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::config::{init_workspace, load_workspace, NeoismWorkspace};
use crate::graph_db::{
    block_on_db, io_other, migrate, open_pool, rebuild_note_graph,
    remove_note_graph_file, replace_note_graph_file, workspace_graph_db_path,
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
    pub fn open(root: impl AsRef<Path>) -> std::io::Result<Self> {
        let workspace = load_workspace(root.as_ref())?.ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!(
                    "{} is not a Neoism workspace; run `neoism init {}` first",
                    root.as_ref().display(),
                    root.as_ref().display()
                ),
            )
        })?;
        let graph = Self { workspace };
        graph.ensure_indexed()?;
        Ok(graph)
    }

    pub fn init(root: impl AsRef<Path>) -> std::io::Result<Self> {
        let workspace = init_workspace(root.as_ref())?;
        let graph = Self { workspace };
        graph.reindex()?;
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
            let pool = open_pool(&db_path).await?;
            migrate(&pool).await?;
            let note = resolve_note(&pool, target).await?;
            pool.close().await;
            Ok(note)
        })
    }

    pub fn notes(&self, limit: NoteQueryLimit) -> std::io::Result<Vec<NoteSummary>> {
        let db_path = self.db_path();
        block_on_db(async {
            let pool = open_pool(&db_path).await?;
            migrate(&pool).await?;
            let rows = sqlx::query(
                "SELECT path, title, modified, indexed_at FROM notes ORDER BY title, path LIMIT ?",
            )
            .bind(limit.0 as i64)
            .fetch_all(&pool)
            .await
            .map_err(io_other)?;
            pool.close().await;
            Ok(rows.into_iter().map(note_from_row).collect())
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
            let pool = open_pool(&db_path).await?;
            migrate(&pool).await?;
            let rows = if let Some(note) = note {
                let note = resolve_note(&pool, &note).await?;
                let Some(note) = note else {
                    pool.close().await;
                    return Ok(Vec::new());
                };
                sqlx::query(
                    "SELECT path, line, level, text, slug FROM headings WHERE path = ? ORDER BY line LIMIT ?",
                )
                .bind(note.path)
                .bind(limit.0 as i64)
                .fetch_all(&pool)
                .await
                .map_err(io_other)?
            } else {
                sqlx::query(
                    "SELECT path, line, level, text, slug FROM headings ORDER BY path, line LIMIT ?",
                )
                .bind(limit.0 as i64)
                .fetch_all(&pool)
                .await
                .map_err(io_other)?
            };
            pool.close().await;
            Ok(rows.into_iter().map(heading_from_row).collect())
        })
    }

    pub fn links(
        &self,
        unresolved_only: bool,
        limit: NoteQueryLimit,
    ) -> std::io::Result<Vec<LinkSummary>> {
        let db_path = self.db_path();
        block_on_db(async {
            let pool = open_pool(&db_path).await?;
            migrate(&pool).await?;
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
            let rows = sqlx::query(sql)
                .bind(limit.0 as i64)
                .fetch_all(&pool)
                .await
                .map_err(io_other)?;
            pool.close().await;
            Ok(rows.into_iter().map(link_from_row).collect())
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
            let pool = open_pool(&db_path).await?;
            migrate(&pool).await?;
            let Some(note) = resolve_db_note(&pool, &target).await? else {
                pool.close().await;
                return Ok(Vec::new());
            };
            let rows = sqlx::query(
                r#"
                SELECT links.source_path, links.source_line, links.raw, links.target,
                       notes.path AS target_path, links.heading, links.alias, links.kind
                FROM links
                LEFT JOIN notes ON notes.id = links.target_note_id
                WHERE links.target_note_id = ?
                ORDER BY links.source_path, links.source_line
                LIMIT ?
                "#,
            )
            .bind(note.id)
            .bind(limit.0 as i64)
            .fetch_all(&pool)
            .await
            .map_err(io_other)?;
            pool.close().await;
            Ok(rows.into_iter().map(link_from_row).collect())
        })
    }

    pub fn tags(&self, limit: NoteQueryLimit) -> std::io::Result<Vec<TagSummary>> {
        let db_path = self.db_path();
        block_on_db(async {
            let pool = open_pool(&db_path).await?;
            migrate(&pool).await?;
            let rows = sqlx::query(
                "SELECT tag, COUNT(*) AS count FROM tags GROUP BY tag ORDER BY count DESC, tag LIMIT ?",
            )
            .bind(limit.0 as i64)
            .fetch_all(&pool)
            .await
            .map_err(io_other)?;
            pool.close().await;
            Ok(rows
                .into_iter()
                .map(|row| TagSummary {
                    tag: row.get("tag"),
                    count: row.get("count"),
                })
                .collect())
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
            let pool = open_pool(&db_path).await?;
            migrate(&pool).await?;
            let rows = if let Some(tag) = tag {
                sqlx::query(
                    r#"
                    SELECT tags.tag, tags.path, tags.line, notes.title
                    FROM tags
                    JOIN notes ON notes.id = tags.note_id
                    WHERE tags.tag = ?
                    ORDER BY tags.path, tags.line
                    LIMIT ?
                    "#,
                )
                .bind(tag)
                .bind(limit.0 as i64)
                .fetch_all(&pool)
                .await
                .map_err(io_other)?
            } else {
                sqlx::query(
                    r#"
                    SELECT tags.tag, tags.path, tags.line, notes.title
                    FROM tags
                    JOIN notes ON notes.id = tags.note_id
                    ORDER BY tags.tag, tags.path, tags.line
                    LIMIT ?
                    "#,
                )
                .bind(limit.0 as i64)
                .fetch_all(&pool)
                .await
                .map_err(io_other)?
            };
            pool.close().await;
            Ok(rows.into_iter().map(tag_occurrence_from_row).collect())
        })
    }

    pub fn tasks(
        &self,
        checked: Option<bool>,
        limit: NoteQueryLimit,
    ) -> std::io::Result<Vec<TaskSummary>> {
        let db_path = self.db_path();
        block_on_db(async {
            let pool = open_pool(&db_path).await?;
            migrate(&pool).await?;
            let rows = if let Some(checked) = checked {
                sqlx::query(
                    "SELECT path, line, checked, text FROM tasks WHERE checked = ? ORDER BY checked, path, line LIMIT ?",
                )
                .bind(if checked { 1_i64 } else { 0_i64 })
                .bind(limit.0 as i64)
                .fetch_all(&pool)
                .await
                .map_err(io_other)?
            } else {
                sqlx::query(
                    "SELECT path, line, checked, text FROM tasks ORDER BY checked, path, line LIMIT ?",
                )
                .bind(limit.0 as i64)
                .fetch_all(&pool)
                .await
                .map_err(io_other)?
            };
            pool.close().await;
            Ok(rows.into_iter().map(task_from_row).collect())
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
            let pool = open_pool(&db_path).await?;
            migrate(&pool).await?;
            let rows = if let Some(note) = note {
                let Some(note) = resolve_note(&pool, &note).await? else {
                    pool.close().await;
                    return Ok(Vec::new());
                };
                sqlx::query(
                    "SELECT path, key, value, value_type FROM note_properties WHERE path = ? ORDER BY key LIMIT ?",
                )
                .bind(note.path)
                .bind(limit.0 as i64)
                .fetch_all(&pool)
                .await
                .map_err(io_other)?
            } else {
                sqlx::query(
                    "SELECT path, key, value, value_type FROM note_properties ORDER BY path, key LIMIT ?",
                )
                .bind(limit.0 as i64)
                .fetch_all(&pool)
                .await
                .map_err(io_other)?
            };
            pool.close().await;
            Ok(rows.into_iter().map(property_from_row).collect())
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
        let like = format!("%{}%", query.trim());
        block_on_db(async {
            let pool = open_pool(&db_path).await?;
            migrate(&pool).await?;
            let rows = match sqlx::query(
                r#"
                SELECT blocks.path, blocks.start_line, blocks.end_line, blocks.kind,
                       snippet(blocks_fts, 4, '[', ']', '...', 16) AS text
                FROM blocks_fts
                JOIN blocks ON blocks.id = blocks_fts.block_id
                WHERE blocks_fts MATCH ?
                ORDER BY rank
                LIMIT ?
                "#,
            )
            .bind(&fts)
            .bind(limit.0 as i64)
            .fetch_all(&pool)
            .await
            {
                Ok(rows) => rows,
                Err(_) => {
                    sqlx::query(
                        "SELECT path, start_line, end_line, kind, text FROM blocks WHERE text LIKE ? ORDER BY path, start_line LIMIT ?",
                    )
                    .bind(&like)
                    .bind(limit.0 as i64)
                    .fetch_all(&pool)
                    .await
                    .map_err(io_other)?
                }
            };
            pool.close().await;
            Ok(rows.into_iter().map(search_hit_from_row).collect())
        })
    }

    pub fn graph(&self, limit: NoteQueryLimit) -> std::io::Result<NoteGraphSummary> {
        let db_path = self.db_path();
        block_on_db(async {
            let pool = open_pool(&db_path).await?;
            migrate(&pool).await?;
            let nodes =
                sqlx::query("SELECT path, title FROM notes ORDER BY title, path LIMIT ?")
                    .bind(limit.0 as i64)
                    .fetch_all(&pool)
                    .await
                    .map_err(io_other)?
                    .into_iter()
                    .map(|row| NoteGraphNode {
                        path: row.get("path"),
                        title: row.get("title"),
                    })
                    .collect::<Vec<_>>();
            let edges = sqlx::query(
                r#"
                SELECT links.source_path, notes.path AS target_path, links.source_line, links.kind
                FROM links
                JOIN notes ON notes.id = links.target_note_id
                WHERE links.kind != 'code_ref'
                ORDER BY links.source_path, links.source_line
                LIMIT ?
                "#,
            )
            .bind(limit.0 as i64)
            .fetch_all(&pool)
            .await
            .map_err(io_other)?
            .into_iter()
            .map(|row| NoteGraphEdge {
                source_path: row.get("source_path"),
                target_path: row.get("target_path"),
                source_line: row.get("source_line"),
                kind: row.get("kind"),
            })
            .collect::<Vec<_>>();
            pool.close().await;
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

async fn resolve_note(
    pool: &sqlx::SqlitePool,
    target: &str,
) -> std::io::Result<Option<NoteSummary>> {
    let note = resolve_db_note(pool, target).await?;
    Ok(note.map(|note| NoteSummary {
        path: note.path,
        title: note.title,
        modified: note.modified,
        indexed_at: note.indexed_at,
    }))
}

async fn resolve_db_note(
    pool: &sqlx::SqlitePool,
    target: &str,
) -> std::io::Result<Option<DbNote>> {
    let target = target.trim();
    let rows = sqlx::query("SELECT id, path, title, modified, indexed_at FROM notes")
        .fetch_all(pool)
        .await
        .map_err(io_other)?;
    let notes = rows
        .into_iter()
        .map(|row| DbNote {
            id: row.get("id"),
            path: row.get("path"),
            title: row.get("title"),
            modified: row.get("modified"),
            indexed_at: row.get("indexed_at"),
        })
        .collect::<Vec<_>>();
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

fn note_from_row(row: sqlx::sqlite::SqliteRow) -> NoteSummary {
    NoteSummary {
        path: row.get("path"),
        title: row.get("title"),
        modified: row.get("modified"),
        indexed_at: row.get("indexed_at"),
    }
}

fn heading_from_row(row: sqlx::sqlite::SqliteRow) -> HeadingSummary {
    HeadingSummary {
        path: row.get("path"),
        line: row.get("line"),
        level: row.get("level"),
        text: row.get("text"),
        slug: row.get("slug"),
    }
}

fn link_from_row(row: sqlx::sqlite::SqliteRow) -> LinkSummary {
    LinkSummary {
        source_path: row.get("source_path"),
        source_line: row.get("source_line"),
        raw: row.get("raw"),
        target: row.get("target"),
        target_path: row.get("target_path"),
        heading: row.get("heading"),
        alias: row.get("alias"),
        kind: row.get("kind"),
    }
}

fn tag_occurrence_from_row(row: sqlx::sqlite::SqliteRow) -> TagOccurrenceSummary {
    TagOccurrenceSummary {
        tag: row.get("tag"),
        path: row.get("path"),
        line: row.get("line"),
        title: row.get("title"),
    }
}

fn task_from_row(row: sqlx::sqlite::SqliteRow) -> TaskSummary {
    let checked: i64 = row.get("checked");
    TaskSummary {
        path: row.get("path"),
        line: row.get("line"),
        checked: checked != 0,
        text: row.get("text"),
    }
}

fn property_from_row(row: sqlx::sqlite::SqliteRow) -> PropertySummary {
    PropertySummary {
        path: row.get("path"),
        key: row.get("key"),
        value: row.get("value"),
        value_type: row.get("value_type"),
    }
}

fn search_hit_from_row(row: sqlx::sqlite::SqliteRow) -> NoteSearchHit {
    NoteSearchHit {
        path: row.get("path"),
        start_line: row.get("start_line"),
        end_line: row.get("end_line"),
        kind: row.get("kind"),
        text: row.get("text"),
    }
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
        let graph = NoteGraph::init(&root).unwrap();
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

        assert_eq!(graph.notes(NoteQueryLimit(10)).unwrap().len(), 3);
        assert_eq!(
            graph
                .backlinks("Roadmap", NoteQueryLimit(10))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(graph.tags(NoteQueryLimit(10)).unwrap()[0].tag, "neoism");
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
        assert!(!graph.search("ship", NoteQueryLimit(10)).unwrap().is_empty());

        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(notes_home);
    }
}
