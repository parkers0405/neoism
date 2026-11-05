//! Workspace-hosted SQLite browser. Enums use serde's external wire tagging.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "lowercase")]
pub enum DatabaseValue { Null, Integer(String), Real(f64), Text(String), Blob(String) }
pub type DatabaseRecord = BTreeMap<String, DatabaseValue>;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseFilter { pub column: String, pub values: Vec<DatabaseValue> }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseSort { pub column: String, pub descending: bool }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DatabaseChangeKind { Insert, Update, Delete }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseChange {
    pub kind: DatabaseChangeKind,
    pub values: DatabaseRecord,
    pub identity: Option<DatabaseRecord>,
    pub original: Option<DatabaseRecord>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DatabaseRequest {
    Schema { path: String },
    Browse { path: String, table: String, limit: u32, offset: u32, sort: Option<DatabaseSort>, #[serde(default)] filters: Vec<DatabaseFilter> },
    Values { path: String, table: String, column: String, #[serde(default)] filters: Vec<DatabaseFilter> },
    Apply { path: String, table: String, changes: Vec<DatabaseChange> },
}
impl DatabaseRequest {
    pub fn path(&self) -> &str { match self { Self::Schema { path } | Self::Browse { path, .. } | Self::Values { path, .. } | Self::Apply { path, .. } => path } }
    pub fn is_mutation(&self) -> bool { matches!(self, Self::Apply { .. }) }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseColumn { pub name: String, pub data_type: String, pub not_null: bool, pub default_value: Option<String>, pub primary_key: u32 }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseIndex { pub name: String, pub unique: bool, pub columns: Vec<String> }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseTrigger { pub name: String, pub sql: String }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DatabaseObjectKind { Table, View }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseObject { pub name: String, pub kind: DatabaseObjectKind, pub sql: String, pub columns: Vec<DatabaseColumn>, pub indexes: Vec<DatabaseIndex>, pub triggers: Vec<DatabaseTrigger> }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseValueCount { pub value: DatabaseValue, pub count: u64 }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DatabaseReply {
    Schema { objects: Vec<DatabaseObject> },
    Rows { columns: Vec<String>, rows: Vec<Vec<DatabaseValue>>, identities: Vec<Option<DatabaseRecord>>, total: u64, editable: bool },
    Values { values: Vec<DatabaseValueCount>, truncated: bool },
    Applied { affected: u64 },
    Error { message: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn external_request_reply_tags_and_precision() {
        let request: DatabaseRequest = serde_json::from_value(json!({"Browse": {
            "path":"db.sqlite", "table":"items", "limit":200, "offset":0, "filters":[]
        }})).unwrap();
        assert!(!request.is_mutation());
        assert_eq!(request.path(), "db.sqlite");
        assert!(serde_json::to_value(request).unwrap().get("Browse").is_some());
        let value = DatabaseValue::Integer("9223372036854775807".into());
        assert_eq!(serde_json::to_value(&value).unwrap(), json!({"kind":"integer","value":"9223372036854775807"}));
        assert_eq!(serde_json::to_value(DatabaseValue::Null).unwrap(), json!({"kind":"null"}));
        assert_eq!(serde_json::to_value(DatabaseReply::Applied { affected: 1 }).unwrap(), json!({"Applied":{"affected":1}}));
    }
}