//! Read-only session checklist, using the same shell and pixel scroll as `/usage`.
use super::{Kind, Picker};
use crate::{agent::timeline, app::App, tabs::TabKind, ui::theme::rgb};
use neoism_protocol::agent::TodoItem;
use rune::{El, Tx, col, row, text};
use rune_kit::markdown::Markdown;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status { Pending, InProgress, Completed }

impl Status {
    fn of(status: &str) -> Self {
        match status {
            "completed" => Self::Completed,
            "in_progress" => Self::InProgress,
            _ => Self::Pending,
        }
    }
}

pub(super) fn view(app: &App, composer: [f32; 4]) -> El {
    let p = app.pal;
    let s = app.ty.scale();
    let tab = app.active_tab();
    let session = match &tab.kind {
        TabKind::Agent { session: Some(id) } => app.sessions.get(id),
        _ => None,
    };
    let todos = session.map_or(&[][..], |session| session.session_todos());
    let cw = composer[2];
    let completed = todos.iter().filter(|todo| Status::of(&todo.status) == Status::Completed).count();
    let title = if todos.is_empty() { "Tasks".to_owned() } else { format!("Tasks  ·  {completed}/{}", todos.len()) };
    let mut content = col().w((cw - 2.0 * s).max(0.0)).px(14.0 * s).py(12.0 * s).gap(14.0 * s);
    if todos.is_empty() {
        content = content.child(text("No tasks yet").font(app.ty.body()).color(rgb(p.muted)).w_full().wrap());
    } else {
        let look = (app.pal as *const _ as usize, app.ty.size.to_bits(), app.ty.weight.to_bits(), cw.to_bits());
        for (index, todo) in todos.iter().enumerate() {
            let key = format!("task:{}:{index}", tab.id);
            // No parsing on wheel frames. Rebuild only the changed task or a changed look.
            let task = rune::memo(key, (&todo.content, &todo.status, look), || {
                let status = Status::of(&todo.status);
                let (mark, color) = match status {
                    Status::Completed => ("✓", p.green),
                    Status::InProgress => ("◉", p.blue),
                    Status::Pending => ("□", p.muted),
                };
                let markdown = Markdown::new(todo.content.clone())
                    .highlights(crate::agent::prose::highlights(&todo.content, p))
                    .reveal(0.0).code_frame(timeline::code_frame(app)).selectable(true)
                    .on_link_tx(|a: &mut App, url: &str, _: &mut Tx| a.open_link(url))
                    .render(&timeline::markdown_style(app));
                row().w_full().gap(10.0 * s)
                    .child(text(mark).font(app.ty.body()).color(rgb(color)).w(18.0 * s))
                    // The text is a real multiline Markdown column: subsequent lines remain
                    // aligned with the first, never with the checklist marker.
                    .child(col().grow(1.0).min_w(0.0).child(markdown))
            });
            content = content.child(task);
        }
    }
    super::panel::view(app, composer, "tasks", title, "↑↓ scroll · Esc close", content)
}

impl App {
    pub fn open_tasks(&mut self) {
        self.focus_picker(Picker::new(Kind::Tasks, "Tasks", Vec::new()));
    }

    /// Parent-owned headless scenes can use `empty`, `multiline`, or `long`.
    pub(crate) fn fixture_tasks(&mut self, state: &str) {
        let id = "rune-tasks-fixture".to_owned();
        self.open_tab(TabKind::Agent { session: Some(id.clone()) });
        let todos = if state == "empty" { Vec::new() } else {
            let mut todos = vec![
                TodoItem { status: "completed".into(), content: "Inspect the **session checklist**".into() },
                TodoItem { status: "in_progress".into(), content: "Render `Markdown` tasks\n\nWith a second paragraph and a list:\n\n- Preserve wrapping\n- Keep the marker aligned\n\n```rust\nlet ready = true;\n```".into() },
                TodoItem { status: "pending".into(), content: "Verify narrow layouts and keyboard scrolling".into() },
            ];
            if state == "long" {
                todos.extend((0..30).map(|n| TodoItem { status: "pending".into(), content: format!("Task {}: verify the bounded viewport with a longer checklist entry that wraps at narrow widths.", n + 4) }));
            }
            todos
        };
        self.sessions.entry(id).or_default().todos = Some(todos);
        self.open_tasks();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history(id: &str, tool: &str, todos: Vec<TodoItem>) -> neoism_protocol::agent::HistoryMessage {
        serde_json::from_value(serde_json::json!({
            "id": id, "role": "Assistant", "kind": "Tool", "tool": tool, "todos": todos
        })).expect("history fixture")
    }

    fn task() -> TodoItem { TodoItem { status: "pending".into(), content: "Persisted task".into() } }

    #[test]
    fn live_snapshot_wins_even_when_empty() {
        let mut session = crate::agent::Session::default();
        session.messages.push(history("older", "todowrite", vec![task()]));
        assert_eq!(session.session_todos(), &[task()]);
        session.todos = Some(Vec::new());
        assert!(session.session_todos().is_empty());
        session.todos = Some(vec![TodoItem { status: "completed".into(), content: "Live task".into() }]);
        assert_eq!(session.session_todos()[0].content, "Live task");
    }

    #[test]
    fn newest_persisted_checklist_or_clear_wins() {
        for (id, tool) in [("todos-snapshot", ""), ("clear", "ToDoWrite"), ("clear", "todo_write"), ("clear", "todo")] {
            let mut session = crate::agent::Session::default();
            session.messages = vec![history("older", "", vec![task()]), history(id, tool, vec![]), history("unrelated", "read", vec![])];
            assert!(session.session_todos().is_empty(), "{id} / {tool}");
        }
        let mut session = crate::agent::Session::default();
        session.messages = vec![history("persisted", "", vec![task()]), history("unrelated", "read", vec![])];
        assert_eq!(session.session_todos(), &[task()]);
        assert!(crate::agent::Session::default().session_todos().is_empty());
    }

    #[test]
    fn statuses_keep_unknown_values_pending() {
        assert_eq!(Status::of("completed"), Status::Completed);
        assert_eq!(Status::of("in_progress"), Status::InProgress);
        for status in ["pending", "cancelled", "", "unknown"] {
            assert_eq!(Status::of(status), Status::Pending);
        }
    }
}