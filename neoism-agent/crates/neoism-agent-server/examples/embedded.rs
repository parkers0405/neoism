use axum::{
    body::{to_bytes, Body},
    http::Request,
};
use neoism_agent_server::{EmbeddedAgent, EmbeddedOptions};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args_os().skip(1);
    let database = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("usage: embedded DATABASE WORKSPACE"))?;
    let directory = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("usage: embedded DATABASE WORKSPACE"))?;
    let directory = std::path::PathBuf::from(directory).canonicalize()?;
    let host = EmbeddedAgent::open(EmbeddedOptions::new(
        database,
        neoism_agent_server::standard_services(),
    ))
    .await?;
    let result = async {
        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("directory", &directory.to_string_lossy())
            .finish();
        let mut request = Request::builder()
            .method("POST")
            .uri(format!("/v2/sessions?{query}"))
            .header("content-type", "application/json");
        if let Ok(token) = std::env::var("NEOISM_AGENT_TOKEN") {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        let response = host
            .request(request.body(Body::from(r#"{"title":"Embedded application"}"#))?)
            .await?;
        anyhow::ensure!(
            response.status().is_success(),
            "session creation returned {}",
            response.status()
        );
        let session: neoism_agent_core::SessionInfo =
            serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await?)?;
        println!(
            "Created session {} through the existing API without a listener",
            session.id
        );
        anyhow::Ok(())
    }
    .await;
    host.close().await?;
    result
}
