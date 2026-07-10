
use super::*;

#[test]
fn parses_array_sources_and_renders_code() {
    let raw = r##"{
          "cells": [
            {"cell_type":"markdown","metadata":{},"source":["# Title\n", "body"]},
            {"cell_type":"code","metadata":{},"execution_count":1,"source":["print('hi')\n"],"outputs":[{"output_type":"stream","name":"stdout","text":"hi\n"}]}
          ],
          "metadata": {"language_info":{"name":"python"}},
          "nbformat": 4,
          "nbformat_minor": 5
        }"##;
    let doc = NotebookDocument::from_json(raw).unwrap();
    let rendered = doc.render_markdown();
    assert!(rendered.markdown.contains("# Title"));
    assert!(rendered.markdown.contains("```python"));
    assert!(rendered.markdown.contains("print('hi')"));
    assert!(rendered.markdown.contains("hi"));
    assert_eq!(rendered.cell_ranges.len(), 2);
}

#[test]
fn from_document_assigns_stable_cell_ids() {
    let mut doc = NotebookDocument::default();
    doc.cells.push(NotebookCell {
        cell_type: NotebookCellType::Code,
        metadata: Value::Object(serde_json::Map::new()),
        source: NotebookSource::Text("print('hi')\n".to_string()),
        execution_count: None,
        outputs: Vec::new(),
        extra: serde_json::Map::new(),
    });

    let pane = NotebookPane::from_document(
        PathBuf::from("test.ipynb"),
        doc,
        String::new(),
        None,
    );

    assert!(notebook_cell_id(&pane.document.cells[0]).is_some());
    assert!(pane.document.to_json().unwrap().contains("\"id\""));
}

#[test]
fn set_kernel_spec_updates_notebook_metadata() {
    let mut doc = NotebookDocument::default();
    doc.metadata = serde_json::json!({
        "language_info": {
            "name": "python",
            "version": "3.11"
        },
        "custom": true
    });
    doc.cells.push(new_notebook_cell(NotebookCellType::Code));
    let mut pane = NotebookPane::from_document(
        PathBuf::from("test.ipynb"),
        doc,
        String::new(),
        None,
    );

    assert!(pane
        .set_kernel_spec("conda-analysis", "Python (analysis)", "python")
        .unwrap());

    assert_eq!(pane.kernel_name().as_deref(), Some("conda-analysis"));
    assert_eq!(pane.kernel_display_label(), "Python (analysis)");
    assert_eq!(
        pane.document
            .metadata
            .get("language_info")
            .and_then(|info| info.get("version"))
            .and_then(Value::as_str),
        Some("3.11")
    );
    assert_eq!(
        pane.document
            .metadata
            .get("custom")
            .and_then(Value::as_bool),
        Some(true)
    );
}

#[test]
fn prepare_all_cell_executions_reserves_counts_and_marks_running() {
    let mut doc = NotebookDocument::default();
    doc.metadata = serde_json::json!({"language_info":{"name":"python"}});
    doc.cells.push(NotebookCell {
        cell_type: NotebookCellType::Code,
        metadata: Value::Object(serde_json::Map::new()),
        source: NotebookSource::Text("a = 1\n".to_string()),
        execution_count: Some(4),
        outputs: vec![serde_json::json!({
            "output_type":"stream",
            "name":"stdout",
            "text":"old\n"
        })],
        extra: serde_json::Map::new(),
    });
    doc.cells.push(NotebookCell {
        cell_type: NotebookCellType::Markdown,
        metadata: Value::Object(serde_json::Map::new()),
        source: NotebookSource::Text("notes".to_string()),
        execution_count: None,
        outputs: Vec::new(),
        extra: serde_json::Map::new(),
    });
    doc.cells.push(NotebookCell {
        cell_type: NotebookCellType::Code,
        metadata: Value::Object(serde_json::Map::new()),
        source: NotebookSource::Text("print(a)\n".to_string()),
        execution_count: None,
        outputs: Vec::new(),
        extra: serde_json::Map::new(),
    });
    let mut pane = NotebookPane::from_document(
        PathBuf::from("test.ipynb"),
        doc,
        String::new(),
        None,
    );

    let jobs = pane.prepare_all_cell_executions().unwrap();

    assert_eq!(jobs.len(), 2);
    assert_eq!(jobs[0].cell_index, 0);
    assert_eq!(jobs[0].execution_count, 5);
    assert_eq!(jobs[1].cell_index, 2);
    assert_eq!(jobs[1].execution_count, 6);
    assert_eq!(pane.running_cells, BTreeSet::from([0, 2]));
    assert_eq!(pane.running_cell_runs.get(&0), Some(&jobs[0].run_id));
    assert_eq!(pane.running_cell_runs.get(&2), Some(&jobs[1].run_id));
    assert!(pane.document.cells[0].outputs.is_empty());
    assert_eq!(pane.document.cells[0].execution_count, None);
    assert!(pane
        .markdown
        .lines
        .iter()
        .any(|line| line.contains("neoism_state=running")));
}

#[test]
fn prepare_cell_and_below_executions_starts_at_selected_cell() {
    let mut doc = NotebookDocument::default();
    doc.metadata = serde_json::json!({"language_info":{"name":"python"}});
    for kind in [
        NotebookCellType::Code,
        NotebookCellType::Markdown,
        NotebookCellType::Code,
        NotebookCellType::Code,
    ] {
        let mut cell = new_notebook_cell(kind);
        cell.source = NotebookSource::Text(format!("{kind:?}\n"));
        doc.cells.push(cell);
    }
    doc.cells[0].execution_count = Some(7);
    let mut pane = NotebookPane::from_document(
        PathBuf::from("test.ipynb"),
        doc,
        String::new(),
        None,
    );
    pane.markdown.cursor_line = pane.cell_ranges[1].line_start;

    let jobs = pane.prepare_cell_and_below_executions().unwrap();

    assert_eq!(jobs.len(), 2);
    assert_eq!(jobs[0].cell_index, 2);
    assert_eq!(jobs[0].execution_count, 8);
    assert_eq!(jobs[1].cell_index, 3);
    assert_eq!(jobs[1].execution_count, 9);
    assert_eq!(pane.running_cells, BTreeSet::from([2, 3]));
}

#[test]
fn prepare_execution_uses_notebook_kernelspec_metadata() {
    let mut doc = NotebookDocument::default();
    doc.metadata = serde_json::json!({
        "kernelspec": {
            "name": "conda-env-analysis-py",
            "display_name": "Python (analysis)",
            "language": "python"
        }
    });
    doc.cells.push(NotebookCell {
        cell_type: NotebookCellType::Code,
        metadata: Value::Object(serde_json::Map::new()),
        source: NotebookSource::Text("x = 1\n".to_string()),
        execution_count: None,
        outputs: Vec::new(),
        extra: serde_json::Map::new(),
    });
    let mut pane = NotebookPane::from_document(
        PathBuf::from("custom.ipynb"),
        doc,
        String::new(),
        None,
    );

    let job = pane.prepare_cell_execution(0).unwrap();

    assert_eq!(job.language, "python");
    assert_eq!(job.kernel_name.as_deref(), Some("conda-env-analysis-py"));
}

#[test]
fn clear_all_outputs_removes_outputs_counts_and_elapsed() {
    let mut doc = NotebookDocument::default();
    doc.metadata = serde_json::json!({"language_info":{"name":"python"}});
    doc.cells.push(NotebookCell {
        cell_type: NotebookCellType::Code,
        metadata: Value::Object(serde_json::Map::new()),
        source: NotebookSource::Text("print('hi')\n".to_string()),
        execution_count: Some(2),
        outputs: vec![serde_json::json!({
            "output_type":"stream",
            "name":"stdout",
            "text":"hi\n"
        })],
        extra: serde_json::Map::new(),
    });
    doc.cells.push(NotebookCell {
        cell_type: NotebookCellType::Code,
        metadata: Value::Object(serde_json::Map::new()),
        source: NotebookSource::Text("1 + 1\n".to_string()),
        execution_count: Some(3),
        outputs: Vec::new(),
        extra: serde_json::Map::new(),
    });
    let mut pane = NotebookPane::from_document(
        PathBuf::from("test.ipynb"),
        doc,
        String::new(),
        None,
    );
    pane.completed_elapsed_ms.insert(0, 40);
    pane.completed_elapsed_ms.insert(1, 50);

    let cleared = pane.clear_all_outputs().unwrap();

    assert_eq!(cleared, 2);
    assert!(pane
        .document
        .cells
        .iter()
        .all(|cell| cell.outputs.is_empty()));
    assert!(pane
        .document
        .cells
        .iter()
        .all(|cell| cell.execution_count.is_none()));
    assert!(pane.completed_elapsed_ms.is_empty());
    assert!(pane
        .markdown
        .lines
        .iter()
        .all(|line| !line.contains("neoism_notebook_output")));
}

#[test]
fn clear_current_output_only_clears_selected_cell() {
    let mut doc = NotebookDocument::default();
    doc.metadata = serde_json::json!({"language_info":{"name":"python"}});
    for label in ["first", "second"] {
        let mut cell = new_notebook_cell(NotebookCellType::Code);
        cell.source = NotebookSource::Text(format!("print('{label}')"));
        cell.execution_count = Some(1);
        cell.outputs = vec![serde_json::json!({
            "output_type":"stream",
            "name":"stdout",
            "text":format!("{label}\n")
        })];
        doc.cells.push(cell);
    }
    let mut pane = NotebookPane::from_document(
        PathBuf::from("test.ipynb"),
        doc,
        String::new(),
        None,
    );
    pane.completed_elapsed_ms.insert(0, 10);
    pane.completed_elapsed_ms.insert(1, 20);
    pane.markdown.cursor_line = pane.cell_ranges[1].line_start;

    let cleared = pane.clear_current_output().unwrap();

    assert_eq!(cleared, 1);
    assert_eq!(pane.document.cells[0].outputs.len(), 1);
    assert_eq!(pane.document.cells[0].execution_count, Some(1));
    assert!(pane.document.cells[1].outputs.is_empty());
    assert_eq!(pane.document.cells[1].execution_count, None);
    assert_eq!(pane.completed_elapsed_ms.get(&0), Some(&10));
    assert!(!pane.completed_elapsed_ms.contains_key(&1));
}

#[test]
fn insert_cell_below_adds_stable_cell_and_focuses_body() {
    let mut doc = NotebookDocument::default();
    doc.metadata = serde_json::json!({"language_info":{"name":"python"}});
    let mut cell = new_notebook_cell(NotebookCellType::Markdown);
    cell.source = NotebookSource::Text("notes".to_string());
    doc.cells.push(cell);
    let mut pane = NotebookPane::from_document(
        PathBuf::from("test.ipynb"),
        doc,
        String::new(),
        None,
    );

    let inserted = pane.insert_cell_below(NotebookCellType::Code).unwrap();

    assert_eq!(inserted, 1);
    assert_eq!(pane.document.cells.len(), 2);
    assert_eq!(pane.document.cells[1].cell_type, NotebookCellType::Code);
    assert!(notebook_cell_id(&pane.document.cells[1]).is_some());
    assert_eq!(pane.current_cell_index(), Some(1));
    assert_eq!(pane.markdown.lines[pane.markdown.cursor_line], "");
    assert_eq!(
        pane.markdown.mode,
        crate::editor::markdown::MarkdownMode::Insert
    );
}

#[test]
fn insert_cell_above_shifts_completed_elapsed() {
    let mut doc = NotebookDocument::default();
    let mut first = new_notebook_cell(NotebookCellType::Code);
    first.source = NotebookSource::Text("print('a')".to_string());
    let mut second = new_notebook_cell(NotebookCellType::Code);
    second.source = NotebookSource::Text("print('b')".to_string());
    doc.cells.push(first);
    doc.cells.push(second);
    let mut pane = NotebookPane::from_document(
        PathBuf::from("test.ipynb"),
        doc,
        String::new(),
        None,
    );
    pane.completed_elapsed_ms.insert(0, 10);
    pane.completed_elapsed_ms.insert(1, 20);
    pane.markdown.cursor_line = pane.cell_ranges[1].line_start;

    let inserted = pane.insert_cell_above(NotebookCellType::Markdown).unwrap();

    assert_eq!(inserted, 1);
    assert_eq!(pane.document.cells[1].cell_type, NotebookCellType::Markdown);
    assert_eq!(pane.completed_elapsed_ms.get(&0), Some(&10));
    assert_eq!(pane.completed_elapsed_ms.get(&2), Some(&20));
    assert_eq!(pane.current_cell_index(), Some(1));
}

#[test]
fn delete_current_cell_removes_elapsed_and_shifts_following_cells() {
    let mut doc = NotebookDocument::default();
    for label in ["a", "b", "c"] {
        let mut cell = new_notebook_cell(NotebookCellType::Code);
        cell.source = NotebookSource::Text(format!("print('{label}')"));
        doc.cells.push(cell);
    }
    let mut pane = NotebookPane::from_document(
        PathBuf::from("test.ipynb"),
        doc,
        String::new(),
        None,
    );
    pane.completed_elapsed_ms.insert(0, 10);
    pane.completed_elapsed_ms.insert(1, 20);
    pane.completed_elapsed_ms.insert(2, 30);
    pane.markdown.cursor_line = pane.cell_ranges[1].line_start;

    let deleted = pane.delete_current_cell().unwrap();

    assert_eq!(deleted, 1);
    assert_eq!(pane.document.cells.len(), 2);
    assert_eq!(pane.document.cells[0].source.as_str(), "print('a')");
    assert_eq!(pane.document.cells[1].source.as_str(), "print('c')");
    assert_eq!(pane.completed_elapsed_ms.get(&0), Some(&10));
    assert_eq!(pane.completed_elapsed_ms.get(&1), Some(&30));
    assert!(!pane
        .markdown
        .lines
        .iter()
        .any(|line| line.contains("print('b')")));
}

#[test]
fn move_current_cell_down_carries_outputs_and_elapsed() {
    let mut doc = NotebookDocument::default();
    let mut first = new_notebook_cell(NotebookCellType::Code);
    first.source = NotebookSource::Text("print('first')".to_string());
    first.outputs = vec![serde_json::json!({
        "output_type":"stream",
        "name":"stdout",
        "text":"first\n"
    })];
    let mut second = new_notebook_cell(NotebookCellType::Code);
    second.source = NotebookSource::Text("print('second')".to_string());
    doc.cells.push(first);
    doc.cells.push(second);
    let mut pane = NotebookPane::from_document(
        PathBuf::from("test.ipynb"),
        doc,
        String::new(),
        None,
    );
    pane.completed_elapsed_ms.insert(0, 10);
    pane.completed_elapsed_ms.insert(1, 20);
    pane.markdown.cursor_line = pane.cell_ranges[0].line_start;
    let first_id = notebook_cell_id(&pane.document.cells[0])
        .unwrap()
        .to_string();

    let moved_to = pane.move_current_cell_down().unwrap();

    assert_eq!(moved_to, 1);
    assert_eq!(pane.document.cells[0].source.as_str(), "print('second')");
    assert_eq!(pane.document.cells[1].source.as_str(), "print('first')");
    assert_eq!(
        notebook_cell_id(&pane.document.cells[1]),
        Some(first_id.as_str())
    );
    assert_eq!(
        output_text(&pane.document.cells[1].outputs[0]).unwrap(),
        "first\n"
    );
    assert_eq!(pane.completed_elapsed_ms.get(&0), Some(&20));
    assert_eq!(pane.completed_elapsed_ms.get(&1), Some(&10));
    assert_eq!(pane.current_cell_index(), Some(1));
}

#[test]
fn structure_edits_are_blocked_while_cells_are_running() {
    let mut doc = NotebookDocument::default();
    doc.cells.push(new_notebook_cell(NotebookCellType::Code));
    let mut pane = NotebookPane::from_document(
        PathBuf::from("test.ipynb"),
        doc,
        String::new(),
        None,
    );
    pane.running_cells.insert(0);

    assert!(pane.insert_cell_below(NotebookCellType::Code).is_err());
    assert!(pane.delete_current_cell().is_err());
    assert!(pane.move_current_cell_down().is_err());
}

#[test]
fn syncs_edited_rendered_code_back_to_cell_source() {
    let mut doc = NotebookDocument::default();
    doc.metadata = serde_json::json!({"language_info":{"name":"python"}});
    doc.cells.push(NotebookCell {
        cell_type: NotebookCellType::Code,
        metadata: Value::Object(serde_json::Map::new()),
        source: NotebookSource::Text("print('old')\n".to_string()),
        execution_count: None,
        outputs: Vec::new(),
        extra: serde_json::Map::new(),
    });
    let mut pane = NotebookPane::from_document(
        PathBuf::from("test.ipynb"),
        doc,
        String::new(),
        None,
    );
    let code_line = pane
        .markdown
        .lines
        .iter()
        .position(|line| line.contains("print('old')"))
        .unwrap();
    pane.markdown.lines[code_line] = "print('new')".to_string();
    pane.sync_from_rendered_markdown();
    assert_eq!(pane.document.cells[0].source.as_str(), "print('new')");
}

#[test]
fn sync_preserves_markdown_ranges_around_code_cells() {
    let mut doc = NotebookDocument::default();
    doc.metadata = serde_json::json!({"language_info":{"name":"python"}});
    doc.cells.push(NotebookCell {
        cell_type: NotebookCellType::Markdown,
        metadata: Value::Object(serde_json::Map::new()),
        source: NotebookSource::Text("# Title\n\nintro".to_string()),
        execution_count: None,
        outputs: Vec::new(),
        extra: serde_json::Map::new(),
    });
    doc.cells.push(NotebookCell {
        cell_type: NotebookCellType::Code,
        metadata: Value::Object(serde_json::Map::new()),
        source: NotebookSource::Text("print('hi')\n".to_string()),
        execution_count: Some(4),
        outputs: Vec::new(),
        extra: serde_json::Map::new(),
    });
    doc.cells.push(NotebookCell {
        cell_type: NotebookCellType::Markdown,
        metadata: Value::Object(serde_json::Map::new()),
        source: NotebookSource::Text("after".to_string()),
        execution_count: None,
        outputs: Vec::new(),
        extra: serde_json::Map::new(),
    });

    let mut pane = NotebookPane::from_document(
        PathBuf::from("test.ipynb"),
        doc,
        String::new(),
        None,
    );
    pane.sync_from_rendered_markdown();

    assert_eq!(pane.document.cells[0].source.as_str(), "# Title\n\nintro");
    assert_eq!(pane.document.cells[1].source.as_str(), "print('hi')");
    assert_eq!(pane.document.cells[2].source.as_str(), "after");
}

#[test]
fn renders_notebook_style_outputs_and_running_state() {
    let mut doc = NotebookDocument::default();
    doc.metadata = serde_json::json!({"language_info":{"name":"python"}});
    doc.cells.push(NotebookCell {
            cell_type: NotebookCellType::Code,
            metadata: Value::Object(serde_json::Map::new()),
            source: NotebookSource::Text("print('hi')\n".to_string()),
            execution_count: Some(7),
            outputs: vec![
                serde_json::json!({"output_type":"stream","name":"stdout","text":"hi\n"}),
                serde_json::json!({"output_type":"stream","name":"stderr","text":"warn\n"}),
                serde_json::json!({"output_type":"display_data","data":{"application/json":{"ok":true}}}),
            ],
            extra: serde_json::Map::new(),
        });
    let rendered = doc.render_markdown_with_status(
        &BTreeSet::new(),
        &BTreeMap::new(),
        &BTreeMap::from([(0, 1_250)]),
    );
    assert!(rendered
        .markdown
        .contains("%%neoism_notebook_output _ 1.2_s hi"));
    assert!(rendered
        .markdown
        .contains("%%neoism_notebook_output Err_[7] _ warn"));
    assert!(rendered
        .markdown
        .contains("%%neoism_notebook_output Out_[7] _"));
    assert!(!rendered.markdown.contains("```text neoism_notebook_output"));
    assert!(rendered.markdown.contains("\"ok\": true"));

    let running = doc.render_markdown_with_status(
        &BTreeSet::from([0]),
        &BTreeMap::new(),
        &BTreeMap::new(),
    );
    assert!(running
        .markdown
        .contains("neoism_state=running neoism_count=*"));
    assert!(running
        .markdown
        .contains("%%neoism_notebook_output _ _ neoism_state=running hi"));
}

#[test]
fn rich_display_outputs_prefer_images_and_preserve_mime_context() {
    let output = serde_json::json!({
        "output_type": "display_data",
        "data": {
            "text/plain": "<Figure size 1x1>",
            "image/png": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg=="
        }
    });

    let text = output_text(&output).unwrap();

    assert!(text.contains("Image output: image/png"));
    assert!(text.contains("1x1"));
    assert!(!text.contains("<Figure size 1x1>"));
}

#[test]
fn rendered_image_outputs_decode_bitmap_outputs() {
    let mut doc = NotebookDocument::default();
    doc.cells.push(NotebookCell {
            cell_type: NotebookCellType::Code,
            metadata: Value::Object(serde_json::Map::new()),
            source: NotebookSource::Text("display(fig)\n".to_string()),
            execution_count: Some(1),
            outputs: vec![serde_json::json!({
                "output_type": "display_data",
                "data": {
                    "text/plain": "<Figure size 1x1>",
                    "image/png": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg=="
                }
            })],
            extra: serde_json::Map::new(),
        });
    let pane = NotebookPane::from_document(
        PathBuf::from("/tmp/render.ipynb"),
        doc,
        String::new(),
        None,
    );

    let images = pane.rendered_image_outputs();

    assert_eq!(images.len(), 1);
    let image = &images[0];
    assert_eq!(image.cell_index, 0);
    assert_eq!(image.output_index, 0);
    assert_eq!(image.line, 3);
    assert_eq!(image.mime, "image/png");
    assert_eq!((image.width, image.height), (1, 1));
    assert_eq!(image.pixels.len(), 4);
    assert_eq!(image.image_id & 0xff00_0000, NOTEBOOK_IMAGE_ID_NAMESPACE);
    assert!(pane.markdown.lines[image.line].contains("Image output: image/png"));
}

#[test]
fn rendered_image_outputs_track_marker_lines_after_text_outputs() {
    let mut doc = NotebookDocument::default();
    doc.cells.push(NotebookCell {
            cell_type: NotebookCellType::Code,
            metadata: Value::Object(serde_json::Map::new()),
            source: NotebookSource::Text("display(fig)\n".to_string()),
            execution_count: Some(1),
            outputs: vec![
                serde_json::json!({
                    "output_type": "stream",
                    "name": "stdout",
                    "text": "alpha\nbeta\n"
                }),
                serde_json::json!({
                    "output_type": "display_data",
                    "data": {
                        "image/png": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg=="
                    }
                }),
            ],
            extra: serde_json::Map::new(),
        });
    let pane = NotebookPane::from_document(
        PathBuf::from("/tmp/render.ipynb"),
        doc,
        String::new(),
        None,
    );

    let images = pane.rendered_image_outputs();

    assert_eq!(images.len(), 1);
    assert_eq!(images[0].line, 5);
    assert!(pane.markdown.lines[3].contains("alpha"));
    assert!(pane.markdown.lines[4].contains("beta"));
    assert!(pane.markdown.lines[5].contains("Image output: image/png"));
}

#[test]
fn rendered_image_outputs_include_markdown_cell_attachments() {
    let mut extra = serde_json::Map::new();
    extra.insert(
            "attachments".to_string(),
            serde_json::json!({
                "plot.png": {
                    "image/png": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg=="
                }
            }),
        );
    let mut doc = NotebookDocument::default();
    doc.cells.push(NotebookCell {
        cell_type: NotebookCellType::Markdown,
        metadata: Value::Object(serde_json::Map::new()),
        source: NotebookSource::Text("![plot](attachment:plot.png)\n".to_string()),
        execution_count: None,
        outputs: Vec::new(),
        extra,
    });
    let pane = NotebookPane::from_document(
        PathBuf::from("/tmp/attachments.ipynb"),
        doc,
        String::new(),
        None,
    );

    let images = pane.rendered_image_outputs();

    assert_eq!(images.len(), 1);
    let image = &images[0];
    assert_eq!(image.cell_index, 0);
    assert_eq!(image.output_index, 0);
    assert_eq!(image.attachment_name.as_deref(), Some("plot.png"));
    assert_eq!(image.line, 0);
    assert_eq!(image.mime, "image/png");
    assert_eq!((image.width, image.height), (1, 1));
    assert_eq!(image.pixels.len(), 4);
    assert_eq!(image.image_id & 0xff00_0000, NOTEBOOK_IMAGE_ID_NAMESPACE);
}

#[test]
fn markdown_attachment_preview_dimensions_refresh_after_source_change() {
    let mut extra = serde_json::Map::new();
    extra.insert(
            "attachments".to_string(),
            serde_json::json!({
                "plot.png": {
                    "image/png": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg=="
                }
            }),
        );
    let mut doc = NotebookDocument::default();
    doc.cells.push(NotebookCell {
        cell_type: NotebookCellType::Markdown,
        metadata: Value::Object(serde_json::Map::new()),
        source: NotebookSource::Text("![plot](attachment:plot.png)\n".to_string()),
        execution_count: None,
        outputs: Vec::new(),
        extra,
    });
    let mut pane = NotebookPane::from_document(
        PathBuf::from("/tmp/attachments.ipynb"),
        doc,
        String::new(),
        None,
    );

    assert_eq!(
        pane.markdown.notebook_image_preview_dimensions_for_line(0),
        Some((1, 1))
    );

    assert!(pane.set_cell_source(0, "no image here\n".to_string()));
    assert_eq!(
        pane.markdown.notebook_image_preview_dimensions_for_line(0),
        None
    );
}

#[test]
fn rendered_image_outputs_decode_percent_encoded_attachment_names() {
    let mut extra = serde_json::Map::new();
    extra.insert(
            "attachments".to_string(),
            serde_json::json!({
                "plot image.png": {
                    "image/png": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg=="
                }
            }),
        );
    let mut doc = NotebookDocument::default();
    doc.cells.push(NotebookCell {
        cell_type: NotebookCellType::Markdown,
        metadata: Value::Object(serde_json::Map::new()),
        source: NotebookSource::Text(
            "<img src=\"attachment:plot%20image.png\">\n".to_string(),
        ),
        execution_count: None,
        outputs: Vec::new(),
        extra,
    });
    let pane = NotebookPane::from_document(
        PathBuf::from("/tmp/attachments.ipynb"),
        doc,
        String::new(),
        None,
    );

    let images = pane.rendered_image_outputs();

    assert_eq!(images.len(), 1);
    assert_eq!(images[0].attachment_name.as_deref(), Some("plot image.png"));
    assert_eq!((images[0].width, images[0].height), (1, 1));
}

#[test]
fn display_update_replaces_matching_output_from_previous_cell() {
    let mut doc = NotebookDocument::default();
    doc.metadata = serde_json::json!({"language_info":{"name":"python"}});
    doc.cells.push(NotebookCell {
        cell_type: NotebookCellType::Code,
        metadata: Value::Object(serde_json::Map::new()),
        source: NotebookSource::Text("display(handle)\n".to_string()),
        execution_count: Some(1),
        outputs: vec![serde_json::json!({
            "output_type": "display_data",
            "data": {"text/plain": "old"},
            "metadata": {},
            "transient": {"display_id": "plot-1"}
        })],
        extra: serde_json::Map::new(),
    });
    doc.cells.push(NotebookCell {
        cell_type: NotebookCellType::Code,
        metadata: Value::Object(serde_json::Map::new()),
        source: NotebookSource::Text("handle.update('new')\n".to_string()),
        execution_count: None,
        outputs: Vec::new(),
        extra: serde_json::Map::new(),
    });
    let mut pane = NotebookPane::from_document(
        PathBuf::from("display.ipynb"),
        doc,
        String::new(),
        None,
    );
    let job = pane.prepare_cell_execution(1).unwrap();

    let replaced = pane
        .apply_display_update(NotebookDisplayUpdate {
            cell_index: job.cell_index,
            cell_id: job.cell_id.clone(),
            run_id: job.run_id,
            display_id: "plot-1".to_string(),
            output: serde_json::json!({
                "output_type": "display_data",
                "data": {"text/plain": "new"},
                "metadata": {},
                "transient": {"display_id": "plot-1"}
            }),
        })
        .unwrap();

    assert_eq!(replaced, 1);
    assert_eq!(
        pane.document.cells[0].outputs[0]["data"]["text/plain"],
        "new"
    );
    assert!(pane.markdown.lines.iter().any(|line| line.contains("new")));
    assert!(!pane.markdown.lines.iter().any(|line| line.contains("old")));
}

#[test]
fn display_update_ignores_stale_run_ids() {
    let mut doc = NotebookDocument::default();
    doc.cells.push(NotebookCell {
        cell_type: NotebookCellType::Code,
        metadata: Value::Object(serde_json::Map::new()),
        source: NotebookSource::Text("display(handle)\n".to_string()),
        execution_count: Some(1),
        outputs: vec![serde_json::json!({
            "output_type": "display_data",
            "data": {"text/plain": "old"},
            "metadata": {},
            "transient": {"display_id": "plot-1"}
        })],
        extra: serde_json::Map::new(),
    });
    doc.cells.push(new_notebook_cell(NotebookCellType::Code));
    let mut pane = NotebookPane::from_document(
        PathBuf::from("display.ipynb"),
        doc,
        String::new(),
        None,
    );
    let job = pane.prepare_cell_execution(1).unwrap();

    let replaced = pane
        .apply_display_update(NotebookDisplayUpdate {
            cell_index: job.cell_index,
            cell_id: job.cell_id,
            run_id: job.run_id.saturating_add(1),
            display_id: "plot-1".to_string(),
            output: serde_json::json!({
                "output_type": "display_data",
                "data": {"text/plain": "new"},
                "metadata": {},
                "transient": {"display_id": "plot-1"}
            }),
        })
        .unwrap();

    assert_eq!(replaced, 0);
    assert_eq!(
        pane.document.cells[0].outputs[0]["data"]["text/plain"],
        "old"
    );
}

#[test]
fn rich_display_outputs_format_json_and_html() {
    let json_output = serde_json::json!({
        "output_type": "execute_result",
        "data": {"application/json": {"ok": true, "items": [1, 2]}}
    });
    let html_output = serde_json::json!({
        "output_type": "display_data",
        "data": {"text/html": "<div><b>Hello</b>&nbsp;<span>Neoism</span></div>"}
    });

    let json_text = output_text(&json_output).unwrap();
    let html_text = output_text(&html_output).unwrap();

    assert!(json_text.starts_with("JSON output:"));
    assert!(json_text.contains("\"ok\": true"));
    assert_eq!(html_text, "HTML output:\nHello Neoism");
}

#[test]
fn html_table_outputs_render_as_readable_pipe_tables() {
    let output = serde_json::json!({
        "output_type": "display_data",
        "data": {
            "text/html": "<table><thead><tr><th>Name</th><th>Value</th></tr></thead><tbody><tr><td>A&amp;B</td><td>1|2</td></tr><tr><td>C</td><td>3</td></tr></tbody></table>"
        }
    });

    let text = output_text(&output).unwrap();

    assert!(text.starts_with("HTML table output:\n"));
    assert!(text.contains("| Name | Value |"));
    assert!(text.contains("| --- | --- |"));
    assert!(text.contains("| A&B | 1\\|2 |"));
    assert!(text.contains("| C | 3 |"));
    assert!(!text.contains("NameValueA&B"));
}

#[test]
fn latex_outputs_are_preferred_over_plain_text_fallbacks() {
    let output = serde_json::json!({
        "output_type": "display_data",
        "data": {
            "text/plain": "Integral(x**2, x)",
            "text/latex": "$\\int x^{2}\\, dx$"
        }
    });

    let text = output_text(&output).unwrap();

    assert_eq!(text, "LaTeX output:\n$\\int x^{2}\\, dx$");
    assert!(!text.contains("Integral(x**2"));
    assert!(!text.contains("text/latex output"));
}

#[test]
fn text_outputs_strip_ansi_sequences_for_rendering() {
    let output = serde_json::json!({
        "output_type": "stream",
        "name": "stdout",
        "text": "\u{1b}[31mred\u{1b}[0m plain \u{1b}]8;;https://example.test\u{7}link\u{1b}]8;;\u{7}\n"
    });

    let text = output_text(&output).unwrap();

    assert_eq!(text, "red plain link\n");
}

#[test]
fn text_outputs_collapse_carriage_return_progress_updates() {
    let output = serde_json::json!({
        "output_type": "stream",
        "name": "stdout",
        "text": "progress 10%\rprogress 50%\rprogress 100%\nnext\r\n"
    });

    let text = output_text(&output).unwrap();

    assert_eq!(text, "progress 100%\nnext\n");
}

#[test]
fn large_text_outputs_are_truncated_for_rendering_but_preserved() {
    let large = (0..3_000)
        .map(|index| format!("row {index}\n"))
        .collect::<String>();
    let mut doc = NotebookDocument::default();
    doc.cells.push(NotebookCell {
        cell_type: NotebookCellType::Code,
        metadata: Value::Object(serde_json::Map::new()),
        source: NotebookSource::Text("for i in range(3000): print(i)\n".to_string()),
        execution_count: Some(1),
        outputs: vec![serde_json::json!({
            "output_type": "stream",
            "name": "stdout",
            "text": large,
        })],
        extra: serde_json::Map::new(),
    });

    let rendered = doc.render_markdown();

    assert!(rendered.markdown.contains("row 0"));
    assert!(rendered.markdown.contains("row 1999"));
    assert!(!rendered.markdown.contains("row 2999"));
    assert!(rendered
        .markdown
        .contains("Neoism output truncated for display"));
    assert!(rendered
        .markdown
        .contains("Full output is preserved in notebook data"));
    assert_eq!(
        value_text(doc.cells[0].outputs[0].get("text")).unwrap(),
        (0..3_000)
            .map(|index| format!("row {index}\n"))
            .collect::<String>()
    );
}

#[test]
fn error_tracebacks_strip_ansi_sequences_for_rendering() {
    let output = serde_json::json!({
        "output_type": "error",
        "ename": "\u{1b}[31mValueError\u{1b}[0m",
        "evalue": "bad",
        "traceback": ["Traceback\n", "\u{1b}[31mValueError\u{1b}[0m: bad"]
    });

    let text = output_text(&output).unwrap();

    assert_eq!(text, "Traceback\nValueError: bad");
}

#[test]
fn appends_streaming_output_chunks() {
    let mut doc = NotebookDocument::default();
    doc.cells.push(NotebookCell {
        cell_type: NotebookCellType::Code,
        metadata: Value::Object(serde_json::Map::new()),
        source: NotebookSource::Text("print('hi')\n".to_string()),
        execution_count: None,
        outputs: Vec::new(),
        extra: serde_json::Map::new(),
    });
    let mut pane = NotebookPane::from_document(
        PathBuf::from("test.ipynb"),
        doc,
        String::new(),
        None,
    );
    let cell_id = notebook_cell_id(&pane.document.cells[0])
        .unwrap()
        .to_string();
    pane.running_cells.insert(0);
    pane.running_cell_runs.insert(0, 1);
    pane.apply_execution_chunk(NotebookExecutionChunk {
        cell_index: 0,
        cell_id: cell_id.clone(),
        run_id: 1,
        stream: NotebookOutputStream::Stdout,
        text: "hel".to_string(),
    })
    .unwrap();
    pane.apply_execution_chunk(NotebookExecutionChunk {
        cell_index: 0,
        cell_id,
        run_id: 1,
        stream: NotebookOutputStream::Stdout,
        text: "lo\n".to_string(),
    })
    .unwrap();
    let text = output_text(&pane.document.cells[0].outputs[0]).unwrap();
    assert_eq!(text, "hello\n");
    assert!(pane
        .markdown
        .lines
        .iter()
        .any(|line| line == "%%neoism_notebook_output _ _ neoism_state=running hello"));
}

#[test]
fn streaming_output_follows_cell_id_after_reorder() {
    let mut doc = NotebookDocument::default();
    doc.metadata = serde_json::json!({"language_info":{"name":"python"}});
    doc.cells.push(NotebookCell {
        cell_type: NotebookCellType::Code,
        metadata: Value::Object(serde_json::Map::new()),
        source: NotebookSource::Text("print('first')\n".to_string()),
        execution_count: None,
        outputs: Vec::new(),
        extra: serde_json::Map::new(),
    });
    doc.cells.push(NotebookCell {
        cell_type: NotebookCellType::Code,
        metadata: Value::Object(serde_json::Map::new()),
        source: NotebookSource::Text("print('second')\n".to_string()),
        execution_count: None,
        outputs: Vec::new(),
        extra: serde_json::Map::new(),
    });
    let mut pane = NotebookPane::from_document(
        PathBuf::from("test.ipynb"),
        doc,
        String::new(),
        None,
    );
    let job = pane.prepare_cell_execution(0).unwrap();
    let first_range = pane.cell_ranges[0].line_start..pane.cell_ranges[0].line_end + 1;
    let moved = pane.markdown.lines.drain(first_range).collect::<Vec<_>>();
    pane.markdown.lines.extend(moved);

    pane.sync_order_from_rendered_markdown();

    assert_eq!(pane.running_cell_runs.get(&1), Some(&job.run_id));
    pane.apply_execution_chunk(NotebookExecutionChunk {
        cell_index: job.cell_index,
        cell_id: job.cell_id.clone(),
        run_id: job.run_id,
        stream: NotebookOutputStream::Stdout,
        text: "moved\n".to_string(),
    })
    .unwrap();

    assert!(pane.document.cells[0].outputs.is_empty());
    assert_eq!(
        notebook_cell_id(&pane.document.cells[1]),
        Some(job.cell_id.as_str())
    );
    assert_eq!(
        output_text(&pane.document.cells[1].outputs[0]).unwrap(),
        "moved\n"
    );
}

#[test]
fn ignores_stale_streaming_output_chunks() {
    let mut doc = NotebookDocument::default();
    doc.cells.push(NotebookCell {
        cell_type: NotebookCellType::Code,
        metadata: Value::Object(serde_json::Map::new()),
        source: NotebookSource::Text("print('new')\n".to_string()),
        execution_count: None,
        outputs: Vec::new(),
        extra: serde_json::Map::new(),
    });
    let mut pane = NotebookPane::from_document(
        PathBuf::from("test.ipynb"),
        doc,
        String::new(),
        None,
    );
    let job = pane.prepare_cell_execution(0).unwrap();

    pane.apply_execution_chunk(NotebookExecutionChunk {
        cell_index: 0,
        cell_id: job.cell_id.clone(),
        run_id: job.run_id.saturating_sub(1),
        stream: NotebookOutputStream::Stdout,
        text: "old\n".to_string(),
    })
    .unwrap();
    assert!(pane.document.cells[0].outputs.is_empty());

    pane.apply_execution_chunk(NotebookExecutionChunk {
        cell_index: 0,
        cell_id: job.cell_id.clone(),
        run_id: job.run_id,
        stream: NotebookOutputStream::Stdout,
        text: "new\n".to_string(),
    })
    .unwrap();
    let text = output_text(&pane.document.cells[0].outputs[0]).unwrap();
    assert_eq!(text, "new\n");
}

#[test]
fn output_before_next_cell_has_no_blank_separator_stop() {
    let mut doc = NotebookDocument::default();
    doc.metadata = serde_json::json!({"language_info":{"name":"python"}});
    doc.cells.push(NotebookCell {
        cell_type: NotebookCellType::Code,
        metadata: Value::Object(serde_json::Map::new()),
        source: NotebookSource::Text("print('hi')\n".to_string()),
        execution_count: Some(1),
        outputs: vec![serde_json::json!({
            "output_type":"stream",
            "name":"stdout",
            "text":"hi\n"
        })],
        extra: serde_json::Map::new(),
    });
    doc.cells.push(NotebookCell {
        cell_type: NotebookCellType::Markdown,
        metadata: Value::Object(serde_json::Map::new()),
        source: NotebookSource::Text("next".to_string()),
        execution_count: None,
        outputs: Vec::new(),
        extra: serde_json::Map::new(),
    });
    let rendered = doc.render_markdown();
    let lines = rendered.markdown.lines().collect::<Vec<_>>();
    let output_ix = lines
        .iter()
        .position(|line| *line == "%%neoism_notebook_output _ _ hi")
        .unwrap();
    assert_eq!(lines.get(output_ix + 1), Some(&"next"));
    assert!(!lines.iter().any(|line| *line == "---"));
}

#[test]
fn sync_preserves_trailing_blank_lines_in_code_cell() {
    let mut doc = NotebookDocument::default();
    doc.metadata = serde_json::json!({"language_info":{"name":"python"}});
    doc.cells.push(NotebookCell {
        cell_type: NotebookCellType::Code,
        metadata: Value::Object(serde_json::Map::new()),
        source: NotebookSource::Text("print('hi')\n".to_string()),
        execution_count: None,
        outputs: Vec::new(),
        extra: serde_json::Map::new(),
    });
    let mut pane = NotebookPane::from_document(
        PathBuf::from("test.ipynb"),
        doc,
        String::new(),
        None,
    );
    let code_line = pane
        .markdown
        .lines
        .iter()
        .position(|line| line == "print('hi')")
        .unwrap();
    pane.markdown.lines.insert(code_line + 1, String::new());
    pane.markdown.lines.insert(code_line + 2, String::new());

    pane.sync_from_rendered_markdown();

    assert_eq!(pane.document.cells[0].source.as_str(), "print('hi')\n\n\n");
}

#[test]
fn sync_order_from_rendered_markdown_reorders_cells_with_outputs() {
    let mut doc = NotebookDocument::default();
    doc.metadata = serde_json::json!({"language_info":{"name":"python"}});
    doc.cells.push(NotebookCell {
        cell_type: NotebookCellType::Code,
        metadata: Value::Object(serde_json::Map::new()),
        source: NotebookSource::Text("print('first')\n".to_string()),
        execution_count: Some(1),
        outputs: vec![serde_json::json!({
            "output_type":"stream",
            "name":"stdout",
            "text":"first\n"
        })],
        extra: serde_json::Map::new(),
    });
    doc.cells.push(NotebookCell {
        cell_type: NotebookCellType::Code,
        metadata: Value::Object(serde_json::Map::new()),
        source: NotebookSource::Text("print('second')\n".to_string()),
        execution_count: None,
        outputs: Vec::new(),
        extra: serde_json::Map::new(),
    });
    let mut pane = NotebookPane::from_document(
        PathBuf::from("test.ipynb"),
        doc,
        String::new(),
        None,
    );
    let first_range = pane.cell_ranges[0].line_start..pane.cell_ranges[0].line_end + 1;
    let moved = pane.markdown.lines.drain(first_range).collect::<Vec<_>>();
    pane.markdown.lines.extend(moved);

    pane.sync_order_from_rendered_markdown();

    assert_eq!(pane.document.cells[0].source.as_str(), "print('second')");
    assert_eq!(pane.document.cells[1].source.as_str(), "print('first')");
    assert_eq!(
        output_text(&pane.document.cells[1].outputs[0]).unwrap(),
        "first\n"
    );
}

#[test]
fn deleting_rendered_code_cell_drops_its_output_markers() {
    let mut doc = NotebookDocument::default();
    doc.metadata = serde_json::json!({"language_info":{"name":"python"}});
    doc.cells.push(NotebookCell {
        cell_type: NotebookCellType::Markdown,
        metadata: Value::Object(serde_json::Map::new()),
        source: NotebookSource::Text("before".to_string()),
        execution_count: None,
        outputs: Vec::new(),
        extra: serde_json::Map::new(),
    });
    doc.cells.push(NotebookCell {
        cell_type: NotebookCellType::Code,
        metadata: Value::Object(serde_json::Map::new()),
        source: NotebookSource::Text("print('boom')\n".to_string()),
        execution_count: Some(1),
        outputs: vec![serde_json::json!({
            "output_type":"stream",
            "name":"stderr",
            "text":"boom\n"
        })],
        extra: serde_json::Map::new(),
    });
    doc.cells.push(NotebookCell {
        cell_type: NotebookCellType::Markdown,
        metadata: Value::Object(serde_json::Map::new()),
        source: NotebookSource::Text("after".to_string()),
        execution_count: None,
        outputs: Vec::new(),
        extra: serde_json::Map::new(),
    });
    let mut pane = NotebookPane::from_document(
        PathBuf::from("test.ipynb"),
        doc,
        String::new(),
        None,
    );

    let code_start = pane
        .markdown
        .lines
        .iter()
        .position(|line| line.contains("neoism_notebook_cell=1"))
        .unwrap();
    let code_end = pane.markdown.lines[code_start..]
        .iter()
        .position(|line| line.trim() == "```")
        .map(|offset| code_start + offset)
        .unwrap();
    pane.markdown.lines.drain(code_start..=code_end);

    pane.sync_from_rendered_markdown();

    assert_eq!(pane.document.cells.len(), 2);
    assert!(pane
        .document
        .cells
        .iter()
        .all(|cell| cell.cell_type != NotebookCellType::Code));
    assert!(pane
        .document
        .cells
        .iter()
        .all(|cell| !cell.source.as_str().contains("neoism_notebook_output")));
    assert!(pane
        .markdown
        .lines
        .iter()
        .all(|line| !line.contains("neoism_notebook_output")));
}

#[test]
fn runs_python_cell_and_persists_stdout() {
    if std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("skipping notebook execution test: python3 unavailable");
        return;
    }
    let mut doc = NotebookDocument::default();
    doc.cells.push(NotebookCell {
        cell_type: NotebookCellType::Code,
        metadata: Value::Object(serde_json::Map::new()),
        source: NotebookSource::Text("print(2 + 3)\n".to_string()),
        execution_count: None,
        outputs: Vec::new(),
        extra: serde_json::Map::new(),
    });
    let mut pane = NotebookPane::from_document(
        PathBuf::from("test.ipynb"),
        doc,
        String::new(),
        None,
    );
    pane.run_cell(0).unwrap();
    let text = output_text(&pane.document.cells[0].outputs[0]).unwrap();
    assert_eq!(text, "5\n");
    assert_eq!(pane.document.cells[0].execution_count, Some(1));
}

#[test]
fn running_python_cell_can_use_prior_cell_state() {
    if std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("skipping notebook execution test: python3 unavailable");
        return;
    }
    let mut doc = NotebookDocument::default();
    for source in ["x = 40\n", "print(x + 2)\n"] {
        doc.cells.push(NotebookCell {
            cell_type: NotebookCellType::Code,
            metadata: Value::Object(serde_json::Map::new()),
            source: NotebookSource::Text(source.to_string()),
            execution_count: None,
            outputs: Vec::new(),
            extra: serde_json::Map::new(),
        });
    }
    let mut pane = NotebookPane::from_document(
        PathBuf::from("test.ipynb"),
        doc,
        String::new(),
        None,
    );
    pane.run_cell(1).unwrap();
    let text = output_text(&pane.document.cells[1].outputs[0]).unwrap();
    assert_eq!(text, "42\n");
}

#[test]
fn runs_shell_cell_and_persists_stdout() {
    let mut doc = NotebookDocument::default();
    doc.metadata = serde_json::json!({"language_info":{"name":"bash"}});
    doc.cells.push(NotebookCell {
        cell_type: NotebookCellType::Code,
        metadata: Value::Object(serde_json::Map::new()),
        source: NotebookSource::Text("name=neoism\necho hello-$name\n".to_string()),
        execution_count: None,
        outputs: Vec::new(),
        extra: serde_json::Map::new(),
    });
    let mut pane = NotebookPane::from_document(
        PathBuf::from("test.ipynb"),
        doc,
        String::new(),
        None,
    );
    pane.run_cell(0).unwrap();
    let text = output_text(&pane.document.cells[0].outputs[0]).unwrap();
    assert_eq!(text, "hello-neoism\n");
}
