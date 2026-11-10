//! Workspace SQLite tables over the existing Database protocol. Drafts stay in their tab.
mod controls;
mod view;
use crate::{app::App, daemon::Family, tabs::TabKind};
use neoism_protocol::database::*;
use rune::TextEdit;
use std::collections::{BTreeMap, BTreeSet};
pub use view::view;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CellKind {
    Text,
    Integer,
    Real,
    Null,
    Default,
}
impl CellKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Integer => "integer",
            Self::Real => "real",
            Self::Null => "NULL",
            Self::Default => "DEFAULT",
        }
    }
}
pub struct FilterPicker {
    pub column: String,
    pub values: Vec<DatabaseValueCount>,
    pub selected: Vec<DatabaseValue>,
    pub search: TextEdit,
    pub truncated: bool,
    pub loading: bool,
}

pub struct Database {
    pub filters: BTreeMap<String, Vec<DatabaseValue>>,
    pub filter_picker: Option<FilterPicker>,
    pub show_schema: bool,
    pub default_cells: BTreeSet<(usize, usize)>,
    pub cell_kind: CellKind,
    pub path: String,
    pub scope: String,
    pub path_input: TextEdit,
    pub objects: Vec<DatabaseObject>,
    pub table: Option<String>,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<DatabaseValue>>,
    pub original: Vec<Vec<DatabaseValue>>,
    pub identities: Vec<Option<DatabaseRecord>>,
    pub deleted: Vec<bool>,
    pub total: u64,
    pub offset: u32,
    pub sort: Option<DatabaseSort>,
    pub editable: bool,
    pub pending: Option<u64>,
    pub error: Option<String>,
    pub applying: bool,
    pub uncertain: bool,
    pub cell: Option<(usize, usize, TextEdit)>,
}
impl Database {
    fn new(path: &str, scope: &str) -> Self {
        Self {
            filters: BTreeMap::new(),
            filter_picker: None,
            show_schema: false,
            default_cells: BTreeSet::new(),
            cell_kind: CellKind::Text,
            path: path.into(),
            scope: scope.into(),
            path_input: TextEdit::new(path),
            objects: vec![],
            table: None,
            columns: vec![],
            rows: vec![],
            original: vec![],
            identities: vec![],
            deleted: vec![],
            total: 0,
            offset: 0,
            sort: None,
            editable: false,
            pending: None,
            error: None,
            cell: None,
            applying: false,
            uncertain: false,
        }
    }
    pub fn changes(&self) -> Vec<DatabaseChange> {
        let record = |row: &[DatabaseValue]| {
            self.columns
                .iter()
                .cloned()
                .zip(row.iter().cloned())
                .collect::<BTreeMap<_, _>>()
        };
        self.rows
            .iter()
            .enumerate()
            .filter_map(|(i, row)| {
                if i >= self.original.len() {
                    return (!self.deleted[i]).then(|| DatabaseChange {
                        kind: DatabaseChangeKind::Insert,
                        values: record(row)
                            .into_iter()
                            .filter(|(name, _)| {
                                let column =
                                    self.columns.iter().position(|c| c == name).unwrap();
                                !self.default_cells.contains(&(i, column))
                            })
                            .collect(),
                        identity: None,
                        original: None,
                    });
                }
                let identity = self.identities.get(i)?.clone()?;
                if self.deleted[i] {
                    return Some(DatabaseChange {
                        kind: DatabaseChangeKind::Delete,
                        values: BTreeMap::new(),
                        identity: Some(identity),
                        original: Some(record(&self.original[i])),
                    });
                }
                let values = row
                    .iter()
                    .enumerate()
                    .filter(|(j, v)| self.original[i].get(*j) != Some(*v))
                    .map(|(j, v)| (self.columns[j].clone(), v.clone()))
                    .collect::<BTreeMap<_, _>>();
                (!values.is_empty()).then(|| DatabaseChange {
                    kind: DatabaseChangeKind::Update,
                    values,
                    identity: Some(identity),
                    original: Some(record(&self.original[i])),
                })
            })
            .collect()
    }
    pub fn locked(&self) -> bool {
        self.pending.is_some() || self.cell.is_some() || !self.changes().is_empty()
    }
}
pub fn is_path(path: &str) -> bool {
    path.rsplit_once('.').is_some_and(|(_, ext)| {
        matches!(
            ext.to_ascii_lowercase().as_str(),
            "db" | "sqlite" | "sqlite3" | "db3" | "turso" | "tursodb"
        )
    })
}
pub fn display(value: &DatabaseValue) -> String {
    match value {
        DatabaseValue::Null => "NULL".into(),
        DatabaseValue::Integer(s) | DatabaseValue::Text(s) => s.clone(),
        DatabaseValue::Real(n) => n.to_string(),
        DatabaseValue::Blob(s) => format!("BLOB ({} bytes)", s.len() / 2),
    }
}
fn edited(value: &DatabaseValue, text: &str) -> Result<DatabaseValue, String> {
    Ok(match value {
        DatabaseValue::Integer(_) => DatabaseValue::Integer(
            text.parse::<i64>()
                .map_err(|_| "Expected a 64-bit integer")?
                .to_string(),
        ),
        DatabaseValue::Real(_) => {
            let n = text.parse::<f64>().map_err(|_| "Expected a number")?;
            if !n.is_finite() {
                return Err("Expected a finite number".into());
            }
            DatabaseValue::Real(n)
        }
        DatabaseValue::Blob(_) => return Err("Binary cells are read-only".into()),
        _ => DatabaseValue::Text(text.to_owned()),
    })
}
impl App {
    pub fn open_database(&mut self, path: &str) {
        let path = if path.is_empty() {
            String::new()
        } else {
            std::path::Path::new(&self.cwd)
                .join(path)
                .to_string_lossy()
                .into_owned()
        };
        if let Some(index) = self
            .tabs
            .iter()
            .position(|t| t.kind == (TabKind::Database { path: path.clone() }))
        {
            self.select_tab(self.tabs[index].id);
            return;
        }
        self.databases
            .entry(path.clone())
            .or_insert_with(|| Database::new(&path, &self.cwd));
        self.open_tab(TabKind::Database { path: path.clone() });
        self.focus = crate::app::Focus::Content;
        if !path.is_empty() {
            self.database_request(&path, DatabaseRequest::Schema { path: path.clone() });
        }
    }
    pub fn database_disconnected(&mut self) {
        for db in self.databases.values_mut() {
            if db.pending.take().is_some() {
                db.uncertain |= db.applying;
                db.error = Some(if db.uncertain { "Connection lost during apply. Discard and reload to verify the saved rows before editing again." } else { "Workspace daemon disconnected. Reopen the database to retry." }.into());
                db.applying = false;
            }
        }
    }

    pub fn active_database(&self) -> Option<&Database> {
        let TabKind::Database { path } = &self.active_tab().kind else {
            return None;
        };
        self.databases.get(path)
    }
    fn database_request(&mut self, key: &str, request: DatabaseRequest) {
        let Some(db) = self.databases.get_mut(key) else {
            return;
        };
        db.error = None;
        db.applying = request.is_mutation();
        let Some(daemon) = &self.daemon else {
            db.error = Some("Connect to the workspace daemon to open databases".into());
            return;
        };
        db.pending = Some(daemon.request(
            Family::Database,
            serde_json::to_value(request).expect("database request"),
            Some(db.scope.clone()),
        ));
    }
    pub fn database_browse(&mut self, key: &str) {
        let Some(db) = self.databases.get(key) else {
            return;
        };
        let Some(table) = db.table.clone() else {
            return;
        };
        self.database_request(
            key,
            DatabaseRequest::Browse {
                path: db.path.clone(),
                table,
                limit: 50,
                offset: db.offset,
                sort: db.sort.clone(),
                filters: db
                    .filters
                    .iter()
                    .map(|(column, values)| DatabaseFilter {
                        column: column.clone(),
                        values: values.clone(),
                    })
                    .collect(),
            },
        );
    }
    pub fn database_table(&mut self, key: &str, table: &str) {
        let Some(db) = self.databases.get_mut(key) else {
            return;
        };
        if db.locked() {
            db.error = Some("Apply or discard changes before switching tables".into());
            return;
        }
        db.table = Some(table.into());
        db.offset = 0;
        db.sort = None;
        db.filters.clear();
        db.filter_picker = None;
        self.database_browse(key);
    }
    pub fn database_page(&mut self, key: &str, next: bool) {
        let Some(db) = self.databases.get_mut(key) else {
            return;
        };
        if db.locked() {
            return;
        }
        db.offset = if next && u64::from(db.offset) + 50 < db.total {
            db.offset.saturating_add(50)
        } else if !next {
            db.offset.saturating_sub(50)
        } else {
            return;
        };
        self.database_browse(key);
    }
    pub fn database_sort(&mut self, key: &str, column: &str) {
        let Some(db) = self.databases.get_mut(key) else {
            return;
        };
        if db.locked() {
            return;
        }
        let descending = db
            .sort
            .as_ref()
            .is_some_and(|s| s.column == column && !s.descending);
        db.sort = Some(DatabaseSort {
            column: column.into(),
            descending,
        });
        db.offset = 0;
        self.database_browse(key);
    }
    pub fn database_add_row(&mut self, key: &str) {
        let Some(db) = self.databases.get_mut(key) else {
            return;
        };
        if !db.editable || db.pending.is_some() || db.cell.is_some() || db.uncertain {
            return;
        }
        let r = db.rows.len();
        db.default_cells
            .extend((0..db.columns.len()).map(|c| (r, c)));
        db.rows.push(vec![DatabaseValue::Null; db.columns.len()]);
        db.deleted.push(false);
    }
    pub fn database_filter_open(&mut self, key: &str, column: &str) {
        let Some(db) = self.databases.get_mut(key) else {
            return;
        };
        if db.locked() || db.uncertain {
            db.error = Some("Apply or discard changes before filtering".into());
            return;
        }
        let Some(table) = db.table.clone() else {
            return;
        };
        let filters = db
            .filters
            .iter()
            .filter(|(name, _)| name.as_str() != column)
            .map(|(column, values)| DatabaseFilter {
                column: column.clone(),
                values: values.clone(),
            })
            .collect();
        let path = db.path.clone();
        let mut search = TextEdit::new("");
        search.focused = true;
        db.filter_picker = Some(FilterPicker {
            column: column.into(),
            values: vec![],
            selected: vec![],
            search,
            truncated: false,
            loading: true,
        });
        self.database_request(
            key,
            DatabaseRequest::Values {
                path,
                table,
                column: column.into(),
                filters,
            },
        );
    }
    pub fn database_filter_apply(&mut self, key: &str, clear: bool) {
        let Some(db) = self.databases.get_mut(key) else {
            return;
        };
        if db.locked() || db.uncertain {
            return;
        }
        let Some(picker) = db.filter_picker.take() else {
            return;
        };
        if clear {
            db.filters.remove(&picker.column);
        } else {
            db.filters.insert(picker.column, picker.selected);
        }
        db.offset = 0;
        self.database_browse(key);
    }
    pub fn database_filter_clear(&mut self, key: &str, column: &str) {
        let Some(db) = self.databases.get_mut(key) else {
            return;
        };
        if db.locked() || db.uncertain {
            return;
        }
        db.filters.remove(column);
        db.offset = 0;
        db.filter_picker = None;
        self.database_browse(key);
    }
    pub fn database_edit(&mut self, key: &str, row: usize, column: usize) {
        let Some(db) = self.databases.get_mut(key) else {
            return;
        };
        if !db.editable
            || db.pending.is_some()
            || db.deleted.get(row) != Some(&false)
            || row < db.original.len()
                && db.identities.get(row).is_none_or(Option::is_none)
        {
            return;
        }
        let Some(value) = db.rows.get(row).and_then(|r| r.get(column)) else {
            return;
        };
        if matches!(value, DatabaseValue::Blob(_)) {
            db.error = Some("Binary cells are read-only".into());
            return;
        }
        db.cell_kind = if db.default_cells.contains(&(row, column)) {
            CellKind::Default
        } else {
            match value {
                DatabaseValue::Integer(_) => CellKind::Integer,
                DatabaseValue::Real(_) => CellKind::Real,
                DatabaseValue::Null => CellKind::Null,
                _ => CellKind::Text,
            }
        };
        let mut edit = TextEdit::new(if matches!(value, DatabaseValue::Null) {
            String::new()
        } else {
            display(value)
        });
        edit.focused = true;
        edit.set_selection(0, edit.text.len());
        db.cell = Some((row, column, edit));
    }
    pub fn database_commit_cell(&mut self, key: &str, null: bool) {
        let Some(db) = self.databases.get_mut(key) else {
            return;
        };
        let Some((r, c, edit)) = db.cell.take() else {
            return;
        };
        let kind = if null { CellKind::Null } else { db.cell_kind };
        let value = match kind {
            CellKind::Default if r < db.original.len() => {
                Err("Defaults apply only to new rows".into())
            }
            CellKind::Null | CellKind::Default => Ok(DatabaseValue::Null),
            CellKind::Integer => edited(&DatabaseValue::Integer("0".into()), &edit.text),
            CellKind::Real => edited(&DatabaseValue::Real(0.0), &edit.text),
            CellKind::Text => Ok(DatabaseValue::Text(edit.text.clone())),
        };
        match value {
            Ok(v) => {
                db.rows[r][c] = v;
                if kind == CellKind::Default {
                    db.default_cells.insert((r, c));
                } else {
                    db.default_cells.remove(&(r, c));
                }
                db.error = None
            }
            Err(e) => {
                db.error = Some(e);
                db.cell = Some((r, c, edit));
            }
        }
    }
    pub fn database_apply(&mut self, key: &str) {
        let Some(db) = self.databases.get(key) else {
            return;
        };
        if db.pending.is_some() || db.cell.is_some() || db.uncertain {
            return;
        }
        let changes = db.changes();
        if changes.is_empty() {
            return;
        }
        let Some(table) = db.table.clone() else {
            return;
        };
        self.database_request(
            key,
            DatabaseRequest::Apply {
                path: db.path.clone(),
                table,
                changes,
            },
        );
    }
    pub fn database_discard(&mut self, key: &str) {
        let Some(db) = self.databases.get_mut(key) else {
            return;
        };
        if db.pending.is_some() {
            return;
        }
        db.rows = db.original.clone();
        db.deleted = vec![false; db.rows.len()];
        db.cell = None;
        db.default_cells.clear();
        db.error = None;
        if std::mem::take(&mut db.uncertain) {
            self.database_browse(key);
        }
    }
    pub fn apply_database_reply(
        &mut self,
        id: u64,
        message: Result<Box<DatabaseReply>, String>,
    ) {
        let Some(key) = self
            .databases
            .iter()
            .find(|(_, d)| d.pending == Some(id))
            .map(|(k, _)| k.clone())
        else {
            return;
        };
        let db = self.databases.get_mut(&key).unwrap();
        db.pending = None;
        db.applying = false;
        let reply = message.map(|reply| *reply);
        match reply {
            Ok(DatabaseReply::Schema { objects }) => {
                db.objects = objects;
                db.table = db.objects.first().map(|o| o.name.clone());
                self.database_browse(&key);
            }
            Ok(DatabaseReply::Rows {
                columns,
                rows,
                identities,
                total,
                editable,
            }) => {
                db.default_cells.clear();
                db.cell = None;
                db.columns = columns;
                db.original = rows.clone();
                db.rows = rows;
                db.identities = identities;
                db.total = total;
                db.editable = editable;
                db.deleted = vec![false; db.rows.len()];
            }
            Ok(DatabaseReply::Values { values, truncated }) => {
                if let Some(picker) = &mut db.filter_picker {
                    picker.selected =
                        db.filters.get(&picker.column).cloned().unwrap_or_else(|| {
                            values.iter().map(|v| v.value.clone()).collect()
                        });
                    picker.values = values;
                    picker.truncated = truncated;
                    picker.loading = false;
                }
            }
            Ok(DatabaseReply::Applied { .. }) => self.database_browse(&key),
            Ok(DatabaseReply::Error { message }) => db.error = Some(message),
            Err(e) => db.error = Some(format!("Invalid database response: {e}")),
        }
        if let Some(db) = self.databases.get_mut(&key) {
            if db.error.is_some() {
                if let Some(p) = &mut db.filter_picker {
                    p.loading = false;
                }
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inserted_defaults_are_omitted_but_explicit_null_is_sent() {
        let mut db = Database::new("x.db", "/");
        db.columns = vec!["id".into(), "title".into(), "done".into()];
        db.rows = vec![vec![
            DatabaseValue::Null,
            DatabaseValue::Text("draft".into()),
            DatabaseValue::Null,
        ]];
        db.deleted = vec![false];
        db.default_cells.insert((0, 0));
        let changes = db.changes();
        assert_eq!(changes.len(), 1);
        assert!(!changes[0].values.contains_key("id"));
        assert_eq!(changes[0].values["done"], DatabaseValue::Null);
        assert_eq!(
            changes[0].values["title"],
            DatabaseValue::Text("draft".into())
        );
        db.deleted[0] = true;
        assert!(db.changes().is_empty());
    }

    #[test]
    fn integer_precision_and_null_are_preserved() {
        assert_eq!(
            edited(&DatabaseValue::Integer("0".into()), "9223372036854775807").unwrap(),
            DatabaseValue::Integer("9223372036854775807".into())
        );
        assert!(
            edited(&DatabaseValue::Integer("0".into()), "9223372036854775808").is_err()
        );
        assert_eq!(
            edited(&DatabaseValue::Text("".into()), "NULL").unwrap(),
            DatabaseValue::Text("NULL".into())
        );
        assert!(!is_path("x.db-wal"));
    }
    #[test]
    fn updates_include_original_for_conflict_detection() {
        let mut db = Database::new("x.db", "/");
        db.columns = vec!["id".into(), "name".into()];
        db.rows = vec![vec![
            DatabaseValue::Integer("1".into()),
            DatabaseValue::Text("old".into()),
        ]];
        db.original = db.rows.clone();
        db.identities = vec![Some(BTreeMap::from([(
            "id".into(),
            DatabaseValue::Integer("1".into()),
        )]))];
        db.deleted = vec![false];
        db.rows[0][1] = DatabaseValue::Text("new".into());
        let c = db.changes();
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].values.len(), 1);
        assert_eq!(
            c[0].original.as_ref().unwrap()["name"],
            DatabaseValue::Text("old".into())
        );
    }
}

pub fn fixture(app: &mut App) {
    let mut db = Database::new("fixture.sqlite", "/tmp");
    db.table = Some("tasks".into());
    db.objects = serde_json::from_value(serde_json::json!([{"name":"tasks","kind":"table","sql":"CREATE TABLE tasks(id INTEGER PRIMARY KEY, title TEXT, done INTEGER)","columns":[{"name":"id","data_type":"INTEGER","not_null":false,"default_value":null,"primary_key":1},{"name":"title","data_type":"TEXT","not_null":false,"default_value":"'Untitled'","primary_key":0},{"name":"done","data_type":"INTEGER","not_null":true,"default_value":"0","primary_key":0}],"indexes":[{"name":"tasks_title","unique":false,"columns":["title"]}],"triggers":[]}])).unwrap();
    db.columns = vec!["id".into(), "title".into(), "done".into()];
    db.rows = (0..12)
        .map(|i| {
            vec![
                DatabaseValue::Integer((i + 1).to_string()),
                DatabaseValue::Text(format!("Task {}", i + 1)),
                DatabaseValue::Integer("0".into()),
            ]
        })
        .collect();
    db.original = db.rows.clone();
    db.identities = (0..12)
        .map(|i| {
            Some(BTreeMap::from([(
                "id".into(),
                DatabaseValue::Integer((i + 1).to_string()),
            )]))
        })
        .collect();
    db.deleted = vec![false; 12];
    db.total = 12;
    db.editable = true;
    app.databases.insert(db.path.clone(), db);
    app.open_tab(TabKind::Database {
        path: "fixture.sqlite".into(),
    });
}
