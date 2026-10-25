use super::*;

#[tokio::test]
async fn connect_catalog_http_failure_is_terminal_and_retry_succeeds_on_same_transport() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    for failed_path in ["/v2/providers", "/v2/providers/auth-methods"] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let paths = if failed_path == "/v2/providers" {
                vec!["/v2/providers", "/v2/providers", "/v2/providers/auth-methods"]
            } else {
                vec!["/v2/providers", "/v2/providers/auth-methods", "/v2/providers", "/v2/providers/auth-methods"]
            };
            let mut failed = false;
            for expected in paths {
                let (mut socket, _) = tokio::time::timeout(std::time::Duration::from_secs(3), listener.accept()).await.unwrap().unwrap();
                let mut request = Vec::new();
                let mut byte = [0];
                while !request.ends_with(b"\r\n\r\n") {
                    socket.read_exact(&mut byte).await.unwrap();
                    request.push(byte[0]);
                }
                let request = String::from_utf8(request).unwrap();
                assert!(request.starts_with(&format!("GET {expected} ")), "{request}");
                let status = if !failed && expected == failed_path { failed = true; 500 } else { 200 };
                socket.write_all(format!("HTTP/1.1 {status} Test\r\nContent-Length: 2\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{{}}").as_bytes()).await.unwrap();
            }
        });
        let client = reqwest::Client::new();
        let get = |path: &'static str| {
            let url = format!("{base}{path}");
            let client = client.clone();
            async move {
                client.get(url).send().await.map_err(|error| error.to_string())?
                    .error_for_status().map_err(|error| error.to_string())?
                    .json::<Value>().await.map_err(|error| error.to_string())
            }
        };
        let (tx, mut rx) = reply_queue::channel();
        // Same response channel/client for failure and retry. The correlation
        // belongs to the payload, not whichever outer command ID is newest.
        tx.send(connect_catalog_reply(Some("first".into()), &get).await).unwrap();
        assert!(matches!(rx.recv().await.unwrap(), AgentServerMessage::ConnectProviderCatalogFailed { request_id, error } if request_id == "first" && error.contains("500")));
        tx.send(connect_catalog_reply(Some("retry".into()), &get).await).unwrap();
        assert!(matches!(rx.recv().await.unwrap(), AgentServerMessage::ConnectProviderCatalog { request_id: Some(id), .. } if id == "retry"));
        assert!(rx.try_recv().is_err(), "one terminal response per request");
        server.await.unwrap();
    }
}

#[tokio::test]
async fn connect_catalog_legacy_failure_preserves_generic_error_form() {
    let reply = connect_catalog_reply(None, |_| std::future::ready(Err("HTTP 500".into()))).await;
    assert!(matches!(reply, AgentServerMessage::Error { message } if message == "HTTP 500"));
}
