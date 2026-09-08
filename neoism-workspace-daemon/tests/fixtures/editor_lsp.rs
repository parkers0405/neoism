//! Deterministic stdio LSP compiled by editor_lsp_ws.rs; std-only so this fixture
//! works on native Windows as well as Unix. Arguments are JSON-quoted file URIs.
use std::io::{BufRead, Read, Write};
fn send(value: String) {
    let mut out = std::io::stdout().lock();
    write!(out, "Content-Length: {}\r\n\r\n{}", value.len(), value).unwrap();
    out.flush().unwrap();
}
fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    let uri = &args[1];
    let other = &args[2];
    let log = &args[3];
    let mut stdin = std::io::stdin().lock();
    let range = r#"{"start":{"line":0,"character":3},"end":{"line":0,"character":9}}"#;
    let mut live = false;
    let mut completion_accepted = false;
    loop {
        let mut len = 0;
        loop {
            let mut line = String::new();
            if stdin.read_line(&mut line).unwrap() == 0 {
                return;
            }
            if line == "\r\n" || line == "\n" {
                break;
            }
            if let Some(n) = line.strip_prefix("Content-Length:") {
                len = n.trim().parse::<usize>().unwrap();
            }
        }
        let mut body = vec![0; len];
        stdin.read_exact(&mut body).unwrap();
        let body = String::from_utf8(body).unwrap();
        let method = body
            .split("\"method\":")
            .nth(1)
            .and_then(|v| v.trim_start().strip_prefix('"'))
            .and_then(|v| v.split('"').next())
            .unwrap_or("");
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log)
            .unwrap();
        writeln!(file, "{method} {body}").unwrap();
        if method == "exit" {
            return;
        }
        if method == "textDocument/didOpen" || method == "textDocument/didChange" {
            live |= body.contains("unsaved-host");
            completion_accepted = body.contains("completion-accepted");
            send(format!(
                r#"{{"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{{"uri":{uri},"diagnostics":[{{"range":{range},"severity":1,"message":"host diagnostic","source":"fixture"}}]}}}}"#
            ));
            send(format!(
                r#"{{"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{{"uri":{other},"diagnostics":[{{"range":{range},"severity":2,"message":"background diagnostic","source":"fixture"}}]}}}}"#
            ));
        }
        let Some(id) = body
            .split("\"id\":")
            .nth(1)
            .map(|v| {
                v.trim_start()
                    .chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect::<String>()
            })
            .filter(|id| !id.is_empty())
        else {
            continue;
        };
        if method == "workspace/executeCommand"
            && body.contains("completion-expected")
            && !completion_accepted
        {
            send(format!(
                r#"{{"jsonrpc":"2.0","id":{id},"error":{{"code":-32603,"message":"command ran before accepted completion text"}}}}"#
            ));
            continue;
        }
        if method == "workspace/executeCommand" && body.contains("force-error") {
            send(format!(
                r#"{{"jsonrpc":"2.0","id":{id},"error":{{"code":-32603,"message":"fixture command error"}}}}"#
            ));
            continue;
        }
        let edit = format!(
            r#"{{"changes":{{{uri}:[{{"range":{range},"newText":"renamed"}}],{other}:[{{"range":{range},"newText":"renamed"}}]}}}}"#
        );
        let result = match method {
            "initialize" => r#"{"capabilities":{"textDocumentSync":{"openClose":true,"change":1,"save":true},"completionProvider":{"triggerCharacters":[".",":"]},"hoverProvider":true,"definitionProvider":true,"referencesProvider":true,"signatureHelpProvider":{"triggerCharacters":["(",","]},"documentSymbolProvider":true,"documentHighlightProvider":true,"codeActionProvider":{"resolveProvider":true},"renameProvider":true,"documentFormattingProvider":true,"executeCommandProvider":{"commands":["fixture.finish"]}}}"#.into(),
            "textDocument/completion" => r#"[{"label":"host_completion","kind":3,"insertText":"host_completion()"}]"#.into(),
            "textDocument/hover" => format!(r#"{{"contents":{{"kind":"markdown","value":"{}"}}}}"#, if live { "unsaved-host hover" } else { "stale disk hover" }),
            "textDocument/definition" | "textDocument/references" => format!(r#"[{{"uri":{uri},"range":{range}}}]"#),
            "textDocument/signatureHelp" => r#"{"signatures":[{"label":"shared(value)","parameters":[{"label":"value"}]}],"activeSignature":0,"activeParameter":0}"#.into(),
            "textDocument/documentSymbol" => format!(r#"[{{"name":"shared","kind":12,"range":{range},"selectionRange":{range}}}]"#),
            "textDocument/documentHighlight" => format!(r#"[{{"range":{range},"kind":1}}]"#),
            "textDocument/codeAction" => r#"[{"title":"Host fix","kind":"quickfix","isPreferred":true,"data":{"fix":1}}]"#.into(),
            "codeAction/resolve" => format!(r#"{{"title":"Host fix","edit":{edit},"command":{{"title":"Finish","command":"fixture.finish"}}}}"#),
            "textDocument/rename" => edit,
            "textDocument/formatting" => format!(r#"[{{"range":{range},"newText":"formatted"}}]"#),
            _ => "null".into(),
        };
        send(format!(
            r#"{{"jsonrpc":"2.0","id":{id},"result":{result}}}"#
        ));
    }
}
