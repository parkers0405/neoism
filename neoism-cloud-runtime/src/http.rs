use crate::*;
use reqwest::{
    header::{HeaderMap, HeaderValue, AUTHORIZATION},
    Client, StatusCode,
};
use std::time::Duration;
use url::Url;

/// Generic v2 bridge, not a vendor-native API. No Debug implementation deliberately.
/// Credentials are held only in the HTTP client's sensitive default header.
pub struct HttpProvider {
    id: String,
    endpoint: Url,
    client: Client,
    capabilities: Capabilities,
}
impl HttpProvider {
    pub fn new(
        id: &str,
        endpoint: &str,
        bearer: &str,
        timeout: Duration,
        capabilities: Capabilities,
    ) -> Result<Self> {
        Self::build(id, endpoint, bearer, timeout, capabilities, false)
    }
    /// Explicit opt-in for private plaintext bridges/testing. Never supplies a default endpoint.
    pub fn new_plaintext(
        id: &str,
        endpoint: &str,
        bearer: &str,
        timeout: Duration,
        capabilities: Capabilities,
    ) -> Result<Self> {
        Self::build(id, endpoint, bearer, timeout, capabilities, true)
    }
    fn build(
        id: &str,
        endpoint: &str,
        bearer: &str,
        timeout: Duration,
        capabilities: Capabilities,
        plaintext: bool,
    ) -> Result<Self> {
        if endpoint.is_empty()
            || endpoint.len() > 2048
            || endpoint
                .chars()
                .any(|c| c.is_control() || c.is_whitespace())
            || endpoint.contains('\\')
        {
            return Err(Error::Invalid);
        }
        let mut endpoint = Url::parse(endpoint).map_err(|_| Error::Invalid)?;
        if !valid_id(id)
            || timeout.is_zero()
            || timeout > Duration::from_secs(600)
            || endpoint.host_str().is_none()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
            || !(endpoint.scheme() == "https" || plaintext && endpoint.scheme() == "http")
            || bearer.is_empty()
            || bearer.len() > 8192
            || !bearer.bytes().all(|b| b.is_ascii_graphic())
        {
            return Err(Error::Invalid);
        }
        if !endpoint.path().ends_with('/') {
            endpoint.set_path(&format!("{}/", endpoint.path()));
        }
        let mut headers = HeaderMap::new();
        let mut auth = HeaderValue::from_str(&format!("Bearer {bearer}"))
            .map_err(|_| Error::Invalid)?;
        auth.set_sensitive(true);
        headers.insert(AUTHORIZATION, auth);
        let client = Client::builder()
            .default_headers(headers)
            .timeout(timeout)
            .connect_timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .map_err(|_| {
                Error::Provider(ProviderError::new(FailureCode::Transport, false))
            })?;
        Ok(Self {
            id: id.into(),
            endpoint,
            client,
            capabilities,
        })
    }
    async fn request(
        &self,
        action: RuntimeAction,
        allocation: &Allocation,
        handle: Option<&MachineHandle>,
    ) -> std::result::Result<MachineStatus, ProviderError> {
        let request = ProtocolRequest::new(action, allocation.clone(), handle.cloned())?;
        if allocation.provider != self.id {
            return Err(ProviderError::new(FailureCode::Identity, false));
        }
        let url = self
            .endpoint
            .join(&format!("v2/runtime/{}", action.as_str()))
            .map_err(|_| ProviderError::new(FailureCode::Protocol, false))?;
        let mut response = self
            .client
            .post(url)
            .json(&request)
            .send()
            .await
            .map_err(transport)?;
        let status = response.status();
        if status != StatusCode::OK && status != StatusCode::ACCEPTED {
            let (code, retryable) = match status.as_u16() {
                401 | 403 => (FailureCode::Unauthorized, false),
                404 => (FailureCode::NotFound, false),
                409 | 412 => (FailureCode::Conflict, false),
                408 | 504 => (FailureCode::Timeout, true),
                429 | 500..=599 => (FailureCode::Unavailable, true),
                _ => (FailureCode::Rejected, false),
            };
            // 404 is not proof of owned destruction: bridge must return an identity-checked tombstone.
            return Err(ProviderError::new(code, retryable));
        }
        if response.content_length().is_some_and(|n| n > 65_536) {
            return Err(ProviderError::new(FailureCode::Protocol, false));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(transport)? {
            if bytes.len() + chunk.len() > 65_536 {
                return Err(ProviderError::new(FailureCode::Protocol, false));
            }
            bytes.extend_from_slice(&chunk);
        }
        let response: ProtocolResponse = serde_json::from_slice(&bytes)
            .map_err(|_| ProviderError::new(FailureCode::Protocol, false))?;
        response.validate(allocation, handle)?;
        Ok(response.status)
    }
}
fn transport(error: reqwest::Error) -> ProviderError {
    ProviderError::new(
        if error.is_timeout() {
            FailureCode::Timeout
        } else {
            FailureCode::Transport
        },
        true,
    )
}
impl RuntimeProvider for HttpProvider {
    fn id(&self) -> &str {
        &self.id
    }
    fn capabilities(&self) -> Capabilities {
        self.capabilities
    }
    fn ensure<'a>(&'a self, a: &'a Allocation) -> ProviderFuture<'a, MachineStatus> {
        Box::pin(self.request(RuntimeAction::Ensure, a, None))
    }
    fn start<'a>(
        &'a self,
        a: &'a Allocation,
        h: &'a MachineHandle,
    ) -> ProviderFuture<'a, MachineStatus> {
        Box::pin(self.request(RuntimeAction::Start, a, Some(h)))
    }
    fn inspect<'a>(
        &'a self,
        a: &'a Allocation,
        h: Option<&'a MachineHandle>,
    ) -> ProviderFuture<'a, MachineStatus> {
        Box::pin(self.request(RuntimeAction::Inspect, a, h))
    }
    fn stop<'a>(
        &'a self,
        a: &'a Allocation,
        h: &'a MachineHandle,
    ) -> ProviderFuture<'a, MachineStatus> {
        Box::pin(self.request(RuntimeAction::Stop, a, Some(h)))
    }
    fn destroy<'a>(
        &'a self,
        a: &'a Allocation,
        h: &'a MachineHandle,
    ) -> ProviderFuture<'a, MachineStatus> {
        Box::pin(self.request(RuntimeAction::Destroy, a, Some(h)))
    }
}
