//! Per-request HTTP SSE reader. No background tasks and no unbounded delivery queue.

use std::time::Duration;

use axum::body::Bytes;
use tokio::time::{timeout_at, Instant};

use super::{
    decode_push_frame, EventError, InvalidWire, PushFrame, PushMessage, MAX_JSON_BYTES,
};

const MAX_HEARTBEAT_MS: u64 = 5 * 60 * 1000;
const MISSED_HEARTBEATS: u32 = 3;

#[derive(Debug)]
pub(crate) enum EventStreamError {
    Wire(InvalidWire),
    HttpStatus(reqwest::StatusCode),
    Http(reqwest::Error),
    MediaType,
    HandshakeTimeout,
    HeartbeatTimeout,
    ExpectedActive,
    HeartbeatGrant,
    UnexpectedEventName,
    Disconnected,
    ReconnectNotAllowed,
    Rejected(i64),
    Failed(i64),
    Terminated(EventError),
}

impl std::fmt::Display for EventStreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Wire(error) => write!(f, "invalid MCP event stream: {error}"),
            Self::HttpStatus(status) => {
                write!(f, "MCP event stream returned HTTP {status}")
            }
            Self::Http(error) => write!(f, "MCP event stream HTTP I/O failed: {error}"),
            Self::MediaType => f.write_str("MCP event stream did not return SSE"),
            Self::HandshakeTimeout => {
                f.write_str("MCP event stream timed out waiting for active")
            }
            Self::HeartbeatTimeout => {
                f.write_str("MCP event stream missed three heartbeat intervals")
            }
            Self::ExpectedActive => {
                f.write_str("MCP event stream did not start with active")
            }
            Self::HeartbeatGrant => {
                f.write_str("MCP event stream heartbeat grant exceeds the local limit")
            }
            Self::UnexpectedEventName => {
                f.write_str("MCP stream event name does not match the subscription")
            }
            Self::Disconnected => {
                f.write_str("MCP event stream ended without a correlated completion")
            }
            Self::ReconnectNotAllowed => f.write_str(
                "MCP event stream failure is not eligible for guarded reconnect",
            ),
            Self::Rejected(code) => {
                write!(f, "MCP event stream rejected (JSON-RPC {code})")
            }
            Self::Failed(code) => write!(f, "MCP event stream failed (JSON-RPC {code})"),
            Self::Terminated(error) => {
                write!(f, "MCP event stream terminated (code {})", error.code)
            }
        }
    }
}

impl std::error::Error for EventStreamError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Wire(error) => Some(error),
            Self::Http(error) => Some(error),
            _ => None,
        }
    }
}

impl From<InvalidWire> for EventStreamError {
    fn from(value: InvalidWire) -> Self {
        Self::Wire(value)
    }
}

impl EventStreamError {
    pub(crate) fn can_reconnect(&self) -> bool {
        match self {
            Self::Disconnected | Self::HeartbeatTimeout => true,
            Self::Http(error) => error.is_body() || error.is_timeout(),
            _ => false,
        }
    }
}

fn is_sse(response: &reqwest::Response) -> bool {
    response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("text/event-stream"))
}

/// Pull-based subscription. Poll `next` continuously while consuming. Dropping the
/// handle (including `cancel`) aborts the per-request HTTP stream; no RPC is sent.
/// The caller owns checkpointing and can reopen with its last *committed* cursor.
pub(crate) struct HttpMcpEventStream {
    reader: Option<SseReader>,
    id: u64,
    name: String,
    deadline: Instant,
    silence: Option<Duration>,
    initial: Option<PushMessage>,
}

impl HttpMcpEventStream {
    pub(crate) async fn open(
        request: reqwest::RequestBuilder,
        id: u64,
        name: String,
        handshake: Duration,
    ) -> Result<Self, EventStreamError> {
        let deadline = Instant::now() + handshake;
        let response = timeout_at(deadline, request.send())
            .await
            .map_err(|_| EventStreamError::HandshakeTimeout)?
            .map_err(|error| EventStreamError::Http(error.without_url()))?;
        if !response.status().is_success() {
            return Err(EventStreamError::HttpStatus(response.status()));
        }
        if !is_sse(&response) {
            // A pre-open refusal may be a finite application/json JSON-RPC error.
            let bytes = timeout_at(deadline, read_finite(response))
                .await
                .map_err(|_| EventStreamError::HandshakeTimeout)??;
            return match decode_push_frame(id, &bytes) {
                Err(InvalidWire::Rpc(code)) => Err(EventStreamError::Rejected(code)),
                Err(InvalidWire::MismatchedResponse) => {
                    Err(InvalidWire::MismatchedResponse.into())
                }
                _ => Err(EventStreamError::MediaType),
            };
        }
        let mut stream = Self {
            reader: Some(SseReader::new(response)),
            id,
            name,
            deadline,
            silence: None,
            initial: None,
        };
        stream.initial = stream.advance().await?;
        Ok(stream)
    }

    /// Returns the initial `Active` first; then stream messages in wire order.
    /// `None` on a correlated successful result; errors and completion close the reader.
    pub(crate) async fn next(&mut self) -> Result<Option<PushMessage>, EventStreamError> {
        if self.initial.is_some() {
            return Ok(self.initial.take());
        }
        if self.reader.is_none() {
            return Ok(None);
        }
        let result = self.advance().await;
        if !matches!(result, Ok(Some(_))) {
            self.reader = None;
        }
        result
    }

    pub(crate) fn cancel(self) {
        drop(self);
    }

    async fn advance(&mut self) -> Result<Option<PushMessage>, EventStreamError> {
        let reader = self.reader.as_mut().expect("open event stream");
        loop {
            let expired = || {
                if self.silence.is_some() {
                    EventStreamError::HeartbeatTimeout
                } else {
                    EventStreamError::HandshakeTimeout
                }
            };
            if Instant::now() >= self.deadline {
                return Err(expired());
            }
            let frame = timeout_at(self.deadline, reader.next_frame())
                .await
                .map_err(|_| expired())??;
            let Some(frame) = frame else {
                return Err(EventStreamError::Disconnected);
            };
            match decode_push_frame(self.id, &frame) {
                Ok(PushFrame::Unrelated) => continue,
                Ok(PushFrame::Completed) if self.silence.is_some() => return Ok(None),
                Ok(PushFrame::Completed) => return Err(EventStreamError::ExpectedActive),
                Err(InvalidWire::Rpc(code)) if self.silence.is_none() => {
                    return Err(EventStreamError::Rejected(code))
                }
                Err(InvalidWire::Rpc(code)) => {
                    return Err(EventStreamError::Failed(code))
                }
                Err(error) => return Err(error.into()),
                Ok(PushFrame::Notification(PushMessage::Terminated(error))) => {
                    return Err(EventStreamError::Terminated(error))
                }
                Ok(PushFrame::Notification(message)) => {
                    if let PushMessage::Active(active) = &message {
                        if active.heartbeat_ms > MAX_HEARTBEAT_MS {
                            return Err(EventStreamError::HeartbeatGrant);
                        }
                        self.silence = Some(
                            Duration::from_millis(active.heartbeat_ms)
                                * MISSED_HEARTBEATS,
                        );
                    }
                    let silence = self.silence.ok_or(EventStreamError::ExpectedActive)?;
                    if matches!(&message, PushMessage::Event(event) if event.name != self.name)
                    {
                        return Err(EventStreamError::UnexpectedEventName);
                    }
                    // Only a valid, correlated message counts as activity. SSE comments,
                    // unrelated traffic and incomplete frames do not reset the clock.
                    self.deadline = Instant::now() + silence;
                    return Ok(Some(message));
                }
            }
        }
    }
}

async fn read_finite(
    mut response: reqwest::Response,
) -> Result<Vec<u8>, EventStreamError> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| EventStreamError::Http(error.without_url()))?
    {
        if chunk.len() > MAX_JSON_BYTES - bytes.len() {
            return Err(InvalidWire::TooLarge.into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

struct SseReader {
    response: reqwest::Response,
    chunk: Bytes,
    line: Vec<u8>,
    data: Vec<u8>,
    frame_bytes: usize,
    skip_lf: bool,
    first_line: bool,
}

impl SseReader {
    fn new(response: reqwest::Response) -> Self {
        Self {
            response,
            chunk: Bytes::new(),
            line: Vec::new(),
            data: Vec::new(),
            frame_bytes: 0,
            skip_lf: false,
            first_line: true,
        }
    }

    async fn next_frame(&mut self) -> Result<Option<Vec<u8>>, EventStreamError> {
        loop {
            if self.chunk.is_empty() {
                let Some(chunk) = self
                    .response
                    .chunk()
                    .await
                    .map_err(|error| EventStreamError::Http(error.without_url()))?
                else {
                    return Ok(None);
                };
                self.chunk = chunk;
                if self.chunk.is_empty() {
                    continue;
                }
            }
            if std::mem::take(&mut self.skip_lf) && self.chunk.first() == Some(&b'\n') {
                let _ = self.chunk.split_to(1);
                if self.chunk.is_empty() {
                    continue;
                }
            }
            let end = self.chunk.iter().position(|b| matches!(b, b'\r' | b'\n'));
            let len = end.unwrap_or(self.chunk.len());
            let cost = len + usize::from(end.is_some());
            if cost > MAX_JSON_BYTES - self.frame_bytes {
                return Err(InvalidWire::TooLarge.into());
            }
            self.frame_bytes += cost;
            self.line.extend_from_slice(&self.chunk.split_to(len));
            if end.is_none() {
                continue;
            }
            self.skip_lf = self.chunk.split_to(1)[0] == b'\r';
            let line = if std::mem::take(&mut self.first_line) {
                self.line
                    .strip_prefix(b"\xef\xbb\xbf")
                    .unwrap_or(&self.line)
            } else {
                &self.line
            };
            if line.is_empty() {
                self.frame_bytes = 0;
                self.line.clear();
                if self.data.iter().all(u8::is_ascii_whitespace) {
                    self.data.clear();
                } else {
                    self.data.pop(); // SSE joins data lines with LF and drops the last LF.
                    return Ok(Some(std::mem::take(&mut self.data)));
                }
                // Yield even on endless buffered comments so the deadline can fire.
                tokio::task::consume_budget().await;
                continue;
            }
            if let Some(data) = line.strip_prefix(b"data:") {
                self.data
                    .extend_from_slice(data.strip_prefix(b" ").unwrap_or(data));
                self.data.push(b'\n');
            } else if line == b"data" {
                self.data.push(b'\n');
            }
            self.line.clear();
        }
    }
}
