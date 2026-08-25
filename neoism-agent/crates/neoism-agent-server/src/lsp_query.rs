use std::path::Path;

#[cfg(test)]
pub(super) fn query_workspace_symbols_with_command(
    root: &Path,
    query: &str,
    command: &[String],
    language: &str,
) -> anyhow::Result<Vec<super::WorkspaceSymbol>> {
    use serde_json::json;

    use super::{
        lsp_client::StdioLspClient, lsp_parse::parse_workspace_symbols, SYMBOL_TIMEOUT,
    };

    let mut client = StdioLspClient::spawn(root, command)?;
    let initialized = client.initialize(root)?;
    if !initialized.workspace_symbol_provider {
        let _ = client.shutdown();
        return Ok(Vec::new());
    }
    let result = client.request(
        "workspace/symbol",
        json!({ "query": query }),
        SYMBOL_TIMEOUT,
    )?;
    let _ = client.shutdown();
    Ok(parse_workspace_symbols(root, language, result))
}
