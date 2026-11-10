use super::*;
use crate::ui::theme::rgb;
use rune::{El, Key, Tx, col, el, row, text};

pub fn view(app: &App) -> El {
    let Some(db) = app.active_database() else {
        return el();
    };
    let p = app.pal;
    let s = app.ty.scale();
    let font = app.ty.sized(12.0);
    let key = db.path.clone();
    let button = |label: String| {
        el().h(30.0 * s)
            .px(12.0 * s)
            .center()
            .radius(5.0 * s)
            .fill(rgb(p.surface))
            .hover(rgb(p.hover))
            .child(text(label).font(font).color(rgb(p.fg)))
    };
    if db.path.is_empty() {
        return col()
            .w_full()
            .grow(1.0)
            .padding(28.0 * s)
            .gap(16.0 * s)
            .child(
                text("Database")
                    .font(app.ty.sized_bold(20.0))
                    .color(rgb(p.fg)),
            )
            .child(
                text("Open a workspace SQLite file (.db, .sqlite or .sqlite3)")
                    .font(font)
                    .color(rgb(p.dim)),
            )
            .child(
                rune::text_input(&db.path_input, font)
                    .key("database-path")
                    .h(34.0 * s)
                    .w_full()
                    .color(rgb(p.fg))
                    .placeholder("Path to database", rgb(p.muted))
                    .on_edit(|a: &mut App| {
                        &mut a.databases.get_mut("").unwrap().path_input
                    })
                    .on_input_key(|a: &mut App, e: &rune::KeyEvent, _: &mut Tx| {
                        if e.key == Key::Enter {
                            let path =
                                a.databases.get("").unwrap().path_input.text.clone();
                            if !path.trim().is_empty() {
                                a.open_database(path.trim());
                            }
                            true
                        } else {
                            false
                        }
                    }),
            )
            .child(button("Open".into()).on_press(|a: &mut App| {
                let path = a.databases.get("").unwrap().path_input.text.clone();
                if !path.trim().is_empty() {
                    a.open_database(path.trim());
                }
            }));
    }
    let mut tables = col()
        .w(200.0 * s)
        .h_full()
        .padding(10.0 * s)
        .gap(3.0 * s)
        .fill(rgb(p.surface))
        .scroll(rune::ScrollSpec {
            stick_bottom: false,
        })
        .child(
            text("TABLES & VIEWS")
                .font(app.ty.sized_bold(11.0))
                .color(rgb(p.dim))
                .mb(12.0 * s),
        );
    for object in &db.objects {
        let (path, name) = (key.clone(), object.name.clone());
        let selected = db.table.as_deref() == Some(name.as_str());
        tables = tables.child(
            el().w_full()
                .h(30.0 * s)
                .px(8.0 * s)
                .center_y()
                .radius(4.0 * s)
                .fill(if selected {
                    rgb(p.hover)
                } else {
                    rune::Color::TRANSPARENT
                })
                .hover(rgb(p.hover))
                .child(
                    text(name.clone())
                        .font(font)
                        .color(rgb(if selected { p.fg } else { p.dim }))
                        .truncate(),
                )
                .on_press(move |a: &mut App| a.database_table(&path, &name)),
        );
    }
    let changes = db.changes().len();
    let path = key.clone();
    let path2 = key.clone();
    let mut toolbar = row()
        .w_full()
        .h(46.0 * s)
        .px(12.0 * s)
        .gap(8.0 * s)
        .center_y()
        .child(
            text(db.table.clone().unwrap_or_else(|| "No tables".into()))
                .font(app.ty.sized_bold(14.0))
                .color(rgb(p.fg))
                .grow(1.0),
        )
        .child(
            button(format!("Apply ({changes})"))
                .key("database-apply")
                .on_press(move |a: &mut App| a.database_apply(&path)),
        )
        .child(
            button("Discard".into())
                .key("database-discard")
                .on_press(move |a: &mut App| a.database_discard(&path2)),
        );
    if db.editable {
        let path = key.clone();
        toolbar = toolbar.child(button("+ Row".into()).on_press(move |a: &mut App| {
            if let Some(db) = a.databases.get_mut(&path) {
                if db.pending.is_none() {
                    db.rows.push(vec![DatabaseValue::Null; db.columns.len()]);
                    db.deleted.push(false);
                }
            }
        }));
    }
    let col_w = 170.0 * s;
    let row_h = 25.0 * s;
    let mut header = row()
        .h(30.0 * s)
        .fill(rgb(p.surface))
        .child(el().w(40.0 * s));
    for column in &db.columns {
        let (path, name) = (key.clone(), column.clone());
        let suffix = db
            .sort
            .as_ref()
            .filter(|sort| sort.column == *column)
            .map_or("", |sort| if sort.descending { " ↓" } else { " ↑" });
        header = header.child(
            el().w(col_w)
                .h(30.0 * s)
                .px(8.0 * s)
                .center_y()
                .border(1.0, rgb(p.border))
                .hover(rgb(p.hover))
                .child(
                    text(format!("{column}{suffix}"))
                        .font(font)
                        .color(rgb(p.fg))
                        .truncate(),
                )
                .on_press(move |a: &mut App| a.database_sort(&path, &name)),
        );
    }
    let mut grid = col()
        .w(40.0 * s + col_w * db.columns.len() as f32)
        .child(header);
    for (r, values) in db.rows.iter().enumerate() {
        let path = key.clone();
        let deleted = db.deleted[r];
        let mut line = row()
            .h(row_h)
            .fill(if deleted {
                rgb(p.red).alpha(0.15)
            } else if r % 2 == 1 {
                rgb(p.surface).alpha(0.35)
            } else {
                rune::Color::TRANSPARENT
            })
            .child(
                el().w(40.0 * s)
                    .h(row_h)
                    .center()
                    .child(
                        text(if deleted {
                            "×".into()
                        } else {
                            (u64::from(db.offset) + r as u64 + 1).to_string()
                        })
                        .font(font)
                        .color(rgb(p.dim)),
                    )
                    .on_press(move |a: &mut App| {
                        if let Some(db) = a.databases.get_mut(&path) {
                            if db.editable
                                && db.pending.is_none()
                                && (r >= db.original.len()
                                    || db.identities.get(r).is_some_and(Option::is_some))
                            {
                                db.deleted[r] = !db.deleted[r];
                            }
                        }
                    }),
            );
        for (c, value) in values.iter().enumerate() {
            let path = key.clone();
            let changed = db.original.get(r).and_then(|v| v.get(c)) != Some(value);
            line = line.child(
                el().key(format!("database-value:{r}:{c}"))
                    .w(col_w)
                    .h(row_h)
                    .px(8.0 * s)
                    .center_y()
                    .border(0.5 * s, rgb(p.border))
                    .hover(rgb(p.hover))
                    .child(
                        text(display(value))
                            .font(font)
                            .color(rgb(if changed {
                                p.yellow
                            } else if matches!(value, DatabaseValue::Null) {
                                p.muted
                            } else {
                                p.fg
                            }))
                            .truncate(),
                    )
                    .on_press(move |a: &mut App| a.database_edit(&path, r, c)),
            );
        }
        grid = grid.child(line);
    }
    let path = key.clone();
    let path2 = key.clone();
    let footer = row()
        .w_full()
        .h(40.0 * s)
        .px(12.0 * s)
        .center_y()
        .gap(10.0 * s)
        .child(
            text(if db.pending.is_some() {
                "Loading…".into()
            } else {
                format!(
                    "{} rows · {}–{} · {}",
                    db.total,
                    if db.total == 0 { 0 } else { db.offset + 1 },
                    (u64::from(db.offset) + db.rows.len() as u64).min(db.total),
                    if db.editable {
                        "Editable · row number marks deletion"
                    } else {
                        "Read only"
                    }
                )
            })
            .font(font)
            .color(rgb(p.dim))
            .grow(1.0)
            .truncate(),
        )
        .child(
            button("Previous".into())
                .on_press(move |a: &mut App| a.database_page(&path, false)),
        )
        .child(
            button("Next".into())
                .key("database-next")
                .on_press(move |a: &mut App| a.database_page(&path2, true)),
        );
    let mut body = col()
        .grow(1.0)
        .shrink()
        .h_full()
        .child(toolbar)
        .child(col().w_full().grow(1.0).shrink().scroll_xy().child(grid))
        .child(footer);
    if let Some(error) = &db.error {
        body = body.child(
            text(error.clone())
                .font(font)
                .color(rgb(p.red))
                .px(12.0 * s)
                .py(8.0 * s),
        );
    }
    if let Some((r, c, edit)) = &db.cell {
        let path = key.clone();
        let path2 = key.clone();
        let path3 = key.clone();
        let path4 = key.clone();
        body = body.child(
            col()
                .w_full()
                .padding(12.0 * s)
                .gap(8.0 * s)
                .fill(rgb(p.surface))
                .child(
                    text(format!("Row {} · {}", r + 1, db.columns[*c]))
                        .font(font)
                        .color(rgb(p.dim)),
                )
                .child(
                    rune::text_input(edit, font)
                        .key("database-cell")
                        .w_full()
                        .h(32.0 * s)
                        .color(rgb(p.fg))
                        .on_edit(move |a: &mut App| {
                            &mut a
                                .databases
                                .get_mut(&path)
                                .unwrap()
                                .cell
                                .as_mut()
                                .unwrap()
                                .2
                        })
                        .on_input_key(
                            move |a: &mut App, e: &rune::KeyEvent, _: &mut Tx| match e.key
                            {
                                Key::Enter => {
                                    a.database_commit_cell(&path2, false);
                                    true
                                }
                                Key::Escape => {
                                    a.databases.get_mut(&path2).unwrap().cell = None;
                                    true
                                }
                                _ => false,
                            },
                        ),
                )
                .child(
                    row()
                        .gap(8.0 * s)
                        .child(button("Set value".into()).on_press(move |a: &mut App| {
                            a.database_commit_cell(&path3, false)
                        }))
                        .child(button("Set NULL".into()).on_press(move |a: &mut App| {
                            a.database_commit_cell(&path4, true)
                        })),
                ),
        );
    }
    row().w_full().h_full().child(tables).child(body)
}
