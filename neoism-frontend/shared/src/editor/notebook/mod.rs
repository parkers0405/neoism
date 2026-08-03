use std::{
    collections::hash_map::DefaultHasher,
    collections::{BTreeMap, BTreeSet},
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
};

use base64::{
    engine::general_purpose::{STANDARD as BASE64_STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use web_time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::editor::markdown::{
    helpers::{is_notebook_output_marker_line, NOTEBOOK_MARKDOWN_CELL_MARKER},
    MarkdownPane,
};

const NOTEBOOK_OUTPUT_DISPLAY_MAX_BYTES: usize = 128 * 1024;
const NOTEBOOK_OUTPUT_DISPLAY_MAX_LINES: usize = 2_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NotebookDocument {
    #[serde(default)]
    pub cells: Vec<NotebookCell>,
    #[serde(default)]
    pub metadata: Value,
    #[serde(default = "default_nbformat")]
    pub nbformat: u8,
    #[serde(default = "default_nbformat_minor")]
    pub nbformat_minor: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NotebookCell {
    #[serde(rename = "cell_type")]
    pub cell_type: NotebookCellType,
    #[serde(default)]
    pub metadata: Value,
    #[serde(default)]
    pub source: NotebookSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<Value>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotebookCellType {
    Markdown,
    Code,
    Raw,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotebookCellAction {
    Run,
    RunAndBelow,
    ClearOutput,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotebookSource {
    Text(String),
}

#[derive(Clone, Debug)]
pub struct NotebookPane {
    pub path: PathBuf,
    pub document: NotebookDocument,
    pub markdown: MarkdownPane,
    pub cell_ranges: Vec<NotebookCellRange>,
    saved_json: String,
    pub error: Option<String>,
    pub running_cells: BTreeSet<usize>,
    pub running_cell_runs: BTreeMap<usize, u64>,
    pub execution_started_at: BTreeMap<usize, Instant>,
    pub completed_elapsed_ms: BTreeMap<usize, u128>,
    rendered_images: Vec<NotebookRenderedImageOutput>,
    next_execution_run_id: u64,
}

#[derive(Clone, Debug)]
pub struct NotebookExecutionResult {
    pub cell_index: usize,
    pub cell_id: String,
    pub run_id: u64,
    pub execution_count: u32,
    pub outputs: Vec<Value>,
    pub status: Result<(), String>,
    pub elapsed_ms: u128,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotebookOutputStream {
    Stdout,
    Stderr,
}

#[derive(Clone, Debug)]
pub struct NotebookExecutionChunk {
    pub cell_index: usize,
    pub cell_id: String,
    pub run_id: u64,
    pub stream: NotebookOutputStream,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct NotebookDisplayUpdate {
    pub cell_index: usize,
    pub cell_id: String,
    pub run_id: u64,
    pub display_id: String,
    pub output: Value,
}

#[derive(Clone, Debug)]
pub enum NotebookExecutionEvent {
    Output(NotebookExecutionChunk),
    DisplayUpdate(NotebookDisplayUpdate),
    Finished(NotebookExecutionResult),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotebookCellRange {
    pub cell_index: usize,
    pub kind: NotebookCellType,
    pub line_start: usize,
    pub line_end: usize,
    pub run_line: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotebookRenderedSource {
    pub markdown: String,
    pub cell_ranges: Vec<NotebookCellRange>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotebookRenderedImageOutput {
    pub cell_index: usize,
    pub output_index: usize,
    pub attachment_name: Option<String>,
    pub line: usize,
    pub image_id: u32,
    pub mime: String,
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    pub is_opaque: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InsertCellPosition {
    Above,
    Below,
}

impl NotebookPane {
    pub fn load(path: PathBuf) -> Self {
        match std::fs::read_to_string(&path) {
            Ok(source) => match NotebookDocument::from_json(&source) {
                Ok(document) => Self::from_document(path, document, source, None),
                Err(err) => Self::error(path, err),
            },
            Err(err) => Self::error(path, err.to_string()),
        }
    }

    pub fn from_document(
        path: PathBuf,
        mut document: NotebookDocument,
        saved_json: String,
        error: Option<String>,
    ) -> Self {
        document.ensure_cell_ids();
        let running_cells = BTreeSet::new();
        let rendered = document.render_markdown_with_running(&running_cells);
        let markdown = MarkdownPane::from_source(path.clone(), &rendered.markdown);
        let mut pane = Self {
            path,
            document,
            markdown,
            cell_ranges: rendered.cell_ranges,
            saved_json,
            error,
            running_cells,
            running_cell_runs: BTreeMap::new(),
            execution_started_at: BTreeMap::new(),
            completed_elapsed_ms: BTreeMap::new(),
            rendered_images: Vec::new(),
            next_execution_run_id: 1,
        };
        pane.refresh_markdown_image_preview_dimensions();
        pane
    }

    pub fn error(path: PathBuf, error: String) -> Self {
        let document = NotebookDocument::default();
        let mut pane =
            Self::from_document(path, document, String::new(), Some(error.clone()));
        let source = format!("# Notebook error\n\n```text\n{}\n```\n", error);
        pane.markdown = MarkdownPane::from_source(pane.path.clone(), &source);
        pane
    }

    pub fn is_dirty(&self) -> bool {
        self.to_json()
            .map(|json| json != self.saved_json)
            .unwrap_or(true)
    }

    pub fn kernel_name(&self) -> Option<String> {
        self.document.kernel_name()
    }

    pub fn kernel_display_label(&self) -> String {
        self.document
            .kernel_display_name()
            .or_else(|| self.document.kernel_name())
            .unwrap_or_else(|| "Python 3".to_string())
    }

    pub fn set_kernel_spec(
        &mut self,
        name: &str,
        display_name: &str,
        language: &str,
    ) -> Result<bool, String> {
        if self.has_running_cells() {
            return Err(
                "Cannot change notebook kernel while cells are running".to_string()
            );
        }
        self.sync_from_rendered_markdown();
        let changed = self.document.set_kernel_spec(name, display_name, language);
        if changed {
            self.rebuild_markdown();
        }
        Ok(changed)
    }

    pub fn save(&mut self) -> std::io::Result<()> {
        self.sync_from_rendered_markdown();
        let json = self
            .to_json()
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
        std::fs::write(&self.path, &json)?;
        self.saved_json = json;
        self.error = None;
        Ok(())
    }

    pub fn to_json(&self) -> Result<String, String> {
        self.document.to_json()
    }

    pub fn rebuild_markdown(&mut self) {
        let rendered = self.document.render_markdown_with_status(
            &self.running_cells,
            &self.execution_started_at,
            &self.completed_elapsed_ms,
        );
        if self.markdown.path == self.path {
            self.markdown.set_source_preserving_view(&rendered.markdown);
        } else {
            self.markdown =
                MarkdownPane::from_source(self.path.clone(), &rendered.markdown);
        }
        self.cell_ranges = rendered.cell_ranges;
        self.refresh_markdown_image_preview_dimensions();
    }

    pub fn set_cell_source(&mut self, cell_index: usize, source: String) -> bool {
        let Some(cell) = self.document.cells.get_mut(cell_index) else {
            return false;
        };
        cell.source = NotebookSource::Text(source);
        self.rebuild_markdown();
        true
    }

    pub fn sync_from_rendered_markdown(&mut self) {
        self.sync_document_sources_from_rendered_markdown();
        self.rebuild_markdown();
    }

    fn sync_document_sources_from_rendered_markdown(&mut self) {
        let rendered_lines = self.markdown.lines.clone();
        let old_cells = self.document.cells.clone();
        let cell_ranges =
            discover_rendered_cell_ranges(&rendered_lines, &self.cell_ranges, &old_cells);
        let old_running = self.running_cells.clone();
        let old_running_runs = self.running_cell_runs.clone();
        let old_started = self.execution_started_at.clone();
        let old_completed = self.completed_elapsed_ms.clone();
        let mut old_to_new = BTreeMap::new();
        let mut cells = Vec::with_capacity(cell_ranges.len());
        let mut remapped_ranges = Vec::with_capacity(cell_ranges.len());

        for mut range in cell_ranges {
            let Some(mut cell) = old_cells.get(range.cell_index).cloned() else {
                continue;
            };
            let slice = rendered_lines
                .get(
                    range.line_start
                        ..=range.line_end.min(rendered_lines.len().saturating_sub(1)),
                )
                .unwrap_or(&[]);
            let source = source_from_rendered_cell(slice, cell.cell_type);
            cell.source = NotebookSource::Text(source);
            let old_index = range.cell_index;
            let new_index = cells.len();
            range.cell_index = new_index;
            old_to_new.insert(old_index, new_index);
            cells.push(cell);
            remapped_ranges.push(range);
        }

        self.document.cells = cells;
        self.cell_ranges = remapped_ranges;
        self.running_cells = old_running
            .into_iter()
            .filter_map(|old_index| old_to_new.get(&old_index).copied())
            .collect();
        self.running_cell_runs.clear();
        for (old_index, run_id) in old_running_runs {
            if let Some(new_index) = old_to_new.get(&old_index).copied() {
                self.running_cell_runs.insert(new_index, run_id);
            }
        }
        self.execution_started_at.clear();
        for (old_index, started_at) in old_started {
            if let Some(new_index) = old_to_new.get(&old_index).copied() {
                self.execution_started_at.insert(new_index, started_at);
            }
        }
        self.completed_elapsed_ms.clear();
        for (old_index, elapsed) in old_completed {
            if let Some(new_index) = old_to_new.get(&old_index).copied() {
                self.completed_elapsed_ms.insert(new_index, elapsed);
            }
        }
    }

    pub fn sync_order_from_rendered_markdown(&mut self) {
        self.sync_document_sources_from_rendered_markdown();
        let mut ordered = self.cell_ranges.clone();
        ordered.sort_by_key(|range| range.line_start);
        if ordered
            .iter()
            .enumerate()
            .all(|(idx, range)| range.cell_index == idx)
        {
            return;
        }
        let old_cells = self.document.cells.clone();
        let old_running = self.running_cells.clone();
        let old_running_runs = self.running_cell_runs.clone();
        let old_started = self.execution_started_at.clone();
        let old_completed = self.completed_elapsed_ms.clone();
        let old_to_new = ordered
            .iter()
            .enumerate()
            .map(|(new_idx, range)| (range.cell_index, new_idx))
            .collect::<BTreeMap<_, _>>();
        self.document.cells = ordered
            .iter()
            .filter_map(|range| old_cells.get(range.cell_index).cloned())
            .collect();
        self.running_cells = old_running
            .into_iter()
            .filter_map(|old_index| old_to_new.get(&old_index).copied())
            .collect();
        self.running_cell_runs.clear();
        for (old_index, run_id) in old_running_runs {
            if let Some(new_index) = old_to_new.get(&old_index).copied() {
                self.running_cell_runs.insert(new_index, run_id);
            }
        }
        self.execution_started_at.clear();
        for (old_index, started_at) in old_started {
            if let Some(new_index) = old_to_new.get(&old_index).copied() {
                self.execution_started_at.insert(new_index, started_at);
            }
        }
        self.completed_elapsed_ms.clear();
        for (old_index, elapsed) in old_completed {
            if let Some(new_idx) = old_to_new.get(&old_index).copied() {
                self.completed_elapsed_ms.insert(new_idx, elapsed);
            }
        }
        self.cell_ranges = ordered
            .into_iter()
            .enumerate()
            .map(|(new_idx, mut range)| {
                range.cell_index = new_idx;
                range
            })
            .collect();
        self.rebuild_markdown();
    }

    pub fn current_cell_index(&self) -> Option<usize> {
        let line = self.markdown.cursor_line;
        self.cell_ranges
            .iter()
            .find(|range| line >= range.line_start && line <= range.line_end)
            .map(|range| range.cell_index)
    }

    pub fn current_cell_type(&self) -> Option<NotebookCellType> {
        self.current_cell_index()
            .and_then(|index| self.document.cells.get(index))
            .map(|cell| cell.cell_type)
    }

    pub fn enter_current_cell_edit_mode(&mut self) {
        let cell_index = self.current_cell_index().unwrap_or(0);
        self.focus_cell(cell_index, true);
    }

    pub fn enter_command_mode(&mut self) {
        self.markdown.enter_notebook_command_mode();
    }

    pub fn select_adjacent_cell(&mut self, delta: isize) -> bool {
        let Some(current) = self.current_cell_index() else {
            return false;
        };
        let target = if delta < 0 {
            current.checked_sub(delta.unsigned_abs())
        } else {
            current.checked_add(delta as usize)
        };
        let Some(target) = target.filter(|target| *target < self.document.cells.len())
        else {
            self.enter_command_mode();
            return false;
        };
        self.focus_cell(target, false);
        self.enter_command_mode();
        true
    }

    pub fn run_cell_at_point(&self, x: f32, y: f32) -> Option<usize> {
        self.markdown.notebook_run_at(x, y)
    }

    pub fn cell_action_at_point(
        &self,
        x: f32,
        y: f32,
    ) -> Option<(usize, NotebookCellAction)> {
        self.markdown.notebook_action_at(x, y)
    }

    pub fn rendered_image_outputs(&self) -> &[NotebookRenderedImageOutput] {
        &self.rendered_images
    }

    fn compute_rendered_image_outputs(&self) -> Vec<NotebookRenderedImageOutput> {
        let mut images = Vec::new();
        for range in &self.cell_ranges {
            let Some(cell) = self.document.cells.get(range.cell_index) else {
                continue;
            };
            if cell.cell_type == NotebookCellType::Markdown {
                let mut attachment_index = 0usize;
                let end = range
                    .line_end
                    .min(self.markdown.lines.len().saturating_sub(1));
                for line in range.line_start..=end {
                    let Some(source_line) = self.markdown.lines.get(line) else {
                        continue;
                    };
                    for attachment_name in attachment_image_references(source_line) {
                        let Some(image) =
                            decoded_notebook_attachment_image(cell, &attachment_name)
                        else {
                            continue;
                        };
                        images.push(NotebookRenderedImageOutput {
                            cell_index: range.cell_index,
                            output_index: attachment_index,
                            attachment_name: Some(attachment_name.clone()),
                            line,
                            image_id: notebook_attachment_image_id(
                                &self.path,
                                cell,
                                attachment_index,
                                &attachment_name,
                                line,
                                &image.mime,
                                image.width,
                                image.height,
                                &image.pixels,
                            ),
                            mime: image.mime,
                            width: image.width,
                            height: image.height,
                            pixels: image.pixels,
                            is_opaque: image.is_opaque,
                        });
                        attachment_index = attachment_index.saturating_add(1);
                    }
                }
                continue;
            }
            if cell.cell_type != NotebookCellType::Code || cell.outputs.is_empty() {
                continue;
            }

            let marker_lines = notebook_output_marker_lines(
                &self.markdown.lines,
                range.line_start,
                range.line_end,
            );
            let mut marker_ix = 0usize;
            for (output_index, output) in cell.outputs.iter().enumerate() {
                let Some(text) = output_text(output) else {
                    continue;
                };
                let marker_line = marker_lines.get(marker_ix).copied();
                if let Some(line) = marker_line {
                    if let Some(image) = decoded_notebook_image_output(output) {
                        images.push(NotebookRenderedImageOutput {
                            cell_index: range.cell_index,
                            output_index,
                            attachment_name: None,
                            line,
                            image_id: notebook_image_output_id(
                                &self.path,
                                cell,
                                output_index,
                                &image.mime,
                                image.width,
                                image.height,
                                &image.pixels,
                            ),
                            mime: image.mime,
                            width: image.width,
                            height: image.height,
                            pixels: image.pixels,
                            is_opaque: image.is_opaque,
                        });
                    }
                }
                marker_ix = marker_ix.saturating_add(output_marker_line_count(&text));
            }
        }
        images
    }

    fn refresh_markdown_image_preview_dimensions(&mut self) {
        self.rendered_images = self.compute_rendered_image_outputs();
        let dimensions = self
            .rendered_images
            .iter()
            .map(|image| (image.line, image.width, image.height));
        self.markdown
            .set_notebook_image_preview_dimensions(dimensions);
    }

    pub fn run_current_cell(&mut self) -> Result<(), String> {
        self.sync_from_rendered_markdown();
        let cell_index = self
            .current_cell_index()
            .or_else(|| {
                self.document
                    .cells
                    .iter()
                    .position(|cell| cell.cell_type == NotebookCellType::Code)
            })
            .ok_or_else(|| "No code cell selected".to_string())?;
        self.run_cell(cell_index)
    }

    pub fn run_linked_cell(&mut self, cell_index: usize) -> Result<(), String> {
        self.sync_from_rendered_markdown();
        self.run_cell(cell_index)
    }

    pub fn prepare_cell_execution(
        &mut self,
        cell_index: usize,
    ) -> Result<NotebookExecutionJob, String> {
        self.sync_from_rendered_markdown();
        let execution_count = next_execution_count(&self.document.cells);
        let job = self.prepare_cell_execution_at(cell_index, execution_count)?;
        self.rebuild_markdown();
        Ok(job)
    }

    pub fn prepare_all_cell_executions(
        &mut self,
    ) -> Result<Vec<NotebookExecutionJob>, String> {
        self.sync_from_rendered_markdown();
        if !self.running_cells.is_empty() {
            return Err("Notebook already has running cells".to_string());
        }
        let cell_indices = self
            .document
            .cells
            .iter()
            .enumerate()
            .filter_map(|(index, cell)| {
                (cell.cell_type == NotebookCellType::Code).then_some(index)
            })
            .collect::<Vec<_>>();
        if cell_indices.is_empty() {
            return Err("No code cells in notebook".to_string());
        }

        let mut execution_count = next_execution_count(&self.document.cells);
        let mut jobs = Vec::with_capacity(cell_indices.len());
        for cell_index in cell_indices {
            jobs.push(self.prepare_cell_execution_at(cell_index, execution_count)?);
            execution_count = execution_count.saturating_add(1);
        }
        self.rebuild_markdown();
        Ok(jobs)
    }

    pub fn prepare_cell_and_below_executions(
        &mut self,
    ) -> Result<Vec<NotebookExecutionJob>, String> {
        let selected = self.current_cell_index();
        self.prepare_cell_and_below_executions_from(selected.unwrap_or(0))
    }

    pub fn prepare_cell_and_below_executions_from(
        &mut self,
        start_index: usize,
    ) -> Result<Vec<NotebookExecutionJob>, String> {
        self.sync_from_rendered_markdown();
        if !self.running_cells.is_empty() {
            return Err("Notebook already has running cells".to_string());
        }
        let start = start_index.min(self.document.cells.len());
        let cell_indices = self
            .document
            .cells
            .iter()
            .enumerate()
            .filter_map(|(index, cell)| {
                (index >= start && cell.cell_type == NotebookCellType::Code)
                    .then_some(index)
            })
            .collect::<Vec<_>>();
        if cell_indices.is_empty() {
            return Err("No code cells at or below the current cell".to_string());
        }

        let mut execution_count = next_execution_count(&self.document.cells);
        let mut jobs = Vec::with_capacity(cell_indices.len());
        for cell_index in cell_indices {
            jobs.push(self.prepare_cell_execution_at(cell_index, execution_count)?);
            execution_count = execution_count.saturating_add(1);
        }
        self.rebuild_markdown();
        Ok(jobs)
    }

    pub fn clear_current_output(&mut self) -> Result<usize, String> {
        let selected = self.current_cell_index();
        let cell_index =
            selected.ok_or_else(|| "No notebook cell selected".to_string())?;
        self.clear_output_at(cell_index)
    }

    pub fn clear_output_at(&mut self, cell_index: usize) -> Result<usize, String> {
        self.sync_from_rendered_markdown();
        if !self.running_cells.is_empty() {
            return Err(
                "Cannot clear notebook output while cells are running".to_string()
            );
        }
        let Some(cell) = self.document.cells.get_mut(cell_index) else {
            return Err("No notebook cell selected".to_string());
        };
        if cell.cell_type != NotebookCellType::Code {
            return Err("Current notebook cell is not code".to_string());
        }
        cell.outputs.clear();
        cell.execution_count = None;
        self.completed_elapsed_ms.remove(&cell_index);
        self.rebuild_markdown();
        Ok(cell_index)
    }

    pub fn clear_all_outputs(&mut self) -> Result<usize, String> {
        self.sync_from_rendered_markdown();
        if !self.running_cells.is_empty() {
            return Err(
                "Cannot clear notebook outputs while cells are running".to_string()
            );
        }

        let mut cleared = 0usize;
        for cell in &mut self.document.cells {
            if !cell.outputs.is_empty() || cell.execution_count.is_some() {
                cell.outputs.clear();
                cell.execution_count = None;
                cleared = cleared.saturating_add(1);
            }
        }
        self.completed_elapsed_ms.clear();
        self.rebuild_markdown();
        Ok(cleared)
    }

    pub fn insert_cell_above(&mut self, kind: NotebookCellType) -> Result<usize, String> {
        self.insert_cell_relative(kind, InsertCellPosition::Above)
    }

    pub fn insert_cell_below(&mut self, kind: NotebookCellType) -> Result<usize, String> {
        self.insert_cell_relative(kind, InsertCellPosition::Below)
    }

    pub fn delete_current_cell(&mut self) -> Result<usize, String> {
        let selected = self.current_cell_index();
        self.sync_from_rendered_markdown();
        self.ensure_structure_edit_allowed()?;
        if self.document.cells.is_empty() {
            return Err("No notebook cell to delete".to_string());
        }
        let cell_index = selected
            .unwrap_or_else(|| self.document.cells.len().saturating_sub(1))
            .min(self.document.cells.len().saturating_sub(1));
        self.document.cells.remove(cell_index);
        shift_elapsed_after_delete(&mut self.completed_elapsed_ms, cell_index);
        self.rebuild_markdown();
        if !self.document.cells.is_empty() {
            self.focus_cell(cell_index.min(self.document.cells.len() - 1), false);
        }
        Ok(cell_index)
    }

    pub fn move_current_cell_up(&mut self) -> Result<usize, String> {
        self.move_current_cell(-1)
    }

    pub fn move_current_cell_down(&mut self) -> Result<usize, String> {
        self.move_current_cell(1)
    }

    pub fn has_running_cells(&self) -> bool {
        !self.running_cells.is_empty()
    }

    fn insert_cell_relative(
        &mut self,
        kind: NotebookCellType,
        position: InsertCellPosition,
    ) -> Result<usize, String> {
        let anchor = self.current_cell_index();
        self.sync_from_rendered_markdown();
        self.ensure_structure_edit_allowed()?;
        let insert_at = match (position, anchor) {
            (InsertCellPosition::Above, Some(index)) => index,
            (InsertCellPosition::Below, Some(index)) => index.saturating_add(1),
            (InsertCellPosition::Above, None) => 0,
            (InsertCellPosition::Below, None) => self.document.cells.len(),
        }
        .min(self.document.cells.len());

        self.document
            .cells
            .insert(insert_at, new_notebook_cell(kind));
        self.document.ensure_cell_ids();
        shift_elapsed_after_insert(&mut self.completed_elapsed_ms, insert_at);
        self.rebuild_markdown();
        self.focus_cell(insert_at, true);
        Ok(insert_at)
    }

    fn move_current_cell(&mut self, delta: isize) -> Result<usize, String> {
        let selected = self.current_cell_index();
        self.sync_from_rendered_markdown();
        self.ensure_structure_edit_allowed()?;
        let Some(cell_index) = selected else {
            return Err("No notebook cell selected".to_string());
        };
        if cell_index >= self.document.cells.len() {
            return Err("No notebook cell selected".to_string());
        }
        let target_index = if delta < 0 {
            cell_index
                .checked_sub(delta.unsigned_abs())
                .ok_or_else(|| "Notebook cell is already first".to_string())?
        } else {
            cell_index.saturating_add(delta as usize)
        };
        if target_index >= self.document.cells.len() {
            return Err("Notebook cell is already last".to_string());
        }
        self.document.cells.swap(cell_index, target_index);
        swap_elapsed_indices(&mut self.completed_elapsed_ms, cell_index, target_index);
        self.rebuild_markdown();
        self.focus_cell(target_index, false);
        Ok(target_index)
    }

    fn ensure_structure_edit_allowed(&self) -> Result<(), String> {
        if self.running_cells.is_empty() {
            Ok(())
        } else {
            Err(
                "Wait for running notebook cells to finish before editing cells"
                    .to_string(),
            )
        }
    }

    fn focus_cell(&mut self, cell_index: usize, enter_insert: bool) {
        let Some(range) = self.cell_ranges.get(cell_index) else {
            self.markdown.cursor_line = 0;
            self.markdown.cursor_col = 0;
            return;
        };
        let line = match range.kind {
            NotebookCellType::Code
            | NotebookCellType::Raw
            | NotebookCellType::Markdown => {
                range.line_start.saturating_add(1).min(range.line_end)
            }
        }
        .min(self.markdown.lines.len().saturating_sub(1));
        self.markdown.cursor_line = line;
        self.markdown.cursor_col = self
            .markdown
            .lines
            .get(line)
            .map(String::len)
            .unwrap_or_default();
        if enter_insert {
            self.markdown.enter_insert();
        }
    }

    fn prepare_cell_execution_at(
        &mut self,
        cell_index: usize,
        execution_count: u32,
    ) -> Result<NotebookExecutionJob, String> {
        if self.running_cells.contains(&cell_index) {
            return Err("Notebook cell is already running".to_string());
        }
        let (cell_id, language, source) = {
            let Some(cell) = self.document.cells.get(cell_index) else {
                return Err("No notebook cell at cursor".to_string());
            };
            if cell.cell_type != NotebookCellType::Code {
                return Err("Current notebook cell is not code".to_string());
            }
            let cell_id = notebook_cell_id(cell)
                .map(ToString::to_string)
                .ok_or_else(|| "Notebook cell has no stable id".to_string())?;
            (
                cell_id,
                self.document.cell_language(cell),
                cell.source.as_str().to_string(),
            )
        };
        let kernel_name = self.document.kernel_name();
        let fallback_script = self.fallback_script_for_cell(cell_index, &language);
        let run_id = self.next_execution_run_id;
        self.next_execution_run_id = self.next_execution_run_id.saturating_add(1).max(1);
        if let Some(cell) = self.document.cells.get_mut(cell_index) {
            cell.outputs.clear();
            cell.execution_count = None;
        }
        self.completed_elapsed_ms.remove(&cell_index);
        self.running_cells.insert(cell_index);
        self.running_cell_runs.insert(cell_index, run_id);
        self.execution_started_at.insert(cell_index, Instant::now());
        self.rebuild_markdown();
        Ok(NotebookExecutionJob {
            path: self.path.clone(),
            cell_index,
            cell_id,
            run_id,
            execution_count,
            language,
            kernel_name,
            source,
            fallback_script,
        })
    }

    pub fn apply_execution_result(
        &mut self,
        result: NotebookExecutionResult,
    ) -> Result<(), String> {
        let status = self.apply_execution_result_without_rebuild(result);
        self.rebuild_markdown();
        status
    }

    pub fn apply_execution_result_without_rebuild(
        &mut self,
        result: NotebookExecutionResult,
    ) -> Result<(), String> {
        let Some(cell_index) =
            self.execution_cell_index(&result.cell_id, result.cell_index)
        else {
            self.clear_running_state_for_run(result.run_id);
            return Err("Notebook cell disappeared before execution finished".to_string());
        };
        if self.running_cell_runs.get(&cell_index) != Some(&result.run_id) {
            return Ok(());
        }
        let Some(cell) = self.document.cells.get_mut(cell_index) else {
            self.clear_running_state_for_run(result.run_id);
            return Err("Notebook cell disappeared before execution finished".to_string());
        };
        cell.outputs = result.outputs;
        cell.execution_count = Some(result.execution_count);
        self.running_cells.remove(&cell_index);
        self.running_cell_runs.remove(&cell_index);
        self.execution_started_at.remove(&cell_index);
        self.completed_elapsed_ms
            .insert(cell_index, result.elapsed_ms);
        result.status
    }

    pub fn apply_execution_chunk(
        &mut self,
        chunk: NotebookExecutionChunk,
    ) -> Result<(), String> {
        let status = self.apply_execution_chunk_without_rebuild(chunk);
        self.rebuild_markdown();
        status
    }

    pub fn apply_execution_chunk_without_rebuild(
        &mut self,
        chunk: NotebookExecutionChunk,
    ) -> Result<(), String> {
        let Some(cell_index) =
            self.execution_cell_index(&chunk.cell_id, chunk.cell_index)
        else {
            self.clear_running_state_for_run(chunk.run_id);
            return Err("Notebook cell disappeared before execution finished".to_string());
        };
        if self.running_cell_runs.get(&cell_index) != Some(&chunk.run_id) {
            return Ok(());
        }
        let Some(cell) = self.document.cells.get_mut(cell_index) else {
            self.clear_running_state_for_run(chunk.run_id);
            return Err("Notebook cell disappeared before execution finished".to_string());
        };
        append_stream_output(&mut cell.outputs, chunk.stream, &chunk.text);
        Ok(())
    }

    pub fn apply_display_update(
        &mut self,
        update: NotebookDisplayUpdate,
    ) -> Result<usize, String> {
        let replaced = self.apply_display_update_without_rebuild(update);
        if matches!(replaced.as_ref(), Ok(count) if *count > 0) {
            self.rebuild_markdown();
        }
        replaced
    }

    pub fn apply_display_update_without_rebuild(
        &mut self,
        update: NotebookDisplayUpdate,
    ) -> Result<usize, String> {
        if update.display_id.trim().is_empty() {
            return Err("Notebook display update has no display_id".to_string());
        }
        let Some(cell_index) =
            self.execution_cell_index(&update.cell_id, update.cell_index)
        else {
            return Ok(0);
        };
        if self.running_cell_runs.get(&cell_index) != Some(&update.run_id) {
            return Ok(0);
        }

        let mut replaced = 0usize;
        for cell in &mut self.document.cells {
            for output in &mut cell.outputs {
                if output_display_id(output) == Some(update.display_id.as_str()) {
                    *output = update.output.clone();
                    replaced = replaced.saturating_add(1);
                }
            }
        }
        Ok(replaced)
    }

    pub fn run_cell(&mut self, cell_index: usize) -> Result<(), String> {
        self.sync_from_rendered_markdown();
        let execution_count = next_execution_count(&self.document.cells);
        let language = self
            .document
            .cells
            .get(cell_index)
            .map(|cell| self.document.cell_language(cell))
            .unwrap_or_else(|| "python".to_string());
        let fallback_script = self.fallback_script_for_cell(cell_index, &language);
        let Some(cell) = self.document.cells.get_mut(cell_index) else {
            return Err("No notebook cell at cursor".to_string());
        };
        if cell.cell_type != NotebookCellType::Code {
            return Err("Current notebook cell is not code".to_string());
        }
        cell.outputs.clear();
        cell.execution_count = None;
        self.completed_elapsed_ms.remove(&cell_index);
        let started_at = Instant::now();
        let mut command = command_for_language(&language)?;
        if let Some(parent) = self
            .path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            command.current_dir(parent);
        }
        let output = command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                if let Some(stdin) = child.stdin.as_mut() {
                    stdin.write_all(fallback_script.as_bytes())?;
                }
                child.wait_with_output()
            })
            .map_err(|err| err.to_string())?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let outputs = outputs_from_process(stdout, stderr);
        cell.outputs = outputs;
        cell.execution_count = Some(execution_count);
        self.completed_elapsed_ms
            .insert(cell_index, started_at.elapsed().as_millis());
        self.rebuild_markdown();
        if output.status.success() {
            Ok(())
        } else {
            Err(format!("Python exited with {}", output.status))
        }
    }

    fn execution_cell_index(
        &self,
        cell_id: &str,
        fallback_index: usize,
    ) -> Option<usize> {
        if cell_id.is_empty() {
            return self
                .document
                .cells
                .get(fallback_index)
                .map(|_| fallback_index);
        }
        self.document
            .cells
            .iter()
            .position(|cell| notebook_cell_id(cell) == Some(cell_id))
    }

    fn clear_running_state_for_run(&mut self, run_id: u64) {
        let indices = self
            .running_cell_runs
            .iter()
            .filter_map(|(index, active_run)| (*active_run == run_id).then_some(*index))
            .collect::<Vec<_>>();
        for index in indices {
            self.running_cells.remove(&index);
            self.running_cell_runs.remove(&index);
            self.execution_started_at.remove(&index);
            self.completed_elapsed_ms.remove(&index);
        }
    }

    fn fallback_script_for_cell(&self, cell_index: usize, language: &str) -> String {
        if is_shell_language(language) {
            return self.shell_script_for_cell(cell_index);
        }
        self.python_script_for_cell(cell_index)
    }

    fn python_script_for_cell(&self, cell_index: usize) -> String {
        let mut prelude = String::new();
        for cell in self.document.cells.iter().take(cell_index) {
            if cell.cell_type == NotebookCellType::Code {
                prelude.push_str(cell.source.as_str());
                ensure_trailing_newline(&mut prelude);
            }
        }
        let current = self
            .document
            .cells
            .get(cell_index)
            .map(|cell| cell.source.as_str())
            .unwrap_or_default();
        format!(
            r#"import contextlib as __neoism_contextlib
import io as __neoism_io
__neoism_prelude = {prelude:?}
with __neoism_contextlib.redirect_stdout(__neoism_io.StringIO()), __neoism_contextlib.redirect_stderr(__neoism_io.StringIO()):
    exec(__neoism_prelude, globals())
__neoism_current = {current:?}
exec(__neoism_current, globals())
"#
        )
    }

    fn shell_script_for_cell(&self, cell_index: usize) -> String {
        let mut script = String::new();
        for cell in self.document.cells.iter().take(cell_index + 1) {
            if cell.cell_type == NotebookCellType::Code {
                script.push_str(cell.source.as_str());
                ensure_trailing_newline(&mut script);
            }
        }
        script
    }
}

#[derive(Clone, Debug)]
pub struct NotebookExecutionJob {
    pub path: PathBuf,
    pub cell_index: usize,
    pub cell_id: String,
    pub run_id: u64,
    pub execution_count: u32,
    pub language: String,
    pub kernel_name: Option<String>,
    pub source: String,
    pub fallback_script: String,
}

impl NotebookExecutionJob {
    pub fn run(self) -> NotebookExecutionResult {
        let started_at = Instant::now();
        let status = run_execution_job(&self);
        let elapsed_ms = started_at.elapsed().as_millis();
        match status {
            Ok((stdout, stderr, success)) => NotebookExecutionResult {
                cell_index: self.cell_index,
                cell_id: self.cell_id,
                run_id: self.run_id,
                execution_count: self.execution_count,
                outputs: outputs_from_process(stdout, stderr),
                status: if success {
                    Ok(())
                } else {
                    Err("Process exited with a non-zero status".to_string())
                },
                elapsed_ms,
            },
            Err(err) => NotebookExecutionResult {
                cell_index: self.cell_index,
                cell_id: self.cell_id,
                run_id: self.run_id,
                execution_count: self.execution_count,
                outputs: vec![serde_json::json!({
                    "output_type": "stream",
                    "name": "stderr",
                    "text": err.clone(),
                })],
                status: Err(err),
                elapsed_ms,
            },
        }
    }

    pub fn run_streaming(self, send: impl Fn(NotebookExecutionEvent)) {
        let started_at = Instant::now();
        let result = match run_execution_job_streaming(&self, &send) {
            Ok((outputs, success)) => NotebookExecutionResult {
                cell_index: self.cell_index,
                cell_id: self.cell_id.clone(),
                run_id: self.run_id,
                execution_count: self.execution_count,
                outputs,
                status: if success {
                    Ok(())
                } else {
                    Err("Process exited with a non-zero status".to_string())
                },
                elapsed_ms: started_at.elapsed().as_millis(),
            },
            Err(err) => NotebookExecutionResult {
                cell_index: self.cell_index,
                cell_id: self.cell_id.clone(),
                run_id: self.run_id,
                execution_count: self.execution_count,
                outputs: vec![serde_json::json!({
                    "output_type": "stream",
                    "name": "stderr",
                    "text": err.clone(),
                })],
                status: Err(err),
                elapsed_ms: started_at.elapsed().as_millis(),
            },
        };
        send(NotebookExecutionEvent::Finished(result));
    }
}

impl NotebookDocument {
    pub fn from_json(source: &str) -> Result<Self, String> {
        serde_json::from_str(source).map_err(|err| err.to_string())
    }

    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self)
            .map(|json| format!("{json}\n"))
            .map_err(|err| err.to_string())
    }

    fn ensure_cell_ids(&mut self) {
        let mut used = BTreeSet::new();
        for (index, cell) in self.cells.iter_mut().enumerate() {
            if let Some(id) = notebook_cell_id(cell).map(ToString::to_string) {
                if used.insert(id) {
                    continue;
                }
            }
            let id = generated_cell_id(index, cell, &used);
            cell.extra
                .insert("id".to_string(), Value::String(id.clone()));
            used.insert(id);
        }
    }

    fn kernel_display_name(&self) -> Option<String> {
        self.metadata
            .get("kernelspec")
            .and_then(|kernelspec| kernelspec.get("display_name"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(ToString::to_string)
    }

    fn set_kernel_spec(
        &mut self,
        name: &str,
        display_name: &str,
        language: &str,
    ) -> bool {
        let name = name.trim();
        let display_name = display_name.trim();
        let language = language.trim();
        if name.is_empty() || display_name.is_empty() || language.is_empty() {
            return false;
        }
        let previous = (
            self.kernel_name(),
            self.kernel_display_name(),
            self.metadata
                .get("kernelspec")
                .and_then(|kernelspec| kernelspec.get("language"))
                .and_then(Value::as_str)
                .map(ToString::to_string),
        );
        let metadata = ensure_json_object(&mut self.metadata);
        metadata.insert(
            "kernelspec".to_string(),
            serde_json::json!({
                "name": name,
                "display_name": display_name,
                "language": language,
            }),
        );
        let language_info = metadata
            .entry("language_info".to_string())
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
        let language_info = ensure_json_object(language_info);
        language_info.insert("name".to_string(), Value::String(language.to_string()));
        previous
            != (
                Some(name.to_string()),
                Some(display_name.to_string()),
                Some(language.to_string()),
            )
    }

    pub fn render_markdown(&self) -> NotebookRenderedSource {
        self.render_markdown_with_running(&BTreeSet::new())
    }

    pub fn render_markdown_with_running(
        &self,
        running_cells: &BTreeSet<usize>,
    ) -> NotebookRenderedSource {
        self.render_markdown_with_status(
            running_cells,
            &BTreeMap::new(),
            &BTreeMap::new(),
        )
    }

    pub fn render_markdown_with_status(
        &self,
        running_cells: &BTreeSet<usize>,
        _execution_started_at: &BTreeMap<usize, Instant>,
        completed_elapsed_ms: &BTreeMap<usize, u128>,
    ) -> NotebookRenderedSource {
        let mut markdown = String::new();
        let mut cell_ranges = Vec::with_capacity(self.cells.len());

        for (cell_index, cell) in self.cells.iter().enumerate() {
            let line_start = markdown_line_count(&markdown);
            let rendered_cell_id = encoded_notebook_cell_id(cell).unwrap_or_default();
            match cell.cell_type {
                NotebookCellType::Markdown => {
                    markdown.push_str(NOTEBOOK_MARKDOWN_CELL_MARKER);
                    markdown.push_str(&cell_index.to_string());
                    markdown.push_str(" markdown neoism_notebook_id=");
                    markdown.push_str(&rendered_cell_id);
                    markdown.push('\n');
                    append_cell_source(&mut markdown, cell.source.as_str());
                }
                NotebookCellType::Code => {
                    let lang = self.cell_language(cell);
                    let is_running = running_cells.contains(&cell_index);
                    let state = if is_running {
                        "running".to_string()
                    } else {
                        "idle".to_string()
                    };
                    let execution = if is_running {
                        "*".to_string()
                    } else {
                        cell.execution_count
                            .map(|count| count.to_string())
                            .unwrap_or_else(|| "_".to_string())
                    };
                    markdown.push_str("```");
                    markdown.push_str(&lang);
                    markdown.push_str(&format!(
                        " neoism_notebook_cell={cell_index} neoism_notebook_id={rendered_cell_id} neoism_state={state} neoism_count={execution}"
                    ));
                    markdown.push('\n');
                    append_cell_source(&mut markdown, cell.source.as_str());
                    markdown.push_str("```\n");
                    append_outputs(
                        &mut markdown,
                        cell,
                        if is_running {
                            None
                        } else {
                            completed_elapsed_ms.get(&cell_index).copied()
                        },
                        is_running,
                    );
                }
                NotebookCellType::Raw => {
                    markdown.push_str("```text");
                    markdown.push_str(&format!(
                        " neoism_notebook_cell={cell_index} neoism_notebook_id={rendered_cell_id} neoism_notebook_kind=raw"
                    ));
                    markdown.push('\n');
                    append_cell_source(&mut markdown, cell.source.as_str());
                    markdown.push_str("```\n");
                }
            }
            let line_end = markdown_line_count(&markdown).saturating_sub(1);
            cell_ranges.push(NotebookCellRange {
                cell_index,
                kind: cell.cell_type,
                line_start,
                line_end,
                run_line: None,
            });
        }

        if markdown.is_empty() {
            markdown.push_str(
                "# Empty notebook\n\nUse the notebook commands to add a cell.\n",
            );
        }

        NotebookRenderedSource {
            markdown,
            cell_ranges,
        }
    }

    fn cell_language(&self, cell: &NotebookCell) -> String {
        cell.metadata
            .get("language")
            .and_then(Value::as_str)
            .or_else(|| {
                self.metadata
                    .get("language_info")
                    .and_then(|info| info.get("name"))
                    .and_then(Value::as_str)
            })
            .or_else(|| {
                self.metadata
                    .get("kernelspec")
                    .and_then(|kernelspec| kernelspec.get("language"))
                    .and_then(Value::as_str)
            })
            .unwrap_or("python")
            .to_string()
    }

    fn kernel_name(&self) -> Option<String> {
        self.metadata
            .get("kernelspec")
            .and_then(|kernelspec| kernelspec.get("name"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(ToString::to_string)
    }
}

fn ensure_json_object(value: &mut Value) -> &mut serde_json::Map<String, Value> {
    if !value.is_object() {
        *value = Value::Object(serde_json::Map::new());
    }
    value
        .as_object_mut()
        .expect("value was normalized to object")
}

impl Default for NotebookDocument {
    fn default() -> Self {
        Self {
            cells: Vec::new(),
            metadata: Value::Object(serde_json::Map::new()),
            nbformat: default_nbformat(),
            nbformat_minor: default_nbformat_minor(),
        }
    }
}

impl NotebookSource {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Text(text) => text,
        }
    }
}

impl Default for NotebookSource {
    fn default() -> Self {
        Self::Text(String::new())
    }
}

impl Serialize for NotebookSource {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for NotebookSource {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        match value {
            Value::String(text) => Ok(Self::Text(text)),
            Value::Array(parts) => {
                let mut text = String::new();
                for part in parts {
                    let Some(part) = part.as_str() else {
                        return Err(serde::de::Error::custom(
                            "notebook source arrays must contain strings",
                        ));
                    };
                    text.push_str(part);
                }
                Ok(Self::Text(text))
            }
            _ => Err(serde::de::Error::custom(
                "notebook source must be a string or string array",
            )),
        }
    }
}

pub fn is_notebook_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("ipynb"))
        .unwrap_or(false)
}

fn new_notebook_cell(kind: NotebookCellType) -> NotebookCell {
    NotebookCell {
        cell_type: kind,
        metadata: Value::Object(serde_json::Map::new()),
        source: NotebookSource::Text(String::new()),
        execution_count: None,
        outputs: Vec::new(),
        extra: serde_json::Map::new(),
    }
}

fn shift_elapsed_after_insert(map: &mut BTreeMap<usize, u128>, at: usize) {
    let old = std::mem::take(map);
    *map = old
        .into_iter()
        .map(|(index, elapsed)| {
            let next_index = if index >= at {
                index.saturating_add(1)
            } else {
                index
            };
            (next_index, elapsed)
        })
        .collect();
}

fn shift_elapsed_after_delete(map: &mut BTreeMap<usize, u128>, at: usize) {
    let old = std::mem::take(map);
    *map = old
        .into_iter()
        .filter_map(|(index, elapsed)| {
            if index == at {
                None
            } else if index > at {
                Some((index - 1, elapsed))
            } else {
                Some((index, elapsed))
            }
        })
        .collect();
}

fn swap_elapsed_indices(map: &mut BTreeMap<usize, u128>, a: usize, b: usize) {
    if a == b {
        return;
    }
    let a_value = map.remove(&a);
    let b_value = map.remove(&b);
    if let Some(value) = a_value {
        map.insert(b, value);
    }
    if let Some(value) = b_value {
        map.insert(a, value);
    }
}

mod execution;
mod image;
mod output_render;
mod render;
mod text_util;

pub(crate) use execution::*;
pub(crate) use image::*;
pub(crate) use output_render::*;
pub(crate) use render::*;
pub(crate) use text_util::*;

fn default_nbformat() -> u8 {
    4
}

fn default_nbformat_minor() -> u8 {
    5
}

#[cfg(test)]
mod tests;
