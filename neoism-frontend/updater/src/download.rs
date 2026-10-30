//! Retry interrupted HTTP transfers without ever installing unverified bytes.
use crate::Error;
use futures::StreamExt;
use sha2::{Digest, Sha256};
use std::{path::Path, time::Duration};
use tokio::io::AsyncWriteExt;

pub fn error_chain(error: &dyn std::error::Error) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(error) = source {
        message.push_str(": ");
        message.push_str(&error.to_string());
        source = error.source();
    }
    message
}

fn transient(error: &reqwest::Error) -> bool {
    error.is_timeout()
        || error.is_connect()
        || error.is_body()
        || error.is_decode()
        || error.status().is_some_and(|s| {
            s.is_server_error() || s.as_u16() == 408 || s.as_u16() == 429
        })
}

pub async fn verified(
    client: &reqwest::Client,
    url: &str,
    path: &Path,
    expected: &str,
) -> Result<(), Error> {
    transfer(client, url, path, Some(expected), Duration::from_secs(1)).await
}

pub async fn text(client: &reqwest::Client, url: &str) -> Result<String, Error> {
    let staging = tempfile::tempdir()?;
    let path = staging.path().join("response");
    transfer(client, url, &path, None, Duration::from_secs(1)).await?;
    Ok(tokio::fs::read_to_string(path).await?)
}

async fn transfer(
    client: &reqwest::Client,
    url: &str,
    path: &Path,
    expected: Option<&str>,
    delay: Duration,
) -> Result<(), Error> {
    const ATTEMPTS: u32 = 4;
    for attempt in 1..=ATTEMPTS {
        let mut received = 0u64;
        let result: Result<(), Error> = async {
            let response = client.get(url).send().await?.error_for_status()?;
            // Each retry starts clean: never append a full response to a partial MSI.
            let mut file = tokio::fs::File::create(path).await?;
            let mut hash = Sha256::new();
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk?;
                file.write_all(&chunk).await?;
                received += chunk.len() as u64;
                hash.update(&chunk);
            }
            file.flush().await?;
            if expected.is_some_and(|expected| {
                !format!("{:x}", hash.finalize()).eq_ignore_ascii_case(expected)
            }) {
                return Err(
                    "Release checksum mismatch; installation was not changed".into()
                );
            }
            Ok(())
        }
        .await;
        match result {
            Ok(()) => return Ok(()),
            Err(error) => {
                let retry = error
                    .downcast_ref::<reqwest::Error>()
                    .is_some_and(transient);
                // Close the stream/file above before removing it, including on Windows.
                let _ = tokio::fs::remove_file(path).await;
                let context = format!("Download {url} failed on attempt {attempt}/{ATTEMPTS} after {received} bytes: {}", error_chain(error.as_ref()));
                if !retry || attempt == ATTEMPTS {
                    return Err(context.into());
                }
                eprintln!("{context}. Retrying…");
                tokio::time::sleep(delay * (1 << (attempt - 1))).await;
            }
        }
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    // A real TCP server deliberately cuts off response bodies, reproducing #7.
    fn server(responses: Vec<&'static str>) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!(
            "http://{}/Neoism-x86_64.msi",
            listener.local_addr().unwrap()
        );
        let handle = thread::spawn(move || {
            for response in responses {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = Vec::new();
                let mut byte = [0];
                while !request.ends_with(b"\r\n\r\n") {
                    if stream.read(&mut byte).unwrap() == 0 {
                        break;
                    }
                    request.push(byte[0]);
                }
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
        (url, handle)
    }
    const CUT: &str =
        "HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\npartial";
    const OK: &str =
        "HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\ncomplete";
    async fn run(
        responses: Vec<&'static str>,
        expected: &str,
    ) -> (Result<(), Error>, tempfile::TempDir) {
        let (url, server) = server(responses);
        let dir = tempfile::tempdir().unwrap();
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap();
        let result = transfer(
            &client,
            &url,
            &dir.path().join("installer"),
            Some(expected),
            Duration::ZERO,
        )
        .await;
        server.join().unwrap();
        (result, dir)
    }
    #[tokio::test]
    async fn interrupted_msi_retries_without_corrupting_payload() {
        let hash = format!("{:x}", Sha256::digest(b"complete"));
        let (result, dir) = run(vec![CUT, "HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", OK], &hash).await;
        result.unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("installer")).unwrap(),
            b"complete"
        );
    }
    #[tokio::test]
    async fn exhausted_retries_explain_transport_failure_and_remove_partial_msi() {
        let (result, dir) = run(vec![CUT; 4], "unused").await;
        let message = result.unwrap_err().to_string();
        assert!(message.contains("attempt 4/4"), "{message}");
        assert!(message.contains("Neoism-x86_64.msi"), "{message}");
        assert!(
            message.contains("body") && message.contains("after "),
            "{message}"
        );
        assert!(
            message.matches(": ").count() >= 2,
            "source chain missing: {message}"
        );
        assert!(!dir.path().join("installer").exists());
    }
    #[tokio::test]
    async fn checksum_mismatch_and_permanent_http_errors_are_not_retried() {
        for responses in [vec![OK], vec!["HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"]] {
            let (result, dir) = run(responses, "wrong checksum").await;
            assert!(result.unwrap_err().to_string().contains("attempt 1/4"));
            assert!(!dir.path().join("installer").exists());
        }
    }
}
