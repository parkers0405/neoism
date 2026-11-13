//! Native binding only: execution remains in neoism-agent-server.

use axum::{
    body::{Body, Bytes},
    http::{Method, Request, Uri},
};
use futures::{stream::BoxStream, StreamExt};
use napi::{bindgen_prelude::Buffer, Error, Result, Status};
use napi_derive::napi;
use neoism_agent_server::{EmbeddedAgent, EmbeddedOptions};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio_util::sync::CancellationToken;

const MAX_REQUEST_BYTES: usize = 16 * 1024 * 1024;
const MAX_REQUESTS: usize = 64;
const CHUNK_BYTES: usize = 64 * 1024;

#[napi]
pub fn binding_version() -> u32 {
    2
}

#[napi(object)]
pub struct NativeOptions {
    pub database: String,
    pub shutdown_timeout_ms: Option<u32>,
    pub config_json: Option<String>,
}

#[napi(object)]
pub struct Header {
    pub name: String,
    pub value: String,
}

#[napi(object)]
pub struct ResponseHead {
    pub status: u32,
    pub headers: Vec<Header>,
}

struct BodyState {
    stream: BoxStream<'static, std::result::Result<Bytes, axum::Error>>,
    pending: Bytes,
}

struct Exchange {
    cancel: CancellationToken,
    body: tokio::sync::Mutex<Option<BodyState>>,
    dispatched: AtomicBool,
}

#[napi]
pub struct NativeAgent {
    host: EmbeddedAgent,
    requests: Arc<Mutex<HashMap<u32, Arc<Exchange>>>>,
    closed: AtomicBool,
}

fn failure(message: impl ToString) -> Error {
    Error::new(Status::GenericFailure, message.to_string())
}

#[napi]
impl NativeAgent {
    #[napi(factory)]
    pub async fn open(options: NativeOptions) -> Result<Self> {
        let mut opts = EmbeddedOptions::new(
            options.database,
            neoism_agent_server::standard_services(),
        );
        if let Some(timeout) = options.shutdown_timeout_ms {
            opts.shutdown_timeout = Duration::from_millis(u64::from(timeout));
        }
        if let Some(config) = options.config_json {
            opts = opts.with_config(serde_json::from_str(&config).map_err(failure)?).map_err(failure)?;
        }
        Ok(Self {
            host: EmbeddedAgent::open(opts).await.map_err(failure)?,
            requests: Arc::new(Mutex::new(HashMap::new())),
            closed: AtomicBool::new(false),
        })
    }

    /// Synchronous admission makes AbortSignal delivery safe before the async
    /// Rust request future has been polled by the native runtime.
    #[napi]
    pub fn begin_request(&self, id: u32) -> Result<()> {
        let mut requests = self.requests.lock().map_err(failure)?;
        if self.closed.load(Ordering::Acquire) {
            return Err(failure("embedded host is closed"));
        }
        if requests.len() >= MAX_REQUESTS {
            return Err(failure("too many embedded requests"));
        }
        if requests.contains_key(&id) {
            return Err(failure("request ID is already active"));
        }
        requests.insert(
            id,
            Arc::new(Exchange {
                cancel: CancellationToken::new(),
                body: tokio::sync::Mutex::new(None),
                dispatched: AtomicBool::new(false),
            }),
        );
        Ok(())
    }

    #[napi]
    pub fn cancel_request(&self, id: u32) -> Result<()> {
        if let Some(exchange) = self.requests.lock().map_err(failure)?.remove(&id) {
            exchange.cancel.cancel();
        }
        Ok(())
    }

    #[napi]
    pub async fn request(
        &self,
        id: u32,
        method: String,
        path: String,
        headers: Vec<Header>,
        body: Buffer,
    ) -> Result<ResponseHead> {
        let exchange = self
            .requests
            .lock()
            .map_err(failure)?
            .get(&id)
            .cloned()
            .ok_or_else(|| failure("request cancelled or not admitted"))?;
        if exchange.dispatched.swap(true, Ordering::AcqRel) {
            return Err(failure("request already dispatched"));
        }
        if body.len() > MAX_REQUEST_BYTES {
            self.cancel_request(id)?;
            return Err(failure("embedded request exceeds 16 MiB"));
        }
        let bytes = body.to_vec();
        drop(body);
        let result = async {
            let uri: Uri = path.parse().map_err(failure)?;
            if uri.scheme().is_some() || uri.authority().is_some() || !path.starts_with('/') {
                return Err(failure("embedded request must use an origin-relative path"));
            }
            let mut request = Request::builder().method(Method::from_bytes(method.as_bytes()).map_err(failure)?).uri(uri);
            for header in headers { request = request.header(header.name, header.value); }
            let request = request.body(Body::from(bytes)).map_err(failure)?;
            let response = tokio::select! {
                biased;
                _ = exchange.cancel.cancelled() => return Err(failure("request cancelled")),
                result = self.host.request(request) => result.map_err(failure)?,
            };
            let status = response.status().as_u16().into();
            let headers = response.headers().iter().map(|(name, value)| {
                Ok(Header { name: name.to_string(), value: value.to_str().map_err(failure)?.to_owned() })
            }).collect::<Result<Vec<_>>>()?;
            *exchange.body.lock().await = Some(BodyState {
                stream: response.into_body().into_data_stream().boxed(), pending: Bytes::new(),
            });
            Ok(ResponseHead { status, headers })
        }.await;
        if result.is_err() {
            self.cancel_request(id)?;
        }
        result
    }

    /// Pull-based body delivery bounds prefetch and preserves streaming/backpressure.
    #[napi]
    pub async fn read(&self, id: u32) -> Result<Option<Buffer>> {
        let exchange = self
            .requests
            .lock()
            .map_err(failure)?
            .get(&id)
            .cloned()
            .ok_or_else(|| failure("response cancelled or closed"))?;
        let result = async {
            let mut guard = exchange.body.lock().await;
            let body = guard.as_mut().ok_or_else(|| failure("response is not ready"))?;
            loop {
                if exchange.cancel.is_cancelled() { return Err(failure("response cancelled")); }
                if !body.pending.is_empty() {
                    let length = body.pending.len().min(CHUNK_BYTES);
                    return Ok(Some(Buffer::from(body.pending.split_to(length).to_vec())));
                }
                match tokio::select! {
                    biased;
                    _ = exchange.cancel.cancelled() => return Err(failure("response cancelled")),
                    result = body.stream.next() => result,
                } {
                    Some(Ok(bytes)) => body.pending = bytes,
                    Some(Err(error)) => return Err(failure(error)),
                    None => return Ok(None),
                }
            }
        }.await;
        if !matches!(&result, Ok(Some(_))) {
            self.cancel_request(id)?;
        }
        result
    }

    #[napi]
    pub async fn close(&self) -> Result<()> {
        self.closed.store(true, Ordering::Release);
        let exchanges = std::mem::take(&mut *self.requests.lock().map_err(failure)?);
        for (_, exchange) in exchanges {
            exchange.cancel.cancel();
        }
        self.host.close().await.map_err(failure)
    }
}
