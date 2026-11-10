use super::*;
use crate::ui::ChildIf;
use crate::ui::theme::rgb;
use rune::{El, Key, Tx, col, el, row, text};

fn button(app: &App, label: &str) -> El {
    el().h(30.0 * app.ty.scale())
        .px(10.0 * app.ty.scale())
        .center()
        .radius(4.0)
        .fill(rgb(app.pal.surface))
        .hover(rgb(app.pal.hover))
        .child(
            text(label.to_owned())
                .font(app.ty.sized(12.0))
                .color(rgb(app.pal.fg)),
        )
}

pub fn schema(app: &App, db: &Database) -> El {
    let mut body = col()
        .key("database-schema")
        .w_full()
        .grow(1.0)
        .shrink()
        .padding(16.0)
        .gap(10.0)
        .scroll(rune::ScrollSpec {
            stick_bottom: false,
        });
    let Some(object) = db
        .objects
        .iter()
        .find(|o| Some(&o.name) == db.table.as_ref())
    else {
        return body;
    };
    let heading = |label: &str| {
        text(label.to_owned())
            .font(app.ty.sized_bold(13.0))
            .color(rgb(app.pal.fg))
            .mt(10.0)
    };
    let detail = |label: String| {
        text(label)
            .font(app.ty.sized(12.0))
            .color(rgb(app.pal.dim))
            .w_full()
            .wrap()
    };
    body = body.child(heading("Columns"));
    for column in &object.columns {
        body = body.child(detail(format!(
            "{} · {}{}{}{}",
            column.name,
            column.data_type,
            if column.primary_key > 0 {
                " · PRIMARY KEY"
            } else {
                ""
            },
            if column.not_null { " · NOT NULL" } else { "" },
            column
                .default_value
                .as_ref()
                .map_or(String::new(), |v| format!(" · DEFAULT {v}"))
        )));
    }
    body = body.child(heading("Indexes"));
    if object.indexes.is_empty() {
        body = body.child(detail("No indexes".into()));
    }
    for index in &object.indexes {
        body = body.child(detail(format!(
            "{}{} ({})",
            index.name,
            if index.unique { " · UNIQUE" } else { "" },
            index.columns.join(", ")
        )));
    }
    body = body.child(heading("Triggers"));
    if object.triggers.is_empty() {
        body = body.child(detail("No triggers".into()));
    }
    for trigger in &object.triggers {
        body = body.child(detail(format!("{}\n{}", trigger.name, trigger.sql)));
    }
    body.child(heading("SQL")).child(detail(object.sql.clone()))
}

pub fn filters(app: &App, db: &Database) -> El {
    let mut bar = row().w_full().gap(6.0).px(12.0).py(6.0).scroll_x();
    for (column, values) in &db.filters {
        let (path, name) = (db.path.clone(), column.clone());
        bar = bar.child(
            button(app, &format!("{column}: {} selected ×", values.len()))
                .on_press(move |a: &mut App| a.database_filter_clear(&path, &name)),
        );
    }
    bar
}

pub fn filter_picker(app: &App, db: &Database, picker: &FilterPicker) -> El {
    let p = app.pal;
    let font = app.ty.sized(12.0);
    let path = db.path.clone();
    let path_key = path.clone();
    let search = rune::text_input(&picker.search, font)
        .key("database-filter-search")
        .w_full()
        .h(32.0)
        .color(rgb(p.fg))
        .placeholder("Search unique values", rgb(p.muted))
        .on_edit(move |a: &mut App| {
            &mut a
                .databases
                .get_mut(&path)
                .unwrap()
                .filter_picker
                .as_mut()
                .unwrap()
                .search
        })
        .on_input_key(move |a: &mut App, e: &rune::KeyEvent, _: &mut Tx| {
            if e.key == Key::Escape {
                a.databases.get_mut(&path_key).unwrap().filter_picker = None;
                return true;
            }
            if e.key == Key::Enter {
                a.database_filter_apply(&path_key, false);
                return true;
            }
            false
        });
    let mut values = col().w_full().h(180.0).scroll(rune::ScrollSpec {
        stick_bottom: false,
    });
    let query = picker.search.text.to_lowercase();
    let shown: Vec<_> = picker
        .values
        .iter()
        .filter(|v| display(&v.value).to_lowercase().contains(&query))
        .collect();
    if picker.loading {
        values =
            values.child(text("Loading unique values…").font(font).color(rgb(p.dim)));
    } else if shown.is_empty() {
        values = values.child(text("No matching values").font(font).color(rgb(p.dim)));
    }
    for (i, value) in shown.iter().enumerate() {
        let selected = picker.selected.contains(&value.value);
        let (path, value_copy) = (db.path.clone(), value.value.clone());
        values = values.child(
            row()
                .key(format!("database-filter-value:{i}"))
                .w_full()
                .h(27.0)
                .gap(8.0)
                .px(8.0)
                .center_y()
                .hover(rgb(p.hover))
                .child(
                    text(if selected { "✓" } else { " " })
                        .font(font)
                        .color(rgb(p.accent)),
                )
                .child(
                    text(display(&value.value))
                        .font(font)
                        .color(rgb(p.fg))
                        .grow(1.0)
                        .truncate(),
                )
                .child(text(value.count.to_string()).font(font).color(rgb(p.dim)))
                .on_press(move |a: &mut App| {
                    if let Some(picker) = a
                        .databases
                        .get_mut(&path)
                        .and_then(|db| db.filter_picker.as_mut())
                    {
                        if let Some(index) =
                            picker.selected.iter().position(|v| v == &value_copy)
                        {
                            picker.selected.remove(index);
                        } else {
                            picker.selected.push(value_copy.clone());
                        }
                    }
                }),
        );
    }
    let mut actions = row().gap(8.0);
    for (label, action) in [
        ("Apply filter", 0),
        ("Clear filter", 1),
        ("Select none", 2),
        ("Cancel", 3),
    ] {
        let path = db.path.clone();
        actions = actions.child(
            button(app, label)
                .key(format!("database-filter-action:{action}"))
                .on_press(move |a: &mut App| match action {
                    0 | 1 => a.database_filter_apply(&path, action == 1),
                    2 => {
                        if let Some(p) = a
                            .databases
                            .get_mut(&path)
                            .and_then(|d| d.filter_picker.as_mut())
                        {
                            p.selected.clear();
                        }
                    }
                    _ => a.databases.get_mut(&path).unwrap().filter_picker = None,
                }),
        );
    }
    col().key("database-filter-picker").w_full().padding(12.0).gap(8.0).fill(rgb(p.surface))
        .child(text(format!("Filter {}", picker.column)).font(app.ty.sized_bold(13.0)).color(rgb(p.fg)))
        .child(search).child(values)
        .child_if(picker.truncated, || text("Only the first values are listed; clear the filter to include all values.").font(font).color(rgb(p.yellow)).wrap())
        .child(actions)
}

pub fn cell(app: &App, db: &Database, r: usize, c: usize, edit: &TextEdit) -> El {
    let font = app.ty.sized(12.0);
    let mut kinds = row().gap(6.0);
    for kind in [
        CellKind::Text,
        CellKind::Integer,
        CellKind::Real,
        CellKind::Null,
        CellKind::Default,
    ] {
        if kind == CellKind::Default && r < db.original.len() {
            continue;
        }
        let path = db.path.clone();
        kinds = kinds.child(
            button(app, kind.label())
                .key(format!("database-cell-kind:{}", kind.label()))
                .border(
                    1.0,
                    rgb(if db.cell_kind == kind {
                        app.pal.accent
                    } else {
                        app.pal.border
                    }),
                )
                .on_press(move |a: &mut App| {
                    a.databases.get_mut(&path).unwrap().cell_kind = kind
                }),
        );
    }
    let path = db.path.clone();
    let path_key = path.clone();
    let input = rune::text_input(edit, font)
        .key("database-cell")
        .w_full()
        .h(32.0)
        .color(rgb(app.pal.fg))
        .on_edit(move |a: &mut App| {
            &mut a.databases.get_mut(&path).unwrap().cell.as_mut().unwrap().2
        })
        .on_input_key(
            move |a: &mut App, e: &rune::KeyEvent, _: &mut Tx| match e.key {
                Key::Enter => {
                    a.database_commit_cell(&path_key, false);
                    true
                }
                Key::Escape => {
                    a.databases.get_mut(&path_key).unwrap().cell = None;
                    true
                }
                _ => false,
            },
        );
    let path = db.path.clone();
    let cancel = path.clone();
    col()
        .w_full()
        .padding(12.0)
        .gap(8.0)
        .fill(rgb(app.pal.surface))
        .child(
            text(format!(
                "Row {} · {}",
                u64::from(db.offset) + r as u64 + 1,
                db.columns[c]
            ))
            .font(font)
            .color(rgb(app.pal.dim)),
        )
        .child(kinds)
        .child(input)
        .child_if(
            matches!(db.cell_kind, CellKind::Null | CellKind::Default),
            || {
                text(if db.cell_kind == CellKind::Default {
                    "Use the column default; input text is ignored."
                } else {
                    "Store SQL NULL; input text is ignored."
                })
                .font(font)
                .color(rgb(app.pal.dim))
            },
        )
        .child(
            row()
                .gap(8.0)
                .child(
                    button(app, "Set value").key("database-cell-save").on_press(
                        move |a: &mut App| a.database_commit_cell(&path, false),
                    ),
                )
                .child(button(app, "Cancel").on_press(move |a: &mut App| {
                    a.databases.get_mut(&cancel).unwrap().cell = None
                })),
        )
}
