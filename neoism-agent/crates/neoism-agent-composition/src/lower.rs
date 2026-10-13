use crate::{Diagnostic, Limits};
use oxc_allocator::Allocator;
use oxc_ast::ast as js;
use oxc_parser::{ParseOptions, Parser};
use oxc_span::{GetSpan, SourceType, Span};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub(crate) struct Expr {
    pub at: usize,
    pub kind: E,
}
#[derive(Clone, Debug)]
pub(crate) enum E {
    Literal(serde_json::Value),
    Name(String),
    Array(Vec<Expr>),
    Object(Vec<(String, Expr)>),
    Member(Box<Expr>, Box<Expr>),
    Call(Box<Expr>, Vec<Expr>),
    Await(Box<Expr>),
    Unary(String, Box<Expr>),
    Binary(String, Box<Expr>, Box<Expr>),
    Conditional(Box<Expr>, Box<Expr>, Box<Expr>),
    Assign(Box<Expr>, String, Box<Expr>),
    Update(Box<Expr>, bool, bool),
    Function(Arc<Function>),
}
#[derive(Clone, Debug)]
pub(crate) struct Function {
    pub params: Vec<String>,
    pub body: Vec<Stmt>,
    pub asynchronous: bool,
}
#[derive(Clone, Debug)]
pub(crate) struct Stmt {
    pub at: usize,
    pub kind: S,
}
#[derive(Clone, Debug)]
pub(crate) enum S {
    Empty,
    Expr(Expr),
    Bind(Vec<(String, bool, Option<Expr>)>),
    Block(Vec<Stmt>),
    If(Expr, Box<Stmt>, Option<Box<Stmt>>),
    While(Expr, Box<Stmt>),
    For(Option<Box<Stmt>>, Option<Expr>, Option<Expr>, Box<Stmt>),
    ForOf(String, bool, Expr, Box<Stmt>),
    Return(Option<Expr>),
    Break,
    Continue,
    Throw(Expr),
    Try(Vec<Stmt>, Option<String>, Vec<Stmt>),
}
pub(crate) fn diagnostic(
    code: &str,
    at: usize,
    kind: &str,
    message: impl Into<String>,
) -> Diagnostic {
    let prefix = &code[..at.min(code.len())];
    Diagnostic {
        kind: kind.into(),
        message: message.into(),
        line: Some(prefix.bytes().filter(|b| *b == b'\n').count() + 1),
        column: Some(prefix.rsplit('\n').next().unwrap_or("").chars().count() + 1),
    }
}
pub(crate) fn blocked(key: &str) -> bool {
    matches!(key, "__proto__" | "constructor" | "prototype")
}
fn reserved(key: &str) -> bool {
    matches!(
        key,
        "tools"
            | "search"
            | "JSON"
            | "Object"
            | "String"
            | "Number"
            | "Math"
            | "console"
            | "Promise"
            | "undefined"
    )
}

pub(crate) fn parse(
    code: &str,
    limits: &Limits,
) -> Result<(Vec<Stmt>, usize), Diagnostic> {
    if code.len().saturating_mul(64) > limits.max_data_bytes {
        return Err(diagnostic(
            code,
            0,
            "limit",
            "Source/AST allocation budget exceeded",
        ));
    }
    if code.len() > limits.max_source_bytes.min(64 * 1024) {
        return Err(diagnostic(code, 0, "limit", "Source length limit exceeded"));
    }
    // Conservative lexical complexity guard BEFORE the recursive third-party parser.
    // Not a JavaScript parser: only delimiters/expression runs are counted, with quoted
    // strings and comments skipped. Ambiguous regex and templates are not supported.
    let mut depth = 0usize;
    let mut run = 0usize;
    let mut recursive_tokens = 0usize;
    let mut previous_identifier = false;
    let mut expression_end = false;
    let mut word = "";
    let mut member_key = false;
    let mut word_is_member = false;
    let mut control_parens = [false; 64];
    let mut parens = 0usize;
    let mut quote = None;
    let mut escaped = false;
    let mut line_comment = false;
    let mut block_comment = false;
    let mut chars = code.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        if line_comment {
            if c == '\n' {
                line_comment = false;
            }
            continue;
        }
        if block_comment {
            if c == '*' && chars.peek().is_some_and(|(_, c)| *c == '/') {
                chars.next();
                block_comment = false;
            }
            continue;
        }
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == q {
                quote = None;
                expression_end = true;
                previous_identifier = false;
                word = "";
            }
            continue;
        }
        if c == '/' && chars.peek().is_some_and(|(_, c)| *c == '/') {
            chars.next();
            line_comment = true;
            continue;
        }
        if c == '/' && chars.peek().is_some_and(|(_, c)| *c == '*') {
            chars.next();
            block_comment = true;
            continue;
        }
        if c == '\\' {
            return Err(diagnostic(
                code,
                at,
                "unsupported",
                "Escaped identifiers are unsupported",
            ));
        }
        if c == '/' && !expression_end {
            return Err(diagnostic(
                code,
                at,
                "unsupported",
                "Regular expression literals are unsupported",
            ));
        }
        if matches!(c, '\'' | '"') {
            quote = Some(c);
            run += 1;
            continue;
        }
        if c == '`' {
            return Err(diagnostic(
                code,
                at,
                "unsupported",
                "Template literals are unsupported",
            ));
        }
        if matches!(
            c,
            '!' | '+'
                | '-'
                | '~'
                | '?'
                | ':'
                | '='
                | '&'
                | '|'
                | '*'
                | '/'
                | '%'
                | '<'
                | '>'
        ) {
            recursive_tokens += 1;
        }
        if !previous_identifier && (c.is_ascii_alphabetic() || c == '_') {
            let tail = &code[at..];
            let end = tail
                .find(|c: char| !c.is_alphanumeric() && c != '_' && c != '$')
                .unwrap_or(tail.len());
            word = &tail[..end];
            word_is_member = member_key;
            member_key = false;
            // Unsupported recursive statement/expression introducers must not reach
            // Oxc, even if ordinary delimiter/run guards would allow their shape.
            if !word_is_member
                && !tail[end..].trim_start().starts_with(':')
                && matches!(
                    word,
                    "with"
                        | "do"
                        | "switch"
                        | "class"
                        | "import"
                        | "export"
                        | "new"
                        | "yield"
                        | "debugger"
                        | "var"
                        | "delete"
                        | "super"
                        | "this"
                )
            {
                return Err(diagnostic(
                    code,
                    at,
                    "unsupported",
                    format!("Unsupported syntax introducer: {word}"),
                ));
            }
            expression_end = word_is_member
                || !matches!(
                    word,
                    "return"
                        | "throw"
                        | "await"
                        | "typeof"
                        | "void"
                        | "else"
                        | "in"
                        | "instanceof"
                );
            for keyword in [
                "if", "else", "function", "try", "catch", "while", "for", "async",
                "await", "typeof", "void",
            ] {
                if let Some(rest) = tail.strip_prefix(keyword) {
                    if !rest
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '$')
                    {
                        recursive_tokens += 1;
                        break;
                    }
                }
            }
        }
        previous_identifier = c.is_alphanumeric() || c == '_' || c == '$';
        if !c.is_whitespace() {
            run += c.len_utf8();
        }
        match c {
            '(' => {
                depth += 1;
                if parens >= 64 {
                    return Err(diagnostic(
                        code,
                        at,
                        "limit",
                        "Parser nesting limit exceeded",
                    ));
                }
                control_parens[parens] =
                    !word_is_member && matches!(word, "if" | "while" | "for" | "catch");
                parens += 1;
                expression_end = false;
                word = "";
            }
            '[' => {
                depth += 1;
                expression_end = false;
                word = "";
            }
            '{' => {
                depth += 1;
                run = 0;
                expression_end = false;
                word = "";
            }
            ')' => {
                depth = depth.saturating_sub(1);
                parens = parens.saturating_sub(1);
                expression_end = !control_parens[parens];
                word = "";
            }
            ']' => {
                depth = depth.saturating_sub(1);
                expression_end = true;
                word = "";
            }
            '}' => {
                depth = depth.saturating_sub(1);
                run = 0;
                expression_end = false;
                word = "";
            }
            ';' | ',' => {
                run = 0;
                expression_end = false;
                word = "";
            }
            '0'..='9' => expression_end = true,
            '.' => {
                member_key = true;
                expression_end = false;
                word = "";
            }
            '+' | '-' | '*' | '/' | '%' | '!' | '~' | '?' | ':' | '=' | '&' | '|'
            | '<' | '>' => {
                expression_end = false;
                word = "";
            }
            _ => {}
        }
        if depth > limits.max_depth.min(64) || run > 512 || recursive_tokens > 256 {
            return Err(diagnostic(
                code,
                at,
                "limit",
                "Parser complexity limit exceeded (nesting or expression run)",
            ));
        }
    }
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, code, SourceType::mjs())
        .with_options(ParseOptions {
            allow_return_outside_function: true,
            ..ParseOptions::default()
        })
        .parse();
    if let Some(error) = parsed.errors.first() {
        let at = error
            .labels
            .as_ref()
            .and_then(|labels| labels.first())
            .map_or(0, |label| label.offset());
        return Err(diagnostic(code, at, "syntax", error.to_string()));
    }
    if parsed.panicked {
        return Err(diagnostic(code, 0, "syntax", "Parser could not complete"));
    }
    let mut lower = Lower {
        code,
        max_depth: limits.max_depth.min(64),
        nodes: 0,
        max_nodes: limits.max_steps,
    };
    let body = lower.list(&parsed.program.body, 0)?;
    let preflight_bytes = crate::preflight::check(code, &body, limits)?;
    Ok((body, preflight_bytes))
}
fn valid_target(e: &Expr) -> bool {
    match &e.kind {
        E::Name(n) => !reserved(n) && !blocked(n),
        E::Member(base, key) => {
            valid_target(base)
                && match &key.kind {
                    E::Literal(serde_json::Value::String(s)) => !blocked(s),
                    E::Literal(serde_json::Value::Number(_)) | E::Name(_) => true,
                    _ => false,
                }
        }
        _ => false,
    }
}
struct Lower<'a> {
    code: &'a str,
    max_depth: usize,
    nodes: u64,
    max_nodes: u64,
}
impl Lower<'_> {
    fn err<T>(&self, span: Span, message: &str) -> Result<T, Diagnostic> {
        Err(diagnostic(
            self.code,
            span.start as usize,
            "unsupported",
            message,
        ))
    }
    fn guard(&mut self, span: Span, depth: usize) -> Result<(), Diagnostic> {
        self.nodes += 1;
        if depth >= self.max_depth || self.nodes > self.max_nodes {
            return Err(diagnostic(
                self.code,
                span.start as usize,
                "limit",
                "AST depth/node limit exceeded",
            ));
        }
        Ok(())
    }
    fn binding(&self, b: &js::BindingPattern<'_>) -> Result<String, Diagnostic> {
        match &b.kind {
            js::BindingPatternKind::BindingIdentifier(i)
                if !reserved(&i.name) && !blocked(&i.name) =>
            {
                Ok(i.name.to_string())
            }
            _ => self.err(
                b.span(),
                "Only simple, non-reserved identifier bindings are supported",
            ),
        }
    }
    fn params(&self, p: &js::FormalParameters<'_>) -> Result<Vec<String>, Diagnostic> {
        if p.rest.is_some() {
            return self.err(p.span, "Rest parameters are unsupported");
        }
        let mut names = Vec::new();
        for p in &p.items {
            let n = self.binding(&p.pattern)?;
            if names.contains(&n) {
                return self.err(p.span, "Duplicate parameter");
            }
            names.push(n);
        }
        Ok(names)
    }
    fn list(
        &mut self,
        list: &[js::Statement<'_>],
        depth: usize,
    ) -> Result<Vec<Stmt>, Diagnostic> {
        list.iter().map(|s| self.stmt(s, depth + 1)).collect()
    }
    fn vars(
        &mut self,
        v: &js::VariableDeclaration<'_>,
        depth: usize,
    ) -> Result<S, Diagnostic> {
        if !matches!(
            v.kind,
            js::VariableDeclarationKind::Const | js::VariableDeclarationKind::Let
        ) {
            return self.err(v.span, "Only const/let declarations are supported");
        }
        let mut out = Vec::new();
        for d in &v.declarations {
            out.push((
                self.binding(&d.id)?,
                v.kind == js::VariableDeclarationKind::Const,
                d.init
                    .as_ref()
                    .map(|e| self.expr(e, depth + 1))
                    .transpose()?,
            ));
        }
        Ok(S::Bind(out))
    }
    fn function(
        &mut self,
        f: &js::Function<'_>,
        depth: usize,
    ) -> Result<Arc<Function>, Diagnostic> {
        if f.generator {
            return self.err(f.span, "Generators are unsupported");
        }
        let body = f.body.as_ref().ok_or_else(|| {
            diagnostic(
                self.code,
                f.span.start as usize,
                "unsupported",
                "Function requires body",
            )
        })?;
        Ok(Arc::new(Function {
            params: self.params(&f.params)?,
            body: self.list(&body.statements, depth + 1)?,
            asynchronous: f.r#async,
        }))
    }
    fn stmt(&mut self, s: &js::Statement<'_>, depth: usize) -> Result<Stmt, Diagnostic> {
        self.guard(s.span(), depth)?;
        let kind = match s {
            js::Statement::EmptyStatement(_) => S::Empty,
            js::Statement::ExpressionStatement(e) => S::Expr(self.expr(&e.expression, depth + 1)?),
            js::Statement::VariableDeclaration(v) => self.vars(v, depth)?,
            js::Statement::BlockStatement(b) => S::Block(self.list(&b.body, depth)?),
            js::Statement::IfStatement(i) => S::If(self.expr(&i.test, depth+1)?, Box::new(self.stmt(&i.consequent, depth+1)?), i.alternate.as_ref().map(|s| self.stmt(s, depth+1).map(Box::new)).transpose()?),
            js::Statement::WhileStatement(w) => S::While(self.expr(&w.test, depth+1)?, Box::new(self.stmt(&w.body, depth+1)?)),
            js::Statement::ForStatement(f) => {
                let init = f.init.as_ref().map(|i| match i {
                    js::ForStatementInit::VariableDeclaration(v) => self.vars(v, depth).map(|kind| Box::new(Stmt { at: v.span.start as usize, kind })),
                    _ => self.expr(i.to_expression(), depth+1).map(|e| Box::new(Stmt { at: e.at, kind: S::Expr(e) })),
                }).transpose()?;
                S::For(init, f.test.as_ref().map(|e| self.expr(e, depth+1)).transpose()?, f.update.as_ref().map(|e| self.expr(e, depth+1)).transpose()?, Box::new(self.stmt(&f.body, depth+1)?))
            }
            js::Statement::ForOfStatement(f) => {
                if f.r#await { return self.err(f.span, "for-await is unsupported"); }
                let js::ForStatementLeft::VariableDeclaration(v) = &f.left else { return self.err(f.span, "for-of requires a const/let binding"); };
                if v.declarations.len() != 1 || !matches!(v.kind, js::VariableDeclarationKind::Const | js::VariableDeclarationKind::Let) { return self.err(f.span, "for-of requires one const/let binding"); }
                S::ForOf(self.binding(&v.declarations[0].id)?, v.kind == js::VariableDeclarationKind::Const, self.expr(&f.right, depth+1)?, Box::new(self.stmt(&f.body, depth+1)?))
            }
            js::Statement::ReturnStatement(r) => S::Return(r.argument.as_ref().map(|e| self.expr(e, depth+1)).transpose()?),
            js::Statement::BreakStatement(b) if b.label.is_none() => S::Break,
            js::Statement::ContinueStatement(c) if c.label.is_none() => S::Continue,
            js::Statement::ThrowStatement(t) => S::Throw(self.expr(&t.argument, depth+1)?),
            js::Statement::TryStatement(t) => {
                if t.finalizer.is_some() { return self.err(t.span, "finally is unsupported"); }
                let Some(h) = &t.handler else { return self.err(t.span, "try requires catch"); };
                S::Try(self.list(&t.block.body, depth+1)?, h.param.as_ref().map(|p| self.binding(&p.pattern)).transpose()?, self.list(&h.body.body, depth+1)?)
            }
            js::Statement::FunctionDeclaration(f) => {
                let Some(id) = &f.id else { return self.err(f.span, "Function declaration requires name"); };
                if reserved(&id.name) || blocked(&id.name) { return self.err(f.span, "Reserved binding"); }
                S::Bind(vec![(id.name.to_string(), true, Some(Expr { at: f.span.start as usize, kind: E::Function(self.function(f, depth+1)?) }))])
            }
            _ => return self.err(s.span(), "Unsupported statement (imports, classes, labels, switch, do/while and var are not available)"),
        };
        Ok(Stmt {
            at: s.span().start as usize,
            kind,
        })
    }
    fn target(
        &mut self,
        t: &js::AssignmentTarget<'_>,
        depth: usize,
    ) -> Result<Expr, Diagnostic> {
        let kind = match t {
            js::AssignmentTarget::AssignmentTargetIdentifier(i) if !reserved(&i.name) => {
                E::Name(i.name.to_string())
            }
            js::AssignmentTarget::StaticMemberExpression(m) => E::Member(
                Box::new(self.expr(&m.object, depth + 1)?),
                Box::new(Expr {
                    at: m.span.start as usize,
                    kind: E::Literal(m.property.name.to_string().into()),
                }),
            ),
            js::AssignmentTarget::ComputedMemberExpression(m) => E::Member(
                Box::new(self.expr(&m.object, depth + 1)?),
                Box::new(self.expr(&m.expression, depth + 1)?),
            ),
            _ => {
                return self.err(
                    t.span(),
                    "Unsupported assignment target or reserved global assignment",
                )
            }
        };
        let expr = Expr {
            at: t.span().start as usize,
            kind,
        };
        if !valid_target(&expr) {
            return self.err(t.span(), "Assignments require a local-rooted path with literal/identifier keys; global/prototype mutation and effectful targets are unsupported");
        }
        Ok(expr)
    }
    fn expr(&mut self, e: &js::Expression<'_>, depth: usize) -> Result<Expr, Diagnostic> {
        self.guard(e.span(), depth)?;
        let kind = match e {
            js::Expression::NullLiteral(_) => E::Literal(serde_json::Value::Null),
            js::Expression::BooleanLiteral(b) => E::Literal(b.value.into()),
            js::Expression::NumericLiteral(n) => { if !n.value.is_finite() || (n.value.fract() == 0.0 && n.value.abs() > 9_007_199_254_740_991.0) { return self.err(n.span, "Nonfinite or unsafe integer literal; use strings for large IDs"); } E::Literal(serde_json::json!(n.value)) }
            js::Expression::StringLiteral(s) => E::Literal(s.value.to_string().into()),
            js::Expression::Identifier(i) => E::Name(i.name.to_string()),
            js::Expression::ParenthesizedExpression(p) => return self.expr(&p.expression, depth+1),
            js::Expression::ArrayExpression(a) => {
                let mut values = Vec::new();
                for e in &a.elements { if matches!(e, js::ArrayExpressionElement::SpreadElement(_) | js::ArrayExpressionElement::Elision(_)) { return self.err(a.span, "Array spreads/holes are unsupported"); } values.push(self.expr(e.to_expression(), depth+1)?); }
                E::Array(values)
            }
            js::Expression::ObjectExpression(o) => {
                let mut out = Vec::new();
                for p in &o.properties {
                    let js::ObjectPropertyKind::ObjectProperty(p) = p else { return self.err(o.span, "Object spread unsupported"); };
                    if p.computed || p.method || p.kind != js::PropertyKind::Init { return self.err(p.span, "Computed keys, methods, accessors unsupported"); }
                    let key = match &p.key { js::PropertyKey::StaticIdentifier(i) => i.name.to_string(), js::PropertyKey::StringLiteral(s) => s.value.to_string(), js::PropertyKey::NumericLiteral(n) => n.value.to_string(), _ => return self.err(p.span, "Unsupported object key") };
                    if blocked(&key) { return self.err(p.span, "Prototype keys are forbidden"); }
                    out.push((key, self.expr(&p.value, depth+1)?));
                }
                E::Object(out)
            }
            js::Expression::StaticMemberExpression(m) => { if m.optional || blocked(&m.property.name) { return self.err(m.span, "Optional/prototype member access unsupported"); } E::Member(Box::new(self.expr(&m.object, depth+1)?), Box::new(Expr { at: m.span.start as usize, kind: E::Literal(m.property.name.to_string().into()) })) }
            js::Expression::ComputedMemberExpression(m) => {
                if m.optional { return self.err(m.span, "Optional access unsupported"); }
                let key = self.expr(&m.expression,depth+1)?;
                if matches!(&key.kind,E::Literal(serde_json::Value::String(s)) if blocked(s)) { return self.err(m.span,"Prototype member access is forbidden"); }
                E::Member(Box::new(self.expr(&m.object,depth+1)?),Box::new(key))
            }
            js::Expression::CallExpression(c) => {
                if c.optional { return self.err(c.span, "Optional calls unsupported"); }
                let mut args = Vec::new();
                for a in &c.arguments { if matches!(a, js::Argument::SpreadElement(_)) { return self.err(c.span, "Spread arguments unsupported"); } args.push(self.expr(a.to_expression(), depth+1)?); }
                E::Call(Box::new(self.expr(&c.callee, depth+1)?), args)
            }
            js::Expression::AwaitExpression(a) => E::Await(Box::new(self.expr(&a.argument, depth+1)?)),
            js::Expression::UnaryExpression(u) => { let op = u.operator.as_str(); if !matches!(op, "!" | "+" | "-" | "typeof" | "void") { return self.err(u.span, "Unsupported unary operator"); } E::Unary(op.into(), Box::new(self.expr(&u.argument, depth+1)?)) }
            js::Expression::BinaryExpression(b) => { let op = b.operator.as_str(); if !matches!(op, "+" | "-" | "*" | "/" | "%" | "**" | "===" | "!==" | "<" | "<=" | ">" | ">=") { return self.err(b.span, "Unsupported binary operator"); } E::Binary(op.into(), Box::new(self.expr(&b.left, depth+1)?), Box::new(self.expr(&b.right, depth+1)?)) }
            js::Expression::LogicalExpression(b) => E::Binary(b.operator.as_str().into(), Box::new(self.expr(&b.left, depth+1)?), Box::new(self.expr(&b.right, depth+1)?)),
            js::Expression::ConditionalExpression(c) => E::Conditional(Box::new(self.expr(&c.test, depth+1)?), Box::new(self.expr(&c.consequent, depth+1)?), Box::new(self.expr(&c.alternate, depth+1)?)),
            js::Expression::AssignmentExpression(a) => { let op = a.operator.as_str(); if !matches!(op, "=" | "+=" | "-=" | "*=" | "/=" | "%=") { return self.err(a.span, "Unsupported assignment operator"); } E::Assign(Box::new(self.target(&a.left, depth+1)?), op.into(), Box::new(self.expr(&a.right, depth+1)?)) }
            js::Expression::UpdateExpression(u) => {
                let target = match &u.argument { js::SimpleAssignmentTarget::AssignmentTargetIdentifier(i) if !reserved(&i.name) => Expr { at: u.span.start as usize, kind: E::Name(i.name.to_string()) }, _ => return self.err(u.span, "Updates require a local identifier") };
                E::Update(Box::new(target), u.operator.as_str() == "++", u.prefix)
            }
            js::Expression::ArrowFunctionExpression(f) => {
                let mut body = self.list(&f.body.statements, depth+1)?;
                if f.expression { if let Some(Stmt { kind: S::Expr(e), at }) = body.pop() { body.push(Stmt { at, kind: S::Return(Some(e)) }); } else { return self.err(f.span, "Invalid expression arrow"); } }
                E::Function(Arc::new(Function { params: self.params(&f.params)?, body, asynchronous: f.r#async }))
            }
            js::Expression::FunctionExpression(f) => { if f.id.is_some() { return self.err(f.span, "Named function expressions unsupported; use a declaration"); } E::Function(self.function(f, depth+1)?) }
            _ => return self.err(e.span(), "Unsupported expression (new, regex, templates, this, spread, optional chains, generators and imports are unavailable)"),
        };
        Ok(Expr {
            at: e.span().start as usize,
            kind,
        })
    }
}
