//! Small, conservative receiver preflight over the already-owned AST. Not a JS
//! type system: host data, object properties and user-function results stay unknown.
use crate::{
    lower::{diagnostic, Expr, Stmt, E, S},
    members::{self, Receiver},
    Diagnostic, Limits,
};
use std::collections::BTreeMap;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ty<'a> {
    Unknown,
    String,
    Array,
    Object,
    Number,
    Bool,
    Null,
    Undefined,
    Function,
    Tools,
    Builtin(&'static str),
    Method(Receiver, &'a str),
}
#[derive(Clone)]
struct Env<'a> {
    scopes: Vec<BTreeMap<&'a str, (Ty<'a>, bool)>>,
    unstable: bool,
}
impl<'a> Env<'a> {
    fn get(&self, name: &str) -> Ty<'a> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).map(|v| v.0))
            .unwrap_or(Ty::Unknown)
    }
    fn forget_mutable(&mut self) {
        self.unstable = true;
        for scope in &mut self.scopes {
            for (ty, constant) in scope.values_mut() {
                if !*constant {
                    *ty = Ty::Unknown;
                }
            }
        }
    }
    fn set(&mut self, name: &str, ty: Ty<'a>) {
        for scope in self.scopes.iter_mut().rev() {
            if let Some((previous, constant)) = scope.get_mut(name) {
                if !*constant {
                    *previous = if self.unstable { Ty::Unknown } else { ty };
                }
                break;
            }
        }
    }
}
pub(crate) fn check(
    code: &str,
    body: &[Stmt],
    limits: &Limits,
) -> Result<usize, Diagnostic> {
    let mut check = Check {
        code,
        limits,
        steps: 0,
        bytes: 0,
    };
    check.reserve(64, 0)?;
    let mut env = Env {
        scopes: vec![BTreeMap::new()],
        unstable: false,
    };
    check.list(body, &mut env, 0)?;
    Ok(check.bytes)
}
struct Check<'c> {
    code: &'c str,
    limits: &'c Limits,
    steps: u64,
    bytes: usize,
}
impl Check<'_> {
    fn step(&mut self, at: usize, depth: usize) -> Result<(), Diagnostic> {
        self.steps = self.steps.saturating_add(1);
        if self.steps > self.limits.max_steps || depth >= self.limits.max_depth.min(64) {
            return Err(diagnostic(
                self.code,
                at,
                "limit",
                "Static preflight depth/step limit exceeded",
            ));
        }
        Ok(())
    }
    fn reserve(&mut self, bytes: usize, at: usize) -> Result<(), Diagnostic> {
        self.bytes = self.bytes.saturating_add(bytes);
        if self
            .bytes
            .saturating_add(self.code.len().saturating_mul(64))
            > self.limits.max_data_bytes
        {
            return Err(diagnostic(
                self.code,
                at,
                "limit",
                "Static preflight allocation limit exceeded",
            ));
        }
        Ok(())
    }
    fn clone_env<'a>(&mut self, env: &Env<'a>, at: usize) -> Result<Env<'a>, Diagnostic> {
        self.reserve(
            env.scopes.len().saturating_mul(64).saturating_add(
                env.scopes
                    .iter()
                    .map(|scope| scope.len().saturating_mul(128))
                    .sum(),
            ),
            at,
        )?;
        Ok(env.clone())
    }
    fn scope<'a>(&mut self, env: &mut Env<'a>, at: usize) -> Result<(), Diagnostic> {
        self.reserve(64, at)?;
        env.scopes.push(BTreeMap::new());
        Ok(())
    }
    fn bind<'a>(
        &mut self,
        env: &mut Env<'a>,
        name: &'a str,
        ty: Ty<'a>,
        constant: bool,
        at: usize,
    ) -> Result<(), Diagnostic> {
        self.reserve(128, at)?;
        env.scopes.last_mut().unwrap().insert(
            name,
            (
                if !constant && env.unstable {
                    Ty::Unknown
                } else {
                    ty
                },
                constant,
            ),
        );
        Ok(())
    }
    fn list<'a>(
        &mut self,
        body: &'a [Stmt],
        env: &mut Env<'a>,
        depth: usize,
    ) -> Result<(), Diagnostic> {
        for stmt in body {
            self.stmt(stmt, env, depth + 1)?;
        }
        Ok(())
    }
    fn stmt<'a>(
        &mut self,
        stmt: &'a Stmt,
        env: &mut Env<'a>,
        depth: usize,
    ) -> Result<(), Diagnostic> {
        self.step(stmt.at, depth)?;
        match &stmt.kind {
            S::Empty | S::Break | S::Continue => {}
            S::Expr(expr) | S::Throw(expr) => {
                self.expr(expr, env, depth + 1)?;
            }
            S::Return(expr) => {
                if let Some(expr) = expr {
                    self.expr(expr, env, depth + 1)?;
                }
            }
            S::Bind(bindings) => {
                for (name, constant, expr) in bindings {
                    let ty = if let Some(expr) = expr {
                        self.expr(expr, env, depth + 1)?
                    } else {
                        Ty::Undefined
                    };
                    self.bind(env, name, ty, *constant, stmt.at)?;
                }
            }
            S::Block(body) => {
                self.scope(env, stmt.at)?;
                self.list(body, env, depth + 1)?;
                env.scopes.pop();
            }
            S::If(test, yes, no) => {
                self.expr(test, env, depth + 1)?;
                let mut branch = self.clone_env(env, stmt.at)?;
                branch.forget_mutable();
                self.stmt(yes, &mut branch, depth + 1)?;
                if let Some(no) = no {
                    let mut branch = self.clone_env(env, stmt.at)?;
                    branch.forget_mutable();
                    self.stmt(no, &mut branch, depth + 1)?;
                }
                env.forget_mutable();
            }
            S::While(test, body) => {
                env.forget_mutable();
                self.expr(test, env, depth + 1)?;
                self.stmt(body, env, depth + 1)?;
                env.forget_mutable();
            }
            S::For(init, test, update, body) => {
                self.scope(env, stmt.at)?;
                if let Some(init) = init {
                    self.stmt(init, env, depth + 1)?;
                }
                env.forget_mutable();
                if let Some(test) = test {
                    self.expr(test, env, depth + 1)?;
                }
                self.stmt(body, env, depth + 1)?;
                if let Some(update) = update {
                    self.expr(update, env, depth + 1)?;
                }
                env.scopes.pop();
                env.forget_mutable();
            }
            S::ForOf(name, constant, right, body) => {
                self.expr(right, env, depth + 1)?;
                env.forget_mutable();
                self.scope(env, stmt.at)?;
                self.bind(env, name, Ty::Unknown, *constant, stmt.at)?;
                self.stmt(body, env, depth + 1)?;
                env.scopes.pop();
            }
            S::Try(body, name, catch) => {
                let mut branch = self.clone_env(env, stmt.at)?;
                self.scope(&mut branch, stmt.at)?;
                self.list(body, &mut branch, depth + 1)?;
                let mut branch = self.clone_env(env, stmt.at)?;
                branch.forget_mutable();
                self.scope(&mut branch, stmt.at)?;
                if let Some(name) = name {
                    self.bind(&mut branch, name, Ty::Object, true, stmt.at)?;
                }
                self.list(catch, &mut branch, depth + 1)?;
                env.forget_mutable();
            }
        }
        Ok(())
    }
    fn expr<'a>(
        &mut self,
        expr: &'a Expr,
        env: &mut Env<'a>,
        depth: usize,
    ) -> Result<Ty<'a>, Diagnostic> {
        self.step(expr.at, depth)?;
        Ok(match &expr.kind {
            E::Literal(value) => match value {
                serde_json::Value::String(_) => Ty::String,
                serde_json::Value::Number(_) => Ty::Number,
                serde_json::Value::Bool(_) => Ty::Bool,
                serde_json::Value::Null => Ty::Null,
                _ => Ty::Unknown,
            },
            E::Name(name) => match name.as_str() {
                "tools" => Ty::Tools,
                "undefined" => Ty::Undefined,
                "String" => Ty::Builtin("String"),
                "Number" => Ty::Builtin("Number"),
                "JSON" => Ty::Builtin("JSON"),
                "Object" => Ty::Builtin("Object"),
                "Math" => Ty::Builtin("Math"),
                "console" => Ty::Builtin("console"),
                "Promise" => Ty::Builtin("Promise"),
                "search" => Ty::Builtin("search"),
                _ => env.get(name),
            },
            E::Array(values) => {
                for value in values {
                    self.expr(value, env, depth + 1)?;
                }
                Ty::Array
            }
            E::Object(values) => {
                for (_, value) in values {
                    self.expr(value, env, depth + 1)?;
                }
                Ty::Object
            }
            E::Function(function) => {
                let mut closure = self.clone_env(env, expr.at)?;
                closure.forget_mutable();
                self.scope(&mut closure, expr.at)?;
                for name in &function.params {
                    self.bind(&mut closure, name, Ty::Unknown, false, expr.at)?;
                }
                self.list(&function.body, &mut closure, depth + 1)?;
                Ty::Function
            }
            E::Member(base, key) => {
                let receiver = self.expr(base, env, depth + 1)?;
                self.expr(key, env, depth + 1)?;
                if receiver == Ty::Tools {
                    Ty::Tools
                } else if let E::Literal(serde_json::Value::String(key)) = &key.kind {
                    let class = match receiver {
                        Ty::String => Some(Receiver::String),
                        Ty::Array => Some(Receiver::Array),
                        Ty::Builtin(base) => Some(Receiver::Builtin(base)),
                        _ => None,
                    };
                    if let Some(class) = class {
                        if !members::allowed(class, key) {
                            return Err(diagnostic(
                                self.code,
                                expr.at,
                                "unsupported",
                                format!("Statically unsupported member: {key}"),
                            ));
                        }
                        if key == "length"
                            && matches!(class, Receiver::Array | Receiver::String)
                        {
                            Ty::Number
                        } else if key.parse::<usize>().is_ok() {
                            Ty::Unknown
                        } else {
                            Ty::Method(class, key)
                        }
                    } else if matches!(
                        receiver,
                        Ty::Number | Ty::Bool | Ty::Null | Ty::Undefined
                    ) {
                        return Err(diagnostic(
                            self.code,
                            expr.at,
                            "unsupported",
                            format!("Statically unsupported primitive member: {key}"),
                        ));
                    } else {
                        Ty::Unknown
                    }
                } else {
                    Ty::Unknown
                }
            }
            E::Call(callee, args) => {
                let callee = self.expr(callee, env, depth + 1)?;
                let mut first = Ty::Undefined;
                for (index,arg) in args.iter().enumerate() {
                    let ty = self.expr(arg, env, depth + 1)?;
                    if index==0 {first=ty;}
                }
                match callee {
                    Ty::Builtin("String") => Ty::String,
                    Ty::Builtin("Number") => Ty::Number,
                    Ty::Builtin("search") => Ty::Object,
                    Ty::Method(Receiver::String, name) => match name {
                        "indexOf" => Ty::Number,
                        "split" => Ty::Array,
                        "includes" | "startsWith" | "endsWith" => Ty::Bool,
                        _ => Ty::String,
                    },
                    Ty::Method(Receiver::Array, name) => {
                        if matches!(
                            name,
                            "map"
                                | "filter"
                                | "flatMap"
                                | "reduce"
                                | "find"
                                | "findIndex"
                                | "some"
                                | "every"
                        ) {
                            env.forget_mutable();
                        }
                        match name {
                            "push" | "indexOf" | "findIndex" => Ty::Number,
                            "join" => Ty::String,
                            "includes" | "some" | "every" => Ty::Bool,
                            "map" | "filter" | "flatMap" | "slice" | "concat"
                            | "flat" => Ty::Array,
                            _ => Ty::Unknown,
                        }
                    }
                    Ty::Method(Receiver::Builtin(base), name) => match base {
                        "Math" => Ty::Number,
                        "Number" => Ty::Bool,
                        "Object" => {
                            if name == "fromEntries" {
                                Ty::Object
                            } else {
                                Ty::Array
                            }
                        }
                        "JSON" if name=="stringify" => match first {Ty::Unknown=>Ty::Unknown,Ty::Undefined=>Ty::Undefined,_=>Ty::String},
                        "JSON" => Ty::Unknown,
                        _ => Ty::Unknown,
                    },
                    Ty::Tools => Ty::Unknown,
                    Ty::Unknown | Ty::Function => {
                        env.forget_mutable();
                        Ty::Unknown
                    }
                    _ => {
                        return Err(diagnostic(
                            self.code,
                            expr.at,
                            "unsupported",
                            "Statically non-callable value",
                        ))
                    }
                }
            }
            E::Await(value) => self.expr(value, env, depth + 1)?,
            E::Unary(op, value) => {
                self.expr(value, env, depth + 1)?;
                match op.as_str() {
                    "typeof" => Ty::String,
                    "!" => Ty::Bool,
                    "void" => Ty::Undefined,
                    _ => Ty::Number,
                }
            }
            E::Binary(op, left, right) => {
                let left = self.expr(left, env, depth + 1)?;
                if matches!(op.as_str(), "&&" | "||" | "??") {
                    env.forget_mutable();
                }
                let right = self.expr(right, env, depth + 1)?;
                match op.as_str() {
                    "+" if left == Ty::String || right == Ty::String => Ty::String,
                    "+" | "-" | "*" | "/" | "%" | "**" => Ty::Number,
                    "&&" | "||" | "??" => {
                        if left == right {
                            left
                        } else {
                            Ty::Unknown
                        }
                    }
                    _ => Ty::Bool,
                }
            }
            E::Conditional(test, yes, no) => {
                self.expr(test, env, depth + 1)?;
                env.forget_mutable();
                let mut branch = self.clone_env(env, expr.at)?;
                let yes = self.expr(yes, &mut branch, depth + 1)?;
                let mut branch = self.clone_env(env, expr.at)?;
                let no = self.expr(no, &mut branch, depth + 1)?;
                if yes == no {
                    yes
                } else {
                    Ty::Unknown
                }
            }
            E::Assign(target, op, value) => {
                let previous = self.expr(target, env, depth + 1)?;
                let value = self.expr(value, env, depth + 1)?;
                let ty = if op == "=" {
                    value
                } else if op == "+=" && (previous == Ty::String || value == Ty::String) {
                    Ty::String
                } else {
                    Ty::Number
                };
                if let E::Name(name) = &target.kind {
                    env.set(name, ty);
                }
                ty
            }
            E::Update(target, _, _) => {
                self.expr(target, env, depth + 1)?;
                if let E::Name(name) = &target.kind {
                    env.set(name, Ty::Number);
                }
                Ty::Number
            }
        })
    }
}
