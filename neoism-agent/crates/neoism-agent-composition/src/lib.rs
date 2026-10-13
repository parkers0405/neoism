//! A deliberately bounded JavaScript subset for composing explicitly catalogued tools.
//! No JavaScript engine, ambient authority, or process-wide interpreter state is used.
mod lower;
mod members;
mod preflight;
mod runtime;

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{atomic::AtomicBool, Arc};
use std::time::Duration;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolDescriptor {
    /// Unprefixed path, for example `neoism.read`.
    pub path: String,
    pub description: String,
    pub signature: String,
}

/// An opt-in, in-process alias to an existing nested result value. Paths are
/// property/index segments, not expressions, and must not contain prototype keys.
#[derive(Clone, Copy, Debug)]
pub struct ResultAlias {
    pub name: &'static str,
    pub path: &'static [&'static str],
}

pub trait ToolHost: Send + Sync {
    /// The empty default does no alias allocation or extra import work.
    fn result_aliases(&self, _tool: &str) -> &'static [ResultAlias] {
        &[]
    }
    fn catalog(&self) -> Vec<ToolDescriptor>;
    fn call(
        &self,
        path: String,
        input: Value,
        cancel: Arc<AtomicBool>,
    ) -> BoxFuture<'static, Result<Value, String>>;
}

#[derive(Clone, Debug)]
pub struct Limits {
    pub wall_clock: Duration,
    pub max_steps: u64,
    pub max_calls: usize,
    pub concurrency: usize,
    pub max_source_bytes: usize,
    pub max_output_bytes: usize,
    pub max_log_bytes: usize,
    /// Cumulative logical allocation, not just currently live values.
    pub max_data_bytes: usize,
    pub max_depth: usize,
    pub cleanup_timeout: Duration,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            wall_clock: Duration::from_secs(120),
            max_steps: 100_000,
            max_calls: 64,
            concurrency: 8,
            max_source_bytes: 64 * 1024,
            max_output_bytes: 64 * 1024,
            max_log_bytes: 16 * 1024,
            max_data_bytes: 16 * 1024 * 1024,
            max_depth: 64,
            cleanup_timeout: Duration::from_millis(250),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Diagnostic {
    pub kind: String,
    pub message: String,
    pub line: Option<usize>,
    pub column: Option<usize>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CallRecord {
    pub id: String,
    pub path: String,
    pub input: Value,
    pub status: String,
    pub error: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExecutionResult {
    pub ok: bool,
    pub value: Value,
    pub error: Option<Diagnostic>,
    pub logs: Vec<String>,
    pub tool_calls: Vec<CallRecord>,
    pub truncated: bool,
    pub warnings: Vec<String>,
}

pub async fn execute(
    code: &str,
    host: Arc<dyn ToolHost>,
    limits: Limits,
    cancel: Arc<AtomicBool>,
) -> ExecutionResult {
    runtime::execute(code, host, limits, cancel).await
}
