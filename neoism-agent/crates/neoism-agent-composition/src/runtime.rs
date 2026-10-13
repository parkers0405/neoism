use crate::lower::{self, blocked, Expr, Function, Stmt, E, S};
use crate::members::{array_method, builtin_member, string_method};
use crate::*;
// Enabled by the pinned parser's oxc_ecmascript -> oxc_syntax dependency.
use oxc_syntax::number::ToJsString;
use futures::{
    future::{BoxFuture, Shared},
    FutureExt,
};
use std::{
    collections::BTreeMap,
    sync::{atomic::Ordering, Mutex},
    time::Instant,
};
use tokio::{
    sync::{oneshot, Semaphore},
    task::JoinHandle,
};

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;

const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;
type R<T> = Result<T, Fault>;
#[derive(Clone, Debug)]
struct Fault {
    kind: &'static str,
    message: String,
    at: Option<usize>,
    fatal: bool,
}
fn fault(
    kind: &'static str,
    message: impl Into<String>,
    at: usize,
    fatal: bool,
) -> Fault {
    Fault {
        kind,
        message: message.into(),
        at: (at != usize::MAX).then_some(at),
        fatal,
    }
}
#[derive(Clone, Debug)]
enum V {
    Undefined,
    Null,
    Bool(bool),
    Num(f64),
    Str(Arc<String>),
    Array(usize),
    Object(usize),
    Function(Arc<Function>, usize, usize),
    Builtin(String),
    Method(Box<V>, String),
    ToolNamespace(String),
    Tool(String),
    Promise(usize),
}
impl V {
    fn truthy(&self) -> bool {
        !matches!(self, Self::Undefined | Self::Null | Self::Bool(false))
            && !matches!(self, Self::Num(n) if *n == 0.0 || n.is_nan())
            && !matches!(self, Self::Str(s) if s.is_empty())
    }
    fn text(&self) -> R<String> {
        Ok(match self {
            Self::Undefined => "undefined".into(),
            Self::Null => "null".into(),
            Self::Bool(b) => b.to_string(),
            Self::Num(n) if *n == 0.0 => "0".into(),
            Self::Num(n) if n.is_nan() => "NaN".into(),
            Self::Num(n) if *n == f64::INFINITY => "Infinity".into(),
            Self::Num(n) if *n == f64::NEG_INFINITY => "-Infinity".into(),
            Self::Num(n) => n.to_js_string(),
            Self::Str(s) => s.as_ref().clone(),
            _ => {
                return Err(fault(
                    "type",
                    "Expected primitive string conversion",
                    usize::MAX,
                    false,
                ))
            }
        })
    }
    fn number(&self) -> R<f64> {
        match self {
            Self::Num(n) => Ok(*n),
            Self::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
            Self::Null => Ok(0.0),
            Self::Str(s) => s
                .trim()
                .parse()
                .map_err(|_| fault("type", "Invalid number", usize::MAX, false)),
            _ => Err(fault("type", "Expected number", usize::MAX, false)),
        }
    }
}
enum Reference {
    Binding(usize, String),
    Array(usize, usize),
    Object(usize, String),
}
enum Mutation {
    Push(usize, Vec<V>),
    Array(usize, usize, V),
    Object(usize, String, V),
}
struct Env {
    parent: Option<usize>,
    vars: BTreeMap<String, (V, bool)>,
}
struct State {
    fatal: Option<Fault>,
    function_ids: usize,
    arrays: Vec<Vec<V>>,
    caught_errors: std::collections::BTreeSet<usize>,
    objects: Vec<BTreeMap<String, V>>,
    envs: Vec<Env>,
    steps: u64,
    yield_at: u64,
    allocated: usize,
    logs: Vec<String>,
    log_bytes: usize,
    records: Vec<CallRecord>,
    promises: Vec<Shared<BoxFuture<'static, R<V>>>>,
    tasks: Vec<JoinHandle<()>>,
    aborts: Vec<tokio::task::AbortHandle>,
    truncated: bool,
}
// Also protects cancellation by dropping the execute future (e.g. HTTP disconnect).
// Abort handles remain registered even while cleanup has taken ownership of joins.
struct ExecutionGuard {
    state: Arc<Mutex<State>>,
    cancel: Arc<AtomicBool>,
}
impl Drop for ExecutionGuard {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        let s = self.state.lock().unwrap();
        for handle in &s.aborts {
            handle.abort();
        }
    }
}
#[derive(Clone)]
struct Engine {
    source: Arc<str>,
    state: Arc<Mutex<State>>,
    host: Arc<dyn ToolHost>,
    catalog: Arc<Vec<ToolDescriptor>>,
    limits: Arc<Limits>,
    local: Arc<AtomicBool>,
    external: Arc<AtomicBool>,
    deadline: Instant,
    semaphore: Arc<Semaphore>,
}
#[derive(Clone)]
enum Flow {
    Next(V),
    Return(V),
    Break,
    Continue,
}
impl Engine {
    fn diagnostic(&self, error: &Fault) -> Diagnostic {
        match error.at {
            Some(at) => {
                lower::diagnostic(&self.source, at, error.kind, error.message.clone())
            }
            None => Diagnostic {
                kind: error.kind.into(),
                message: error.message.clone(),
                line: None,
                column: None,
            },
        }
    }
    async fn checkpoint(&self, at: usize, depth: usize) -> R<()> {
        self.step(at, depth)?;
        let yield_now = {
            let mut s = self.state.lock().unwrap();
            if s.steps >= s.yield_at {
                s.yield_at = s.steps.saturating_add(128);
                true
            } else {
                false
            }
        };
        if yield_now {
            tokio::task::yield_now().await;
            self.step(at, depth)?;
        }
        Ok(())
    }
    fn step(&self, at: usize, depth: usize) -> R<()> {
        if self.local.load(Ordering::Relaxed) || self.external.load(Ordering::Relaxed) {
            self.local.store(true, Ordering::Relaxed);
            return Err(self
                .state
                .lock()
                .unwrap()
                .fatal
                .clone()
                .unwrap_or_else(|| fault("cancelled", "Execution cancelled", at, true)));
        }
        if Instant::now() >= self.deadline {
            self.local.store(true, Ordering::Relaxed);
            return Err(fault(
                "timeout",
                "Execution wall-clock limit exceeded",
                at,
                true,
            ));
        }
        let mut s = self.state.lock().unwrap();
        s.steps = s.steps.saturating_add(1);
        if depth >= self.limits.max_depth.min(64) || s.steps > self.limits.max_steps {
            let error = fault("limit","Evaluation depth/step limit exceeded",at,true);
            if s.fatal.is_none(){s.fatal=Some(error.clone());}
            self.local.store(true,Ordering::Relaxed);
            return Err(error);
        }
        Ok(())
    }
    fn reserve(&self, bytes: usize) -> R<()> {
        let mut s = self.state.lock().unwrap();
        self.admit(&mut s, bytes)
    }
    fn admit(&self, s: &mut State, bytes: usize) -> R<()> {
        let next = s.allocated.checked_add(bytes);
        let error = match next {
            None => Some(fault("limit","Data allocation overflow",usize::MAX,true)),
            Some(next) if next > self.limits.max_data_bytes => Some(fault("limit","Aggregate data allocation limit exceeded",usize::MAX,true)),
            _ => None,
        };
        if let Some(error) = error {
            if s.fatal.is_none(){s.fatal=Some(error.clone());}
            self.local.store(true,Ordering::Relaxed);
            return Err(error);
        }
        s.allocated=next.expect("admitted allocation has a size");
        Ok(())
    }
    fn text(&self, v: &V) -> R<String> {
        if let V::Object(id) = v {
            let mut state = self.state.lock().unwrap();
            if !state.caught_errors.contains(id) {
                self.admit(&mut state, 64)?;
                return Ok("[object Object]".into());
            }
            let primitive = |value: Option<&V>| -> R<V> {
                let value = value.unwrap_or(&V::Undefined);
                if !matches!(
                    value,
                    V::Undefined | V::Null | V::Bool(_) | V::Num(_) | V::Str(_)
                ) {
                    return Err(fault(
                        "type",
                        "Caught-error kind/message coercion requires primitive fields",
                        usize::MAX,
                        false,
                    ));
                }
                Ok(value.clone())
            };
            let kind = primitive(state.objects[*id].get("kind"))?;
            let message = primitive(state.objects[*id].get("message"))?;
            let length = |value: &V| if let V::Str(s) = value { s.len() } else { 64 };
            let size = length(&kind)
                .saturating_add(length(&message))
                .saturating_add(2);
            self.admit(&mut state, size.saturating_mul(2).saturating_add(64))?;
            drop(state);
            let mut output = String::with_capacity(size);
            for (index, value) in [&kind, &message].into_iter().enumerate() {
                if index == 1 {
                    output.push_str(": ");
                }
                if let V::Str(text) = value {
                    output.push_str(text);
                } else {
                    output.push_str(&value.text()?);
                }
            }
            return Ok(output);
        }
        self.reserve(match v {
            V::Str(s) => s.len().saturating_add(32),
            _ => 64,
        })?;
        v.text()
    }
    fn string(&self, s: String) -> R<V> {
        self.reserve(s.len() + 32)?;
        Ok(V::Str(Arc::new(s)))
    }
    fn data_depth(&self, v: &V, depth: usize) -> R<()> {
        self.step(0, depth)?;
        match v {
            V::Array(id) => {
                for v in self.array_snapshot(*id)? {
                    self.data_depth(&v, depth + 1)?;
                }
            }
            V::Object(id) => {
                for v in self.object_snapshot(*id)?.values() {
                    self.data_depth(v, depth + 1)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    fn array_snapshot(&self, id: usize) -> R<Vec<V>> {
        let mut s = self.state.lock().unwrap();
        let bytes = s.arrays[id].len().saturating_mul(64).saturating_add(32);
        self.admit(&mut s, bytes)?;
        Ok(s.arrays[id].clone())
    }
    fn object_snapshot(&self, id: usize) -> R<BTreeMap<String, V>> {
        let mut s = self.state.lock().unwrap();
        let bytes = s.objects[id]
            .len()
            .saturating_mul(128)
            .saturating_add(s.objects[id].keys().map(String::len).sum::<usize>());
        self.admit(&mut s, bytes)?;
        Ok(s.objects[id].clone())
    }
    fn array(&self, a: Vec<V>) -> R<V> {
        for v in &a {
            self.data_depth(v, 1)?;
        }
        self.reserve(a.len().saturating_mul(64) + 32)?;
        let mut s = self.state.lock().unwrap();
        let id = s.arrays.len();
        s.arrays.push(a);
        Ok(V::Array(id))
    }
    fn object(&self, o: BTreeMap<String, V>) -> R<V> {
        for v in o.values() {
            self.data_depth(v, 1)?;
        }
        self.reserve(
            o.len().saturating_mul(128) + o.keys().map(String::len).sum::<usize>(),
        )?;
        let mut s = self.state.lock().unwrap();
        let id = s.objects.len();
        s.objects.push(o);
        Ok(V::Object(id))
    }
    fn mutate(&self, mutation: Mutation) -> R<Option<usize>> {
        self.step(0, 0)?;
        self.reserve(match &mutation {
            Mutation::Object(_, key, _) => key.len().saturating_add(128),
            _ => 64,
        })?;
        let mut s = self.state.lock().unwrap();
        if let Mutation::Push(id, items) = &mutation {
            let new_len = s.arrays[*id]
                .len()
                .checked_add(items.len())
                .filter(|n| (*n as f64) <= MAX_SAFE_INTEGER)
                .ok_or_else(|| {
                    fault(
                        "limit",
                        "Array length overflow or unsafe integer length",
                        usize::MAX,
                        true,
                    )
                })?;
            let capacity_bytes = if new_len > s.arrays[*id].capacity() {
                s.arrays[*id].capacity().saturating_mul(2).max(new_len).max(4).saturating_mul(std::mem::size_of::<V>())
            } else {
                0
            };
            self.admit(
                &mut s,
                capacity_bytes.saturating_add(items.len().saturating_mul(64)),
            )?;
        }
        let mut used = 0u64;
        let mut path = [(0u8, 0usize); 64];
        let validation: R<()> = (|| {
            // Check every arena root before changing a shared node, including parents
            // currently retained only by a pending promise/callback.
            for id in 0..s.arrays.len() {
                validate_graph(
                    self,
                    &s,
                    &V::Array(id),
                    &mutation,
                    0,
                    &mut path,
                    &mut used,
                )?;
            }
            for id in 0..s.objects.len() {
                validate_graph(
                    self,
                    &s,
                    &V::Object(id),
                    &mutation,
                    0,
                    &mut path,
                    &mut used,
                )?;
            }
            Ok(())
        })();
        s.steps = s.steps.saturating_add(used);
        if let Err(error) = validation {
            if error.fatal {
                s.fatal = Some(error.clone());
                self.local.store(true, Ordering::Relaxed);
            }
            return Err(error);
        }
        match mutation {
            Mutation::Push(id, mut items) => {
                if self.external.load(Ordering::Relaxed)
                    || self.local.load(Ordering::Relaxed)
                {
                    return Err(fault(
                        "cancelled",
                        "Execution cancelled",
                        usize::MAX,
                        true,
                    ));
                }
                if Instant::now() >= self.deadline {
                    return Err(fault(
                        "timeout",
                        "Execution wall-clock limit exceeded",
                        usize::MAX,
                        true,
                    ));
                }
                let new_len = s.arrays[id].len()+items.len();
                let additional = if new_len > s.arrays[id].capacity() {s.arrays[id].capacity().saturating_mul(2).max(new_len).max(4)-s.arrays[id].len()} else {0};
                s.arrays[id].try_reserve_exact(additional).map_err(|_| {
                    fault(
                        "limit",
                        "Array capacity allocation failed",
                        usize::MAX,
                        true,
                    )
                })?;
                s.arrays[id].append(&mut items);
                return Ok(Some(s.arrays[id].len()));
            }
            Mutation::Array(id, index, value) => s.arrays[id][index] = value,
            Mutation::Object(id, key, value) => {
                s.objects[id].insert(key, value);
            }
        }
        Ok(None)
    }
    fn result_aliases(&self, value: V, aliases: &'static [ResultAlias]) -> R<V> {
        if aliases.is_empty() {
            return Ok(value);
        }
        if aliases.len() > 32 {
            return Err(fault(
                "limit",
                "Host result alias count exceeds 32",
                usize::MAX,
                true,
            ));
        }
        let V::Object(root) = value else {
            return Err(fault(
                "type",
                "Host result aliases require an object result",
                usize::MAX,
                false,
            ));
        };
        for (index, alias) in aliases.iter().enumerate() {
            self.step(usize::MAX, 0)?;
            if alias.path.len() >= self.limits.max_depth.min(64) {
                return Err(fault(
                    "limit",
                    "Host result alias path exceeds depth limit",
                    usize::MAX,
                    true,
                ));
            }
            if alias.name.is_empty()
                || blocked(alias.name)
                || alias.name.len() > 1024
                || alias
                    .path
                    .iter()
                    .any(|key| key.is_empty() || blocked(key) || key.len() > 1024)
            {
                return Err(fault(
                    "security",
                    "Forbidden or oversized host result alias name/path",
                    usize::MAX,
                    false,
                ));
            }
            if aliases[..index]
                .iter()
                .any(|previous| previous.name == alias.name)
            {
                return Err(fault(
                    "alias",
                    "Duplicate host result alias name",
                    usize::MAX,
                    false,
                ));
            }
        }
        self.reserve(aliases.len().saturating_mul(64))?;
        let mut resolved = Vec::with_capacity(aliases.len());
        for alias in aliases {
            let mut target = Some(V::Object(root));
            for key in alias.path {
                self.step(usize::MAX, 0)?;
                let state = self.state.lock().unwrap();
                target = match target {
                    Some(V::Object(id)) => state.objects[id].get(*key).cloned(),
                    Some(V::Array(id)) => key
                        .parse::<usize>()
                        .ok()
                        .and_then(|index| state.arrays[id].get(index).cloned()),
                    _ => None,
                };
                if target.is_none() {
                    break;
                }
            }
            if let Some(target) = target {
                let conflict =
                    self.state.lock().unwrap().objects[root].contains_key(alias.name);
                if conflict {
                    self.reserve(alias.name.len().saturating_add(96))?;
                    return Err(fault(
                        "alias",
                        format!(
                            "Host result alias conflicts with existing field: {}",
                            alias.name
                        ),
                        usize::MAX,
                        false,
                    ));
                }
                resolved.push((alias.name, target));
            }
        }
        for (name, target) in resolved {
            self.reserve(name.len())?;
            self.mutate(Mutation::Object(root, name.to_owned(), target))?;
        }
        Ok(V::Object(root))
    }
    fn env(&self, parent: usize) -> R<usize> {
        self.reserve(128)?;
        let mut s = self.state.lock().unwrap();
        let id = s.envs.len();
        s.envs.push(Env {
            parent: Some(parent),
            vars: BTreeMap::new(),
        });
        Ok(id)
    }
    fn iteration_env(
        &self,
        parent: usize,
        previous: usize,
        bindings: &[(String, bool, Option<Expr>)],
    ) -> R<usize> {
        let env = self.env(parent)?;
        for (name, constant, _) in bindings {
            if !constant {
                self.bind(env, name, self.get(previous, name)?, false)?;
            }
        }
        Ok(env)
    }
    fn bind(&self, env: usize, name: &str, value: V, constant: bool) -> R<()> {
        self.reserve(name.len() + 96)?;
        let mut s = self.state.lock().unwrap();
        if s.envs[env].vars.contains_key(name) {
            return Err(fault(
                "binding",
                format!("Duplicate binding: {name}"),
                usize::MAX,
                false,
            ));
        }
        s.envs[env].vars.insert(name.into(), (value, constant));
        Ok(())
    }
    fn get(&self, mut env: usize, name: &str) -> R<V> {
        match name {
            "undefined" => return Ok(V::Undefined),
            "tools" => return Ok(V::ToolNamespace(String::new())),
            "search" | "JSON" | "Object" | "String" | "Number" | "Math" | "console"
            | "Promise" => return Ok(V::Builtin(name.into())),
            _ => {}
        }
        let s = self.state.lock().unwrap();
        loop {
            if let Some((v, _)) = s.envs[env].vars.get(name) {
                return Ok(v.clone());
            }
            match s.envs[env].parent {
                Some(p) => env = p,
                None => break,
            }
        }
        Err(fault(
            "name",
            format!("Unknown identifier: {name}"),
            usize::MAX,
            false,
        ))
    }
    fn set(&self, mut env: usize, name: &str, value: V) -> R<()> {
        let mut s = self.state.lock().unwrap();
        loop {
            if let Some((v, constant)) = s.envs[env].vars.get_mut(name) {
                if *constant {
                    return Err(fault(
                        "binding",
                        format!("Cannot assign const {name}"),
                        usize::MAX,
                        false,
                    ));
                }
                *v = value;
                return Ok(());
            }
            match s.envs[env].parent {
                Some(p) => env = p,
                None => break,
            }
        }
        Err(fault(
            "name",
            format!("Assignment to unknown binding: {name}"),
            usize::MAX,
            false,
        ))
    }
    fn charge_json_clone(&self, j: &Value, depth: usize) -> R<()> {
        self.step(0, depth)?;
        self.reserve(64)?;
        match j {
            Value::String(s) => self.reserve(s.len())?,
            Value::Array(a) => {
                self.reserve(a.len().saturating_mul(64))?;
                for v in a {
                    self.charge_json_clone(v, depth + 1)?;
                }
            }
            Value::Object(o) => {
                self.reserve(o.len().saturating_mul(128))?;
                for (k, v) in o {
                    self.reserve(k.len())?;
                    self.charge_json_clone(v, depth + 1)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    fn from_json(&self, j: &Value, depth: usize) -> R<V> {
        self.step(0, depth)?;
        Ok(match j {
            Value::Null => V::Null,
            Value::Bool(b) => V::Bool(*b),
            Value::Number(n) => {
                let f = n.as_f64().ok_or_else(|| {
                    fault("type", "JSON number outside finite range", 0, false)
                })?;
                if !f.is_finite() || (f.fract() == 0.0 && f.abs() > MAX_SAFE_INTEGER) {
                    return Err(fault("type","Unsafe integer JSON value exceeds JavaScript Number's exact range",0,false));
                }
                V::Num(f)
            }
            Value::String(s) => {
                self.reserve(s.len() + 32)?;
                V::Str(Arc::new(s.clone()))
            }
            Value::Array(a) => {
                self.reserve(a.len().saturating_mul(64))?;
                let mut out = Vec::with_capacity(a.len());
                for j in a {
                    out.push(self.from_json(j, depth + 1)?);
                }
                self.array(out)?
            }
            Value::Object(o) => {
                self.reserve(o.len().saturating_mul(128))?;
                let mut out = BTreeMap::new();
                for (k, j) in o {
                    if blocked(k) {
                        return Err(fault(
                            "security",
                            "Prototype keys are forbidden",
                            usize::MAX,
                            false,
                        ));
                    }
                    self.reserve(k.len())?;
                    out.insert(k.clone(), self.from_json(j, depth + 1)?);
                }
                self.object(out)?
            }
        })
    }
    fn json(&self, v: &V, depth: usize) -> R<Value> {
        self.json_mode(v, depth, false)
    }
    fn json_mode(&self, v: &V, depth: usize, stringify: bool) -> R<Value> {
        self.step(0, depth)?;
        Ok(match v {
            V::Undefined if stringify => Value::Null,
            V::Undefined => {
                return Err(fault(
                    "type",
                    "Undefined is not JSON data; omit optional properties explicitly",
                    0,
                    false,
                ))
            }
            V::Null => Value::Null,
            V::Bool(b) => (*b).into(),
            V::Num(n) if n.is_finite() && n.fract() == 0.0 => {
                if n.abs() > MAX_SAFE_INTEGER {
                    return Err(fault("type","Unsafe integer JSON value exceeds JavaScript Number's exact range",0,false));
                }
                if *n >= 0.0 {
                    Value::from(*n as u64)
                } else {
                    Value::from(*n as i64)
                }
            }
            V::Num(n) => serde_json::Number::from_f64(*n)
                .ok_or_else(|| {
                    fault(
                        "type",
                        "Nonfinite JSON output is forbidden",
                        usize::MAX,
                        false,
                    )
                })?
                .into(),
            V::Str(s) => {
                self.reserve(s.len())?;
                s.as_ref().clone().into()
            }
            V::Array(id) => {
                let a = self.array_snapshot(*id)?;
                self.reserve(a.len().saturating_mul(64))?;
                let mut out = Vec::with_capacity(a.len());
                for v in a.iter() {
                    out.push(self.json_mode(v, depth + 1, stringify)?);
                }
                Value::Array(out)
            }
            V::Object(id) => {
                let o = self.object_snapshot(*id)?;
                self.reserve(o.len().saturating_mul(128))?;
                let mut out = serde_json::Map::new();
                for (k, v) in o.iter() {
                    if stringify && matches!(v, V::Undefined) {
                        continue;
                    }
                    self.reserve(k.len())?;
                    out.insert(k.clone(), self.json_mode(v, depth + 1, stringify)?);
                }
                Value::Object(out)
            }
            _ => {
                return Err(fault(
                    "type",
                    "Functions, capabilities and unawaited promises cannot be serialized",
                    usize::MAX,
                    false,
                ))
            }
        })
    }
    fn member(&self, value: V, key: String) -> R<V> {
        if blocked(&key) {
            return Err(fault(
                "security",
                "Prototype access is forbidden",
                usize::MAX,
                false,
            ));
        }
        match value {
            V::ToolNamespace(prefix) => {
                let path = if prefix.is_empty() {
                    key
                } else {
                    format!("{prefix}.{key}")
                };
                if self.catalog.iter().any(|d| d.path == path) {
                    Ok(V::Tool(path))
                } else if self
                    .catalog
                    .iter()
                    .any(|d| d.path.starts_with(&format!("{path}.")))
                {
                    Ok(V::ToolNamespace(path))
                } else {
                    Err(fault(
                        "tool",
                        format!("Tool is not in catalog: tools.{path}"),
                        usize::MAX,
                        false,
                    ))
                }
            }
            V::Object(id) => Ok(self.state.lock().unwrap().objects[id]
                .get(&key)
                .cloned()
                .unwrap_or(V::Undefined)),
            V::Array(id) if key == "length" => {
                Ok(V::Num(self.state.lock().unwrap().arrays[id].len() as f64))
            }
            V::Str(s) if key == "length" => Ok(V::Num(s.chars().count() as f64)),
            V::Array(id) if key.parse::<usize>().is_ok() => {
                Ok(self.state.lock().unwrap().arrays[id]
                    .get(key.parse::<usize>().unwrap())
                    .cloned()
                    .unwrap_or(V::Undefined))
            }
            V::Str(s) if key.parse::<usize>().is_ok() => {
                match s.chars().nth(key.parse::<usize>().unwrap()) {
                    Some(c) => self.string(c.to_string()),
                    None => Ok(V::Undefined),
                }
            }
            V::Array(_) if array_method(&key) => Ok(V::Method(Box::new(value), key)),
            V::Str(_) if string_method(&key) => Ok(V::Method(Box::new(value), key)),
            V::Builtin(ref b) if builtin_member(b, &key) => {
                Ok(V::Builtin(format!("{b}.{key}")))
            }
            _ => Err(fault(
                "unsupported",
                format!("Unsupported member operation: {key}"),
                usize::MAX,
                false,
            )),
        }
    }
    fn spawn<F>(&self, future: F) -> R<V>
    where
        F: std::future::Future<Output = R<V>> + Send + 'static,
    {
        self.reserve(512)?;
        let (tx, rx) = oneshot::channel();
        let promise = async move {
            rx.await.unwrap_or_else(|_| {
                Err(fault("cancelled", "Promise interrupted", usize::MAX, true))
            })
        }
        .boxed()
        .shared();
        let mut s = self.state.lock().unwrap();
        if s.promises.len() >= 4096 {
            let error = fault("limit", "Promise count limit exceeded", usize::MAX, true);
            s.fatal = Some(error.clone());
            self.local.store(true, Ordering::Relaxed);
            return Err(error);
        }
        let id = s.promises.len();
        s.promises.push(promise);
        let state = self.state.clone();
        let cancel = self.local.clone();
        let task = tokio::spawn(async move {
            let result = future.await;
            if let Err(error) = &result {
                if error.fatal {
                    let mut s = state.lock().unwrap();
                    if s.fatal.is_none() {
                        s.fatal = Some(error.clone());
                    }
                    cancel.store(true, Ordering::Relaxed);
                }
            }
            let _ = tx.send(result);
        });
        s.aborts.push(task.abort_handle());
        s.tasks.push(task);
        Ok(V::Promise(id))
    }
    fn resolve(&self, value: V, depth: usize) -> BoxFuture<'_, R<V>> {
        async move {
            self.step(0, depth)?;
            if let V::Promise(id) = value {
                let promise = self.state.lock().unwrap().promises[id].clone();
                let v = self.wait(promise).await?;
                self.resolve(v, depth + 1).await
            } else {
                Ok(value)
            }
        }
        .boxed()
    }
    async fn wait<T>(&self, future: impl std::future::Future<Output = R<T>>) -> R<T> {
        tokio::pin!(future);
        loop {
            self.step(0, 0)?;
            tokio::select! { result = &mut future => return result, _ = tokio::time::sleep(Duration::from_millis(5)) => {} }
        }
    }
    fn eval<'a>(&'a self, e: &'a Expr, env: usize, depth: usize) -> BoxFuture<'a, R<V>> {
        async move {
            self.checkpoint(e.at, depth).await?;
            let result: R<V> = async {
                Ok(match &e.kind {
                    E::Literal(j) => self.from_json(j, depth + 1)?,
                    E::Name(n) => self.get(env, n)?,
                    E::Array(a) => {
                        self.reserve(a.len().saturating_mul(64))?;
                        let mut out = Vec::with_capacity(a.len());
                        for e in a {
                            out.push(self.eval(e, env, depth + 1).await?);
                        }
                        self.array(out)?
                    }
                    E::Object(o) => {
                        self.reserve(o.len().saturating_mul(128))?;
                        let mut out = BTreeMap::new();
                        for (k, e) in o {
                            self.reserve(k.len())?;
                            out.insert(k.clone(), self.eval(e, env, depth + 1).await?);
                        }
                        self.object(out)?
                    }
                    E::Member(a, k) => {
                        let a = self.eval(a, env, depth + 1).await?;
                        let key = self.text(&self.eval(k, env, depth + 1).await?)?;
                        self.member(a, key)?
                    }
                    E::Call(f, args) => {
                        let f = self.eval(f, env, depth + 1).await?;
                        self.reserve(args.len().saturating_mul(64))?;
                        let mut out = Vec::with_capacity(args.len());
                        for e in args {
                            out.push(self.eval(e, env, depth + 1).await?);
                        }
                        self.call(f, out, depth + 1).await?
                    }
                    E::Await(e) => {
                        let v = self.eval(e, env, depth + 1).await?;
                        self.resolve(v, depth + 1).await?
                    }
                    E::Unary(op, e) => {
                        let v = self.eval(e, env, depth + 1).await?;
                        match op.as_str() {
                            "!" => V::Bool(!v.truthy()),
                            "+" => V::Num(v.number()?),
                            "-" => V::Num(-v.number()?),
                            "void" => V::Undefined,
                            "typeof" => self.string(
                                match v {
                                    V::Undefined => "undefined",
                                    V::Bool(_) => "boolean",
                                    V::Num(_) => "number",
                                    V::Str(_) => "string",
                                    V::Function(..) | V::Builtin(_) | V::Tool(_) => {
                                        "function"
                                    }
                                    _ => "object",
                                }
                                .into(),
                            )?,
                            _ => unreachable!(),
                        }
                    }
                    E::Binary(op, a, b) => {
                        let a = self.eval(a, env, depth + 1).await?;
                        if (op == "&&" && !a.truthy())
                            || (op == "||" && a.truthy())
                            || (op == "??" && !matches!(a, V::Undefined | V::Null))
                        {
                            a
                        } else {
                            let b = self.eval(b, env, depth + 1).await?;
                            if matches!(op.as_str(), "&&" | "||" | "??") {
                                b
                            } else {
                                self.binary(op, a, b)?
                            }
                        }
                    }
                    E::Conditional(t, a, b) => {
                        let t = self.eval(t, env, depth + 1).await?;
                        self.eval(if t.truthy() { a } else { b }, env, depth + 1)
                            .await?
                    }
                    E::Assign(t, op, e) => {
                        let reference = self.reference(t, env, depth + 1).await?;
                        let previous = if op != "=" {
                            Some(self.read_reference(&reference)?)
                        } else {
                            None
                        };
                        let value = self.eval(e, env, depth + 1).await?;
                        let value = if let Some(previous) = previous {
                            self.binary(&op[..op.len() - 1], previous, value)?
                        } else {
                            value
                        };
                        self.put_reference(reference, value.clone())?;
                        value
                    }
                    E::Update(t, increment, prefix) => {
                        let old = self.eval(t, env, depth + 1).await?.number()?;
                        let v = V::Num(old + if *increment { 1.0 } else { -1.0 });
                        self.assign(t, v.clone(), env, depth + 1).await?;
                        if *prefix {
                            v
                        } else {
                            V::Num(old)
                        }
                    }
                    E::Function(f) => {
                        self.reserve(64)?;
                        let mut state = self.state.lock().unwrap();
                        let id = state.function_ids;
                        state.function_ids += 1;
                        V::Function(f.clone(), env, id)
                    }
                })
            }
            .await;
            result.map_err(|mut f| {
                if f.at.is_none() {
                    f.at = Some(e.at);
                }
                f
            })
        }
        .boxed()
    }
    fn reference<'a>(
        &'a self,
        t: &'a Expr,
        env: usize,
        depth: usize,
    ) -> BoxFuture<'a, R<Reference>> {
        async move {
            self.checkpoint(t.at,depth).await?;
            match &t.kind {
                E::Name(name) => { self.reserve(name.len().saturating_add(64))?; Ok(Reference::Binding(env,name.clone())) },
                E::Member(base,key) => {
                    let container = self.eval(base,env,depth+1).await?;
                    let key = self.text(&self.eval(key,env,depth+1).await?)?;
                    if blocked(&key) {return Err(fault("security","Prototype mutation forbidden",t.at,false));}
                    match container {
                        V::Object(id) => Ok(Reference::Object(id,key)),
                        V::Array(id) => {
                            let index = key.parse::<usize>().map_err(|_|fault("unsupported","Array updates require integer indices",t.at,false))?;
                            if index >= self.state.lock().unwrap().arrays[id].len(){return Err(fault("unsupported","Sparse/growing array assignment is unsupported; use push or concat",t.at,false));}
                            Ok(Reference::Array(id,index))
                        },
                        _ => Err(fault("security","Cannot mutate capabilities, globals or non-data values",t.at,false)),
                    }
                },
                _ => Err(fault("unsupported","Invalid assignment target",t.at,false)),
            }
        }.boxed()
    }
    fn read_reference(&self, reference: &Reference) -> R<V> {
        self.step(usize::MAX, 0)?;
        match reference {
            Reference::Binding(env, name) => self.get(*env, name),
            Reference::Array(id, index) => {
                Ok(self.state.lock().unwrap().arrays[*id][*index].clone())
            }
            Reference::Object(id, key) => Ok(self.state.lock().unwrap().objects[*id]
                .get(key)
                .cloned()
                .unwrap_or(V::Undefined)),
        }
    }
    fn put_reference(&self, reference: Reference, value: V) -> R<()> {
        match reference {
            Reference::Binding(env, name) => self.set(env, &name, value),
            Reference::Array(id, index) => {
                self.mutate(Mutation::Array(id, index, value)).map(|_| ())
            }
            Reference::Object(id, key) => {
                self.mutate(Mutation::Object(id, key, value)).map(|_| ())
            }
        }
    }
    fn assign<'a>(
        &'a self,
        t: &'a Expr,
        value: V,
        env: usize,
        depth: usize,
    ) -> BoxFuture<'a, R<()>> {
        async move {
            let reference = self.reference(t, env, depth).await?;
            self.put_reference(reference, value)
        }
        .boxed()
    }
    fn binary(&self, op: &str, a: V, b: V) -> R<V> {
        self.step(0, 0)?;
        Ok(match op {
            "+" if matches!(a, V::Str(_)) || matches!(b, V::Str(_)) => {
                let a = self.text(&a)?;
                let b = self.text(&b)?;
                self.reserve(a.len().saturating_add(b.len()))?;
                self.string(a + &b)?
            }
            "+" => V::Num(a.number()? + b.number()?),
            "-" => V::Num(a.number()? - b.number()?),
            "*" => V::Num(a.number()? * b.number()?),
            "/" => V::Num(a.number()? / b.number()?),
            "%" => V::Num(a.number()? % b.number()?),
            "**" => V::Num(a.number()?.powf(b.number()?)),
            "===" | "==" => V::Bool(equal(&a, &b)),
            "!==" | "!=" => V::Bool(!equal(&a, &b)),
            "<" | "<=" | ">" | ">=" => {
                let cmp = if let (V::Str(a), V::Str(b)) = (&a, &b) {
                    a.partial_cmp(b)
                } else {
                    a.number()?.partial_cmp(&b.number()?)
                };
                V::Bool(match op {
                    "<" => cmp == Some(std::cmp::Ordering::Less),
                    ">" => cmp == Some(std::cmp::Ordering::Greater),
                    "<=" => matches!(
                        cmp,
                        Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
                    ),
                    _ => matches!(
                        cmp,
                        Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)
                    ),
                })
            }
            _ => {
                return Err(fault(
                    "unsupported",
                    format!("Unsupported operator {op}"),
                    usize::MAX,
                    false,
                ))
            }
        })
    }
    fn list<'a>(
        &'a self,
        body: &'a [Stmt],
        env: usize,
        depth: usize,
    ) -> BoxFuture<'a, R<Flow>> {
        async move {
            self.step(0, depth)?;
            let mut last = V::Undefined;
            for s in body {
                match self.stmt(s, env, depth + 1).await? {
                    Flow::Next(v) => last = v,
                    f => return Ok(f),
                }
            }
            Ok(Flow::Next(last))
        }
        .boxed()
    }
    fn stmt<'a>(
        &'a self,
        s: &'a Stmt,
        env: usize,
        depth: usize,
    ) -> BoxFuture<'a, R<Flow>> {
        async move {
            self.checkpoint(s.at, depth).await?;
            Ok(match &s.kind {
                S::Empty => Flow::Next(V::Undefined),
                S::Expr(e) => Flow::Next(self.eval(e, env, depth + 1).await?),
                S::Bind(bindings) => {
                    for (name, constant, e) in bindings {
                        let v = if let Some(e) = e {
                            self.eval(e, env, depth + 1).await?
                        } else {
                            V::Undefined
                        };
                        self.bind(env, name, v, *constant)?;
                    }
                    Flow::Next(V::Undefined)
                }
                S::Block(body) => self.list(body, self.env(env)?, depth + 1).await?,
                S::If(t, a, b) => {
                    if self.eval(t, env, depth + 1).await?.truthy() {
                        self.stmt(a, env, depth + 1).await?
                    } else if let Some(b) = b {
                        self.stmt(b, env, depth + 1).await?
                    } else {
                        Flow::Next(V::Undefined)
                    }
                }
                S::While(test, body) => {
                    loop {
                        self.step(s.at, depth)?;
                        if !self.eval(test, env, depth + 1).await?.truthy() {
                            break;
                        }
                        match self.stmt(body, env, depth + 1).await? {
                            Flow::Break => break,
                            Flow::Return(v) => return Ok(Flow::Return(v)),
                            _ => {}
                        }
                    }
                    Flow::Next(V::Undefined)
                }
                S::For(init, test, update, body) => {
                    let initial = self.env(env)?;
                    if let Some(init) = init {
                        self.stmt(init, initial, depth + 1).await?;
                    }
                    let bindings = match init.as_deref().map(|s| &s.kind) {
                        Some(S::Bind(b))
                            if b.iter().any(|(_, constant, _)| !constant) =>
                        {
                            Some(b.as_slice())
                        }
                        _ => None,
                    };
                    let mut env = if let Some(bindings) = bindings {
                        self.iteration_env(initial, initial, bindings)?
                    } else {
                        initial
                    };
                    loop {
                        self.step(s.at, depth)?;
                        if let Some(test) = test {
                            if !self.eval(test, env, depth + 1).await?.truthy() {
                                break;
                            }
                        }
                        match self.stmt(body, env, depth + 1).await? {
                            Flow::Break => break,
                            Flow::Return(v) => return Ok(Flow::Return(v)),
                            _ => {}
                        }
                        // The update runs in the NEXT environment, so closures from
                        // this body retain the pre-update binding (also on continue).
                        if let Some(bindings) = bindings {
                            env = self.iteration_env(initial, env, bindings)?;
                        }
                        if let Some(update) = update {
                            self.eval(update, env, depth + 1).await?;
                        }
                    }
                    Flow::Next(V::Undefined)
                }
                S::ForOf(name, constant, right, body) => {
                    let value = self.eval(right, env, depth + 1).await?;
                    let V::Array(a) = value else {
                        return Err(fault("type", "for-of requires array", s.at, false));
                    };
                    let a = self.array_snapshot(a)?;
                    for v in a.iter() {
                        self.step(s.at, depth)?;
                        let child = self.env(env)?;
                        self.bind(child, name, v.clone(), *constant)?;
                        match self.stmt(body, child, depth + 1).await? {
                            Flow::Break => break,
                            Flow::Return(v) => return Ok(Flow::Return(v)),
                            _ => {}
                        }
                    }
                    Flow::Next(V::Undefined)
                }
                S::Return(e) => Flow::Return(if let Some(e) = e {
                    self.eval(e, env, depth + 1).await?
                } else {
                    V::Undefined
                }),
                S::Break => Flow::Break,
                S::Continue => Flow::Continue,
                S::Throw(e) => {
                    let v = self.eval(e, env, depth + 1).await?;
                    if !matches!(v,V::Undefined|V::Null|V::Bool(_)|V::Num(_)|V::Str(_)) { return Err(fault("type","throw supports primitive values in this subset",s.at,false)); }
                    return Err(fault("throw", self.text(&v)?, s.at, false));
                }
                S::Try(body, name, catch) => {
                    match self.list(body, self.env(env)?, depth + 1).await {
                        Ok(f) => f,
                        Err(e) if !e.fatal => {
                            let child = self.env(env)?;
                            if let Some(name) = name {
                                let origin = self.diagnostic(&e);
                                let mut obj = BTreeMap::new();
                                obj.insert(
                                    "line".into(),
                                    origin.line.map_or(V::Null, |n| V::Num(n as f64)),
                                );
                                obj.insert(
                                    "column".into(),
                                    origin.column.map_or(V::Null, |n| V::Num(n as f64)),
                                );
                                obj.insert("message".into(), self.string(e.message)?);
                                obj.insert("kind".into(), self.string(e.kind.into())?);
                                let value = self.object(obj)?;
                                if let V::Object(id) = value {
                                    self.reserve(128)?;
                                    self.state.lock().unwrap().caught_errors.insert(id);
                                    self.bind(child, name, V::Object(id), true)?;
                                }
                            }
                            self.list(catch, child, depth + 1).await?
                        }
                        Err(e) => return Err(e),
                    }
                }
            })
        }
        .boxed()
    }
    fn function<'a>(
        &'a self,
        f: Arc<Function>,
        captured: usize,
        args: Vec<V>,
        depth: usize,
    ) -> BoxFuture<'a, R<V>> {
        async move {
            self.step(0, depth)?;
            let env = self.env(captured)?;
            for (i, n) in f.params.iter().enumerate() {
                self.bind(env, n, args.get(i).cloned().unwrap_or(V::Undefined), false)?;
            }
            match self.list(&f.body, env, depth + 1).await? {
                Flow::Return(v) => Ok(v),
                Flow::Next(_) => Ok(V::Undefined),
                _ => Err(fault(
                    "syntax",
                    "break/continue outside loop",
                    usize::MAX,
                    false,
                )),
            }
        }
        .boxed()
    }
    fn call(&self, f: V, args: Vec<V>, depth: usize) -> BoxFuture<'_, R<V>> {
        async move {
            self.step(0, depth)?;
            match f {
                V::Function(f, env, _) if f.asynchronous => {
                    let engine = self.clone();
                    self.spawn(async move {
                        let v = engine.function(f, env, args, depth + 1).await?;
                        engine.resolve(v, depth + 1).await
                    })
                }
                V::Function(f, env, _) => self.function(f, env, args, depth + 1).await,
                V::Tool(path) => self.tool(path, args),
                V::Builtin(name) => self.builtin(&name, args, depth + 1).await,
                V::Method(receiver, name) => {
                    self.method(*receiver, &name, args, depth + 1).await
                }
                _ => Err(fault("type", "Value is not callable", usize::MAX, false)),
            }
        }
        .boxed()
    }
    fn builtin(&self, name: &str, args: Vec<V>, depth: usize) -> BoxFuture<'_, R<V>> {
        let name = name.to_owned();
        async move {
            self.step(0, depth)?;
            let first = args.first().cloned().unwrap_or(V::Undefined);
            match name.as_str() {
                "search" => self.search(first),
                "String" => {
                    if args.is_empty() {
                        self.string(String::new())
                    } else {
                        self.string(self.text(&first)?)
                    }
                }
                "Number" => Ok(V::Num(first.number()?)),
                "JSON.parse" => {
                    let V::Str(s) = first else {
                        return Err(fault(
                            "type",
                            "JSON.parse requires a string",
                            usize::MAX,
                            false,
                        ));
                    };
                    self.reserve(s.len().saturating_mul(2).saturating_add(64))?;
                    let mut deserializer = serde_json::Deserializer::from_str(&s);
                    serde::de::DeserializeSeed::deserialize(
                        JsonSeed {
                            engine: self,
                            depth: 0,
                            key: false,
                        },
                        &mut deserializer,
                    )
                    .map_err(|e| {
                        if self.local.load(Ordering::Relaxed) {
                            self.state.lock().unwrap().fatal.clone().unwrap_or_else(
                                || fault("limit", e.to_string(), usize::MAX, true),
                            )
                        } else {
                            fault("json", e.to_string(), usize::MAX, false)
                        }
                    })?;
                    deserializer
                        .end()
                        .map_err(|e| fault("json", e.to_string(), usize::MAX, false))?;
                    drop(deserializer);
                    self.reserve(s.len().saturating_mul(2).saturating_add(64))?;
                    let j: Value = serde_json::from_str(&s)
                        .map_err(|e| fault("json", e.to_string(), usize::MAX, false))?;
                    self.from_json(&j, depth + 1)
                }
                "JSON.stringify" => {
                    if matches!(first, V::Undefined) {
                        return Ok(V::Undefined);
                    }
                    let j = self.json_mode(&first, depth + 1, true)?;
                    self.charge_json_clone(&j, depth + 1)?;
                    let bytes = bounded_json(
                        &j,
                        self.limits.max_data_bytes.min(self.limits.max_output_bytes),
                    )
                    .map_err(|_| {
                        fault("limit", "JSON string limit exceeded", usize::MAX, true)
                    })?;
                    self.string(String::from_utf8(bytes).unwrap())
                }
                "console.log" | "console.warn" | "console.error" | "console.info" => {
                    let remaining = {
                        let s = self.state.lock().unwrap();
                        self.limits.max_log_bytes.saturating_sub(s.log_bytes)
                    };
                    if remaining == 0 {
                        self.state.lock().unwrap().truncated = true;
                        return Ok(V::Undefined);
                    }
                    let mut line = String::new();
                    self.reserve(remaining.saturating_mul(3))?;
                    for (i, v) in args.iter().enumerate() {
                        if i > 0 && line.len() < remaining {
                            line.push(' ');
                        }
                        let text = match v {
                            V::Str(s) => {
                                let mut end =
                                    s.len().min(remaining.saturating_sub(line.len()));
                                while !s.is_char_boundary(end) {
                                    end -= 1;
                                }
                                if end < s.len() {
                                    self.state.lock().unwrap().truncated = true;
                                }
                                s[..end].to_owned()
                            }
                            _ => {
                                let j = self.json(v, depth + 1)?;
                                match bounded_json(
                                    &j,
                                    remaining.saturating_sub(line.len()),
                                ) {
                                    Ok(b) => String::from_utf8(b).unwrap(),
                                    Err(_) => {
                                        self.state.lock().unwrap().truncated = true;
                                        String::new()
                                    }
                                }
                            }
                        };
                        let available = remaining.saturating_sub(line.len());
                        let mut end = text.len().min(available);
                        while !text.is_char_boundary(end) {
                            end -= 1;
                        }
                        line.push_str(&text[..end]);
                        if end < text.len() {
                            self.state.lock().unwrap().truncated = true;
                        }
                    }
                    let mut s = self.state.lock().unwrap();
                    let available = self.limits.max_log_bytes.saturating_sub(s.log_bytes);
                    if clip(&mut line, available) {
                        s.truncated = true;
                    }
                    if !line.is_empty() {
                        s.log_bytes += line.len();
                        s.logs.push(line);
                    } else if available == 0 {
                        s.truncated = true;
                    }
                    Ok(V::Undefined)
                }
                "Object.keys" | "Object.values" | "Object.entries" => {
                    let V::Object(o) = first else {
                        return Err(fault(
                            "type",
                            "Object operation requires plain object",
                            usize::MAX,
                            false,
                        ));
                    };
                    let o = self.object_snapshot(o)?;
                    self.reserve(
                        o.len()
                            .saturating_mul(128)
                            .saturating_add(o.keys().map(String::len).sum::<usize>()),
                    )?;
                    let mut out = Vec::with_capacity(o.len());
                    for (k, v) in o.iter() {
                        self.step(0, depth)?;
                        out.push(match name.as_str() {
                            "Object.keys" => self.string(k.clone())?,
                            "Object.values" => v.clone(),
                            _ => self.array(vec![self.string(k.clone())?, v.clone()])?,
                        });
                    }
                    self.array(out)
                }
                "Object.fromEntries" => {
                    let V::Array(a) = first else {
                        return Err(fault(
                            "type",
                            "fromEntries requires array",
                            usize::MAX,
                            false,
                        ));
                    };
                    let a = self.array_snapshot(a)?;
                    self.reserve(a.len().saturating_mul(128))?;
                    let mut out = BTreeMap::new();
                    for pair in a.iter() {
                        self.step(0, depth)?;
                        let V::Array(pair) = pair else {
                            return Err(fault(
                                "type",
                                "Entry requires pair",
                                usize::MAX,
                                false,
                            ));
                        };
                        let pair = self.array_snapshot(*pair)?;
                        if pair.len() != 2 {
                            return Err(fault(
                                "type",
                                "Entry requires pair",
                                usize::MAX,
                                false,
                            ));
                        }
                        let key = self.text(&pair[0])?;
                        if blocked(&key) {
                            return Err(fault(
                                "security",
                                "Prototype key forbidden",
                                usize::MAX,
                                false,
                            ));
                        }
                        out.insert(key, pair[1].clone());
                    }
                    self.object(out)
                }
                "Promise.all" | "Promise.allSettled" => {
                    let V::Array(a) = first else {
                        return Err(fault(
                            "type",
                            "Promise aggregate requires array",
                            usize::MAX,
                            false,
                        ));
                    };
                    let a = self.array_snapshot(a)?;
                    let engine = self.clone();
                    let settled = name == "Promise.allSettled";
                    self.spawn(async move {
                        engine.reserve(a.len().saturating_mul(128))?;
                        if !settled {
                            let values = futures::future::try_join_all(
                                a.iter().cloned().map(|v| engine.resolve(v, depth + 1)),
                            )
                            .await?;
                            return engine.array(values);
                        }
                        let results = futures::future::join_all(
                            a.iter().cloned().map(|v| engine.resolve(v, depth + 1)),
                        )
                        .await;
                        let mut out = Vec::with_capacity(results.len());
                        for r in results {
                            if settled {
                                let mut o = BTreeMap::new();
                                match r {
                                    Ok(v) => {
                                        o.insert(
                                            "status".into(),
                                            engine.string("fulfilled".into())?,
                                        );
                                        o.insert("value".into(), v);
                                    }
                                    Err(e) if !e.fatal => {
                                        o.insert(
                                            "status".into(),
                                            engine.string("rejected".into())?,
                                        );
                                        o.insert(
                                            "reason".into(),
                                            engine.string(e.message)?,
                                        );
                                    }
                                    Err(e) => return Err(e),
                                }
                                out.push(engine.object(o)?);
                            } else {
                                out.push(r?);
                            }
                        }
                        engine.array(out)
                    })
                }
                n if n.starts_with("Math.") => {
                    let op = &n[5..];
                    let a = first.number()?;
                    let b = args.get(1).unwrap_or(&V::Undefined);
                    let n = match op {
                        "abs" => a.abs(),
                        "floor" => a.floor(),
                        "ceil" => a.ceil(),
                        "round" => (a + 0.5).floor(),
                        "trunc" => a.trunc(),
                        "sqrt" => a.sqrt(),
                        "pow" => a.powf(b.number()?),
                        "min" | "max" => {
                            let mut n = a;
                            for v in args.iter().skip(1) {
                                self.step(0, depth)?;
                                n = if op == "min" {
                                    n.min(v.number()?)
                                } else {
                                    n.max(v.number()?)
                                };
                            }
                            n
                        }
                        _ => {
                            return Err(fault(
                                "unsupported",
                                "Unsupported Math operation",
                                usize::MAX,
                                false,
                            ))
                        }
                    };
                    Ok(V::Num(n))
                }
                "Number.isFinite" => {
                    Ok(V::Bool(matches!(first,V::Num(n) if n.is_finite())))
                }
                "Number.isInteger" => Ok(V::Bool(
                    matches!(first,V::Num(n) if n.is_finite() && n.fract()==0.0),
                )),
                _ => Err(fault(
                    "unsupported",
                    format!("Unsupported builtin: {name}"),
                    usize::MAX,
                    false,
                )),
            }
        }
        .boxed()
    }
    fn search(&self, options: V) -> R<V> {
        let V::Object(o) = options else {
            return Err(fault(
                "type",
                "search expects {query, namespace, limit, offset}",
                usize::MAX,
                false,
            ));
        };
        let o = self.object_snapshot(o)?;
        self.reserve(
            o.values()
                .filter_map(|v| {
                    if let V::Str(s) = v {
                        Some(s.len().saturating_mul(3))
                    } else {
                        None
                    }
                })
                .sum(),
        )?;
        let query = match o.get("query") {
            Some(V::Str(s)) => s.to_lowercase(),
            None => String::new(),
            _ => return Err(fault("type", "query must be string", usize::MAX, false)),
        };
        let namespace = match o.get("namespace") {
            Some(V::Str(s)) => s.as_ref().clone(),
            None => String::new(),
            _ => {
                return Err(fault("type", "namespace must be string", usize::MAX, false))
            }
        };
        let limit = integer_option(o.get("limit"), 20)?.min(100);
        let offset = integer_option(o.get("offset"), 0)?;
        let mut matches = Vec::new();
        for d in self.catalog.iter() {
            self.step(0, 0)?;
            self.reserve(
                d.path
                    .len()
                    .saturating_add(d.description.len())
                    .saturating_add(d.signature.len())
                    .saturating_add(namespace.len())
                    .saturating_mul(3)
                    .saturating_add(64),
            )?;
            if (namespace.is_empty() || d.path.starts_with(&format!("{namespace}.")))
                && (query.is_empty()
                    || d.path.to_lowercase().contains(&query)
                    || d.description.to_lowercase().contains(&query))
            {
                matches.push(d);
            }
        }
        let total = matches.len();
        let mut items = Vec::new();
        for d in matches.into_iter().skip(offset).take(limit) {
            let mut item = BTreeMap::new();
            item.insert("path".into(), self.string(format!("tools.{}", d.path))?);
            item.insert("description".into(), self.string(d.description.clone())?);
            item.insert("signature".into(), self.string(d.signature.clone())?);
            items.push(self.object(item)?);
        }
        let next = offset.saturating_add(items.len());
        let mut out = BTreeMap::new();
        out.insert("tools".into(), self.array(items)?);
        out.insert("total".into(), V::Num(total as f64));
        out.insert("offset".into(), V::Num(offset as f64));
        out.insert("limit".into(), V::Num(limit as f64));
        out.insert(
            "nextOffset".into(),
            if next < total {
                V::Num(next as f64)
            } else {
                V::Null
            },
        );
        self.object(out)
    }
    async fn string_index_of(
        &self,
        haystack: &str,
        needle: &str,
        from: &V,
        depth: usize,
    ) -> R<V> {
        let position = match from {
            V::Undefined | V::Null => 0.0,
            V::Bool(value) => {
                if *value {
                    1.0
                } else {
                    0.0
                }
            }
            V::Num(value) => *value,
            V::Str(value) => {
                if value.trim().is_empty() {
                    0.0
                } else {
                    value.trim().parse::<f64>().unwrap_or(f64::NAN)
                }
            }
            _ => {
                return Err(fault(
                    "type",
                    "indexOf fromIndex must be a primitive",
                    usize::MAX,
                    false,
                ))
            }
        };
        let from = if position.is_nan() || position <= 0.0 {
            0
        } else {
            position.trunc() as usize
        };
        if needle.is_empty() {
            let mut index = 0usize;
            for _ in haystack.chars() {
                self.checkpoint(usize::MAX, depth).await?;
                if index == from {
                    break;
                }
                index += 1;
            }
            return Ok(V::Num(index as f64));
        }
        let mut count = 0usize;
        for _ in needle.chars() {
            self.checkpoint(usize::MAX, depth).await?;
            count += 1;
        }
        self.reserve(
            count
                .saturating_mul(
                    std::mem::size_of::<char>() + std::mem::size_of::<usize>(),
                )
                .saturating_add(64),
        )?;
        let mut pattern = Vec::with_capacity(count);
        for ch in needle.chars() {
            self.checkpoint(usize::MAX, depth).await?;
            pattern.push(ch);
        }
        let mut prefix = Vec::with_capacity(count);
        prefix.push(0usize);
        for i in 1..count {
            self.checkpoint(usize::MAX, depth).await?;
            let mut matched = prefix[i - 1];
            while matched > 0 && pattern[i] != pattern[matched] {
                self.checkpoint(usize::MAX, depth).await?;
                matched = prefix[matched - 1];
            }
            if pattern[i] == pattern[matched] {
                matched += 1;
            }
            prefix.push(matched);
        }
        let mut matched = 0usize;
        for (index, ch) in haystack.chars().enumerate() {
            self.checkpoint(usize::MAX, depth).await?;
            if index < from {
                continue;
            }
            while matched > 0 && ch != pattern[matched] {
                self.checkpoint(usize::MAX, depth).await?;
                matched = prefix[matched - 1];
            }
            if ch == pattern[matched] {
                matched += 1;
            }
            if matched == count {
                return Ok(V::Num((index + 1 - count) as f64));
            }
        }
        Ok(V::Num(-1.0))
    }
    fn method(
        &self,
        receiver: V,
        name: &str,
        args: Vec<V>,
        depth: usize,
    ) -> BoxFuture<'_, R<V>> {
        let name = name.to_owned();
        async move {
            self.step(0, depth)?;
            let first = args.first().cloned().unwrap_or(V::Undefined);
            match receiver {
                V::Array(array_id) => {
                    if name == "push" {
                        if args.is_empty() {
                            return Ok(V::Num(
                                self.state.lock().unwrap().arrays[array_id].len() as f64,
                            ));
                        }
                        let length = self
                            .mutate(Mutation::Push(array_id, args))?
                            .expect("push returns length");
                        return Ok(V::Num(length as f64));
                    }
                    let a = self.array_snapshot(array_id)?;
                    self.reserve(a.len().saturating_mul(64))?;
                    match name.as_str() {
                        "map" | "filter" | "flatMap" | "find" | "findIndex" | "some"
                        | "every" => {
                            let mut out = Vec::new();
                            for (i, v) in a.iter().enumerate() {
                                self.step(0, depth)?;
                                let r = self
                                    .call(
                                        first.clone(),
                                        vec![
                                            v.clone(),
                                            V::Num(i as f64),
                                            V::Array(array_id),
                                        ],
                                        depth + 1,
                                    )
                                    .await?;
                                match name.as_str() {
                                    "map" => out.push(r),
                                    "filter" => {
                                        if r.truthy() {
                                            out.push(v.clone());
                                        }
                                    }
                                    "flatMap" => {
                                        if let V::Array(id) = r {
                                            let b = self.array_snapshot(id)?;
                                            self.reserve(b.len().saturating_mul(64))?;
                                            out.extend(b.iter().cloned());
                                        } else {
                                            out.push(r);
                                        }
                                    }
                                    "find" if r.truthy() => return Ok(v.clone()),
                                    "findIndex" if r.truthy() => {
                                        return Ok(V::Num(i as f64))
                                    }
                                    "some" if r.truthy() => return Ok(V::Bool(true)),
                                    "every" if !r.truthy() => return Ok(V::Bool(false)),
                                    _ => {}
                                }
                            }
                            match name.as_str() {
                                "find" => Ok(V::Undefined),
                                "findIndex" => Ok(V::Num(-1.0)),
                                "some" => Ok(V::Bool(false)),
                                "every" => Ok(V::Bool(true)),
                                _ => self.array(out),
                            }
                        }
                        "reduce" => {
                            let (mut acc, start) = if let Some(v) = args.get(1) {
                                (v.clone(), 0)
                            } else {
                                (
                                    a.first().cloned().ok_or_else(|| {
                                        fault(
                                            "type",
                                            "Reduce of empty array without initial value",
                                            usize::MAX,
                                            false,
                                        )
                                    })?,
                                    1,
                                )
                            };
                            for (i, v) in a.iter().enumerate().skip(start) {
                                self.step(0, depth)?;
                                acc = self
                                    .call(
                                        first.clone(),
                                        vec![
                                            acc,
                                            v.clone(),
                                            V::Num(i as f64),
                                            V::Array(array_id),
                                        ],
                                        depth + 1,
                                    )
                                    .await?;
                            }
                            Ok(acc)
                        }
                        "slice" => {
                            let start = index(&first, a.len(), 0)?;
                            let end = index(
                                args.get(1).unwrap_or(&V::Undefined),
                                a.len(),
                                a.len(),
                            )?;
                            self.array(a[start..end.max(start)].to_vec())
                        }
                        "includes" | "indexOf" => {
                            let mut found = None;
                            for (i, v) in a.iter().enumerate() {
                                self.step(0, depth)?;
                                if equal(v, &first) {
                                    found = Some(i);
                                    break;
                                }
                            }
                            Ok(if name == "includes" {
                                V::Bool(found.is_some())
                            } else {
                                V::Num(found.map_or(-1.0, |i| i as f64))
                            })
                        }
                        "join" => {
                            let sep = if matches!(first, V::Undefined) {
                                ",".into()
                            } else {
                                self.text(&first)?
                            };
                            let mut out = String::new();
                            for (i, v) in a.iter().enumerate() {
                                self.step(0, depth)?;
                                let text = if matches!(v, V::Null | V::Undefined) {
                                    String::new()
                                } else {
                                    self.text(&v)?
                                };
                                self.reserve(
                                    text.len() + if i > 0 { sep.len() } else { 0 },
                                )?;
                                if i > 0 {
                                    out.push_str(&sep);
                                }
                                out.push_str(&text);
                            }
                            self.string(out)
                        }
                        "concat" => {
                            let mut out = a.clone();
                            for v in args {
                                self.step(0, depth)?;
                                if let V::Array(id) = v {
                                    let a = self.array_snapshot(id)?;
                                    self.reserve(a.len().saturating_mul(64))?;
                                    out.extend(a.iter().cloned());
                                } else {
                                    self.reserve(64)?;
                                    out.push(v);
                                }
                            }
                            self.array(out)
                        }
                        "flat" => {
                            if !matches!(first, V::Undefined) && first.number()? != 1.0 {
                                return Err(fault(
                                    "unsupported",
                                    "flat supports depth 1 only",
                                    0,
                                    false,
                                ));
                            }
                            let mut out = Vec::new();
                            for v in a.iter() {
                                self.step(0, depth)?;
                                if let V::Array(id) = v {
                                    let a = self.array_snapshot(*id)?;
                                    self.reserve(a.len().saturating_mul(64))?;
                                    out.extend(a.iter().cloned());
                                } else {
                                    out.push(v.clone());
                                }
                            }
                            self.array(out)
                        }
                        _ => Err(fault(
                            "unsupported",
                            "Unsupported array method",
                            usize::MAX,
                            false,
                        )),
                    }
                }
                V::Str(s) => match name.as_str() {
                    "indexOf" => {
                        let needle = self.text(&first)?;
                        self.string_index_of(
                            &s,
                            &needle,
                            args.get(1).unwrap_or(&V::Undefined),
                            depth,
                        )
                        .await
                    }
                    "trim" => {
                        let trimmed = s.trim();
                        self.reserve(trimmed.len().saturating_add(32))?;
                        self.string(trimmed.to_owned())
                    }
                    "toLowerCase" => {
                        self.reserve(s.len().saturating_mul(3))?;
                        self.string(s.to_lowercase())
                    }
                    "toUpperCase" => {
                        self.reserve(s.len().saturating_mul(3))?;
                        self.string(s.to_uppercase())
                    }
                    "repeat" => {
                        let n = integer_option(Some(&first), 0)?;
                        let bytes = s.len().checked_mul(n).ok_or_else(|| {
                            fault("limit", "repeat size overflow", usize::MAX, true)
                        })?;
                        self.reserve(bytes)?;
                        if n > self.limits.max_steps as usize {
                            return Err(fault(
                                "limit",
                                "repeat count limit exceeded",
                                0,
                                true,
                            ));
                        }
                        self.string(s.repeat(n))
                    }
                    "includes" => Ok(V::Bool(s.contains(&self.text(&first)?))),
                    "startsWith" => Ok(V::Bool(s.starts_with(&self.text(&first)?))),
                    "endsWith" => Ok(V::Bool(s.ends_with(&self.text(&first)?))),
                    "slice" => {
                        self.reserve(s.len().saturating_mul(4))?;
                        let chars: Vec<char> = s.chars().collect();
                        let start = index(&first, chars.len(), 0)?;
                        let end = index(
                            args.get(1).unwrap_or(&V::Undefined),
                            chars.len(),
                            chars.len(),
                        )?;
                        self.string(chars[start..end.max(start)].iter().collect())
                    }
                    "split" => {
                        if matches!(first, V::Undefined) {
                            return self.array(vec![V::Str(s)]);
                        }
                        let sep = self.text(&first)?;
                        let limit =
                            integer_option(args.get(1), self.limits.max_steps as usize)?
                                .min(self.limits.max_steps as usize);
                        let mut out = Vec::new();
                        if sep.is_empty() {
                            for c in s.chars().take(limit) {
                                self.step(0, depth)?;
                                self.reserve(64)?;
                                out.push(self.string(c.to_string())?);
                            }
                        } else {
                            for piece in s.split(&sep).take(limit) {
                                self.step(0, depth)?;
                                self.reserve(64)?;
                                out.push(self.string(piece.into())?);
                            }
                        }
                        self.array(out)
                    }
                    "replace" => {
                        let needle = self.text(&first)?;
                        let replacement =
                            self.text(args.get(1).unwrap_or(&V::Undefined))?;
                        self.reserve(s.len().saturating_add(replacement.len()))?;
                        self.string(s.replacen(&needle, &replacement, 1))
                    }
                    _ => Err(fault(
                        "unsupported",
                        "Unsupported string method",
                        usize::MAX,
                        false,
                    )),
                },
                _ => Err(fault("type", "Invalid method receiver", usize::MAX, false)),
            }
        }
        .boxed()
    }
}
// First pass admits each DOM node, decoded string/key and map/vector capacity
// before serde_json is permitted to materialize any Value collection.
fn json_error<E: serde::de::Error>(engine: &Engine, error: Fault) -> E {
    if error.fatal {
        let mut state = engine.state.lock().unwrap();
        if state.fatal.is_none() {
            state.fatal = Some(error.clone());
        }
        engine.local.store(true, Ordering::Relaxed);
    }
    E::custom(error)
}
struct JsonSeed<'a> {
    engine: &'a Engine,
    depth: usize,
    key: bool,
}
impl<'de> serde::de::DeserializeSeed<'de> for JsonSeed<'_> {
    type Value = ();
    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<(), D::Error> {
        self.engine
            .step(usize::MAX, self.depth)
            .map_err(|error| json_error(self.engine, error))?;
        self.engine
            .reserve(if self.key { 512 } else { 64 })
            .map_err(|error| json_error(self.engine, error))?;
        if self.key {
            deserializer.deserialize_str(self)
        } else {
            deserializer.deserialize_any(self)
        }
    }
}
impl std::fmt::Display for Fault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind, self.message)
    }
}
impl<'de> serde::de::Visitor<'de> for JsonSeed<'_> {
    type Value = ();
    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("bounded JSON data")
    }
    fn visit_unit<E: serde::de::Error>(self) -> Result<(), E> {
        Ok(())
    }
    fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<(), E> {
        Ok(())
    }
    fn visit_i64<E: serde::de::Error>(self, n: i64) -> Result<(), E> {
        if n.unsigned_abs() > 9_007_199_254_740_991 {
            Err(E::custom(
                "Unsafe integer JSON value; use strings for large IDs",
            ))
        } else {
            Ok(())
        }
    }
    fn visit_u64<E: serde::de::Error>(self, n: u64) -> Result<(), E> {
        if n > 9_007_199_254_740_991 {
            Err(E::custom(
                "Unsafe integer JSON value; use strings for large IDs",
            ))
        } else {
            Ok(())
        }
    }
    fn visit_f64<E: serde::de::Error>(self, n: f64) -> Result<(), E> {
        if !n.is_finite() || (n.fract() == 0.0 && n.abs() > MAX_SAFE_INTEGER) {
            Err(E::custom("Nonfinite or unsafe integer JSON number"))
        } else {
            Ok(())
        }
    }
    fn visit_str<E: serde::de::Error>(self, s: &str) -> Result<(), E> {
        if self.key && blocked(s) {
            return Err(E::custom("Prototype keys are forbidden"));
        }
        self.engine
            .reserve(s.len().saturating_mul(4).saturating_add(32))
            .map_err(|error| json_error(self.engine, error))
    }
    fn visit_borrowed_str<E: serde::de::Error>(self, s: &'de str) -> Result<(), E> {
        self.visit_str(s)
    }
    fn visit_string<E: serde::de::Error>(self, s: String) -> Result<(), E> {
        self.visit_str(&s)
    }
    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        self.engine
            .reserve(64)
            .map_err(|error| json_error(self.engine, error))?;
        while seq
            .next_element_seed(JsonSeed {
                engine: self.engine,
                depth: self.depth + 1,
                key: false,
            })?
            .is_some()
        {}
        Ok(())
    }
    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        self.engine
            .reserve(1024)
            .map_err(|error| json_error(self.engine, error))?;
        while map
            .next_key_seed(JsonSeed {
                engine: self.engine,
                depth: self.depth + 1,
                key: true,
            })?
            .is_some()
        {
            map.next_value_seed(JsonSeed {
                engine: self.engine,
                depth: self.depth + 1,
                key: false,
            })?;
        }
        Ok(())
    }
}
fn validate_graph(
    engine: &Engine,
    state: &State,
    value: &V,
    mutation: &Mutation,
    depth: usize,
    path: &mut [(u8, usize); 64],
    used: &mut u64,
) -> R<()> {
    *used = used.saturating_add(1);
    if engine.external.load(Ordering::Relaxed) || engine.local.load(Ordering::Relaxed) {
        return Err(fault("cancelled", "Execution cancelled", usize::MAX, true));
    }
    if Instant::now() >= engine.deadline {
        return Err(fault(
            "timeout",
            "Execution wall-clock limit exceeded",
            0,
            true,
        ));
    }
    if depth >= engine.limits.max_depth.min(64)
        || state.steps.saturating_add(*used) > engine.limits.max_steps
    {
        return Err(fault(
            "limit",
            "Data graph depth/step limit exceeded",
            usize::MAX,
            true,
        ));
    }
    let identity = match value {
        V::Array(id) => (1, *id),
        V::Object(id) => (2, *id),
        _ => return Ok(()),
    };
    if path[..depth].contains(&identity) {
        return Err(fault(
            "security",
            "Cyclic container insertion is unsupported",
            usize::MAX,
            false,
        ));
    }
    path[depth] = identity;
    match value {
        V::Array(id) => {
            for (index, v) in state.arrays[*id].iter().enumerate() {
                let v = match mutation {
                    Mutation::Array(target, i, replacement)
                        if target == id && *i == index =>
                    {
                        replacement
                    }
                    _ => v,
                };
                validate_graph(engine, state, v, mutation, depth + 1, path, used)?;
            }
            if let Mutation::Push(target, items) = mutation {
                if target == id {
                    for value in items {
                        validate_graph(
                            engine,
                            state,
                            value,
                            mutation,
                            depth + 1,
                            path,
                            used,
                        )?;
                    }
                }
            }
        }
        V::Object(id) => {
            let replacement = match mutation {
                Mutation::Object(target, key, value) if target == id => {
                    Some((key, value))
                }
                _ => None,
            };
            for (key, v) in &state.objects[*id] {
                let v = replacement.filter(|(k, _)| *k == key).map_or(v, |(_, v)| v);
                validate_graph(engine, state, v, mutation, depth + 1, path, used)?;
            }
            if let Some((key, v)) = replacement {
                if !state.objects[*id].contains_key(key) {
                    validate_graph(engine, state, v, mutation, depth + 1, path, used)?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}
fn clip(s: &mut String, limit: usize) -> bool {
    if s.len() <= limit {
        return false;
    }
    let mut end = limit;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
    true
}
fn equal(a: &V, b: &V) -> bool {
    match (a, b) {
        (V::Undefined, V::Undefined) | (V::Null, V::Null) => true,
        (V::Bool(a), V::Bool(b)) => a == b,
        (V::Num(a), V::Num(b)) => a == b,
        (V::Str(a), V::Str(b)) => a == b,
        (V::Array(a), V::Array(b)) => a == b,
        (V::Object(a), V::Object(b)) => a == b,
        (V::Function(_, _, a), V::Function(_, _, b)) => a == b,
        _ => false,
    }
}
fn integer_option(v: Option<&V>, default: usize) -> R<usize> {
    match v {
        None | Some(V::Undefined) => Ok(default),
        Some(V::Num(n))
            if n.is_finite()
                && *n >= 0.0
                && n.fract() == 0.0
                && *n < (usize::MAX as f64) =>
        {
            Ok(*n as usize)
        }
        _ => Err(fault(
            "type",
            "Expected bounded nonnegative integer",
            usize::MAX,
            false,
        )),
    }
}
fn index(v: &V, len: usize, default: usize) -> R<usize> {
    if matches!(v, V::Undefined) {
        return Ok(default);
    }
    let n = v.number()?;
    if !n.is_finite() {
        return Err(fault(
            "type",
            "Slice index must be finite",
            usize::MAX,
            false,
        ));
    }
    Ok(if n < 0.0 {
        (len as f64 + n.trunc()).max(0.0) as usize
    } else {
        (n.trunc() as usize).min(len)
    })
}
fn bounded_json(value: &Value, limit: usize) -> Result<Vec<u8>, ()> {
    struct Writer {
        bytes: Vec<u8>,
        limit: usize,
    }
    impl std::io::Write for Writer {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            if b.len() > self.limit.saturating_sub(self.bytes.len()) {
                return Err(std::io::Error::other("JSON length limit"));
            }
            self.bytes.extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = Writer {
        bytes: Vec::new(),
        limit,
    };
    serde_json::to_writer(&mut writer, value).map_err(|_| ())?;
    Ok(writer.bytes)
}

pub(crate) async fn execute(
    code: &str,
    host: Arc<dyn ToolHost>,
    limits: Limits,
    cancel: Arc<AtomicBool>,
) -> ExecutionResult {
    let mut result = ExecutionResult {
        ok: false,
        value: Value::Null,
        error: None,
        logs: Vec::new(),
        tool_calls: Vec::new(),
        truncated: false,
        warnings: Vec::new(),
    };
    if cancel.load(Ordering::Relaxed) {
        result.error = Some(lower::diagnostic(
            code,
            0,
            "cancelled",
            "Execution cancelled",
        ));
        return result;
    }
    let start = Instant::now();
    let (body, preflight_bytes) = match lower::parse(code, &limits) {
        Ok(b) => b,
        Err(e) => {
            result.error = Some(e);
            return result;
        }
    };
    if limits.concurrency == 0 || limits.max_depth == 0 {
        result.error = Some(lower::diagnostic(
            code,
            0,
            "limit",
            "Concurrency and depth must be positive",
        ));
        return result;
    }
    let deadline = start.checked_add(limits.wall_clock).unwrap_or(start);
    let catalog = host.catalog();
    let mut paths = std::collections::BTreeSet::new();
    for descriptor in &catalog {
        if descriptor.path.split('.').any(|part| {
            part.is_empty()
                || blocked(part)
                || !part.chars().enumerate().all(|(i, c)| {
                    c == '_'
                        || c == '$'
                        || c.is_ascii_alphabetic()
                        || (i > 0 && c.is_ascii_digit())
                })
        }) || !paths.insert(descriptor.path.as_str())
        {
            result.error = Some(lower::diagnostic(
                code,
                0,
                "catalog",
                "Catalog contains duplicate or invalid tool-expression paths",
            ));
            return result;
        }
    }
    drop(paths);
    let catalog_bytes = catalog.iter().try_fold(0usize, |n, d| {
        n.checked_add(d.path.len())?
            .checked_add(d.description.len())?
            .checked_add(d.signature.len())?
            .checked_add(128)
    });
    if catalog_bytes.is_none_or(|n| {
        n.saturating_add(code.len().saturating_mul(64))
            .saturating_add(preflight_bytes)
            > limits.max_data_bytes
    }) {
        result.error = Some(lower::diagnostic(
            code,
            0,
            "limit",
            "Catalog exceeds data allocation limit",
        ));
        return result;
    }
    let state = Arc::new(Mutex::new(State {
        fatal: None,
        function_ids: 0,
        arrays: Vec::new(),
        objects: Vec::new(),
        caught_errors: std::collections::BTreeSet::new(),
        envs: vec![Env {
            parent: None,
            vars: BTreeMap::new(),
        }],
        steps: 0,
        yield_at: 128,
        allocated: catalog_bytes.unwrap()
            + code.len().saturating_mul(64)
            + preflight_bytes,
        logs: Vec::new(),
        log_bytes: 0,
        records: Vec::new(),
        promises: Vec::new(),
        tasks: Vec::new(),
        aborts: Vec::new(),
        truncated: false,
    }));
    let engine = Engine {
        source: Arc::from(code),
        state: state.clone(),
        host,
        catalog: Arc::new(catalog),
        semaphore: Arc::new(Semaphore::new(limits.concurrency.min(4096))),
        limits: Arc::new(limits),
        local: Arc::new(AtomicBool::new(false)),
        external: cancel,
        deadline,
    };
    let _guard = ExecutionGuard {
        state: state.clone(),
        cancel: engine.local.clone(),
    };
    let run = async {
        let flow = engine.list(&body, 0, 0).await?;
        let v = match flow {
            Flow::Return(v) | Flow::Next(v) => v,
            _ => {
                return Err(fault(
                    "syntax",
                    "break/continue outside loop",
                    usize::MAX,
                    false,
                ))
            }
        };
        let value = if matches!(v, V::Undefined) {
            Value::Null
        } else {
            engine.json(&v, 0)?
        };
        if bounded_json(&value, engine.limits.max_output_bytes).is_err() {
            state.lock().unwrap().truncated = true;
            return Err(fault(
                "limit",
                "Retained JSON output limit exceeded",
                0,
                true,
            ));
        }
        Ok(value)
    };
    match engine.wait(run).await {
        Ok(v) => {
            result.ok = true;
            result.value = v;
        }
        Err(e) => result.error = Some(engine.diagnostic(&e)),
    }
    // Signal every host before cleanup. No spawned interpreter or tool future is detached.
    engine.local.store(true, Ordering::Relaxed);
    let cleanup_deadline = tokio::time::Instant::now()
        + engine.limits.cleanup_timeout.min(Duration::from_secs(5));
    loop {
        let tasks = std::mem::take(&mut state.lock().unwrap().tasks);
        if tasks.is_empty() {
            break;
        }
        for mut task in tasks {
            if tokio::time::timeout_at(cleanup_deadline, &mut task)
                .await
                .is_err()
            {
                task.abort();
                let _ = task.await;
            }
        }
    }
    let mut s = state.lock().unwrap();
    for r in &mut s.records {
        if r.status == "running" {
            r.status = "interrupted".into();
            r.error = Some("Execution finished before call completed".into());
        }
    }
    result.logs = std::mem::take(&mut s.logs);
    result.tool_calls = std::mem::take(&mut s.records);
    result.truncated = s.truncated;
    // Bound retained call payloads separately from the returned JSON value. Keep all
    // call identities/statuses even if their input/error details must be elided.
    let value_bytes = bounded_json(&result.value, engine.limits.max_output_bytes)
        .map_or(0, |bytes| bytes.len());
    let mut retained = engine.limits.max_output_bytes.saturating_sub(value_bytes);
    for record in &mut result.tool_calls {
        if let Ok(bytes) = bounded_json(&record.input, retained) {
            retained -= bytes.len();
        } else {
            record.input = Value::Null;
            result.truncated = true;
        }
        if let Some(error) = &mut record.error {
            if clip(error, retained.min(4096)) {
                result.truncated = true;
            }
            retained = retained.saturating_sub(error.len());
        }
    }
    if let Some(error) = &mut result.error {
        if clip(&mut error.message, engine.limits.max_output_bytes.min(4096)) {
            result.truncated = true;
        }
    }
    if result.truncated {
        result.warnings.push(
            "Some output, logs or call details were truncated by retention limits".into(),
        );
    }
    result
}

impl Engine {
    fn tool(&self, path: String, args: Vec<V>) -> R<V> {
        if args.len() > 1 {
            return Err(fault(
                "type",
                "Tools accept exactly one JSON argument",
                usize::MAX,
                false,
            ));
        }
        let fallback = self.object(BTreeMap::new())?;
        let input = self.json(args.first().unwrap_or(&fallback), 0)?;
        self.charge_json_clone(&input, 0)?;
        self.reserve(path.len() + 256)?;
        let index = {
            let mut s = self.state.lock().unwrap();
            if s.records.len() >= self.limits.max_calls {
                self.local.store(true, Ordering::Relaxed);
                return Err(fault("limit", "Tool call limit exceeded", usize::MAX, true));
            }
            let index = s.records.len();
            s.records.push(CallRecord {
                id: format!("call_{}", index + 1),
                path: path.clone(),
                input: input.clone(),
                status: "running".into(),
                error: None,
            });
            index
        };
        let engine = self.clone();
        self.spawn(async move {
            let result: R<V> = async {
                let permit = engine
                    .wait(async {
                        engine.semaphore.clone().acquire_owned().await.map_err(|_| {
                            fault(
                                "cancelled",
                                "Tool admission interrupted",
                                usize::MAX,
                                true,
                            )
                        })
                    })
                    .await?;
                engine.step(0, 0)?;
                let aliases = engine.host.result_aliases(&path);
                let host_future = engine.host.call(path, input, engine.local.clone());
                let output = engine
                    .wait(async {
                        match host_future.await {
                            Ok(output) => Ok(output),
                            Err(mut e) => {
                                if clip(&mut e, engine.limits.max_output_bytes.min(4096))
                                {
                                    engine.state.lock().unwrap().truncated = true;
                                }
                                engine.reserve(e.len().saturating_mul(2))?;
                                Err(fault("tool", e, usize::MAX, false))
                            }
                        }
                    })
                    .await?;
                drop(permit);
                let imported = engine.from_json(&output, 0)?;
                engine.result_aliases(imported, aliases)
            }
            .await;
            {
                let mut s = engine.state.lock().unwrap();
                let r = &mut s.records[index];
                match &result {
                    Ok(_) => r.status = "completed".into(),
                    Err(e) => {
                        r.status = if e.fatal { "interrupted" } else { "error" }.into();
                        r.error = Some(e.message.clone());
                    }
                }
            }
            result
        })
    }
}
