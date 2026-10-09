//! Fixed-route, authenticated provider bridge; never exposes provider-private data.
use crate::*;
use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use std::{sync::Arc, time::Duration};

/// No privileged fallback: implementations must authenticate the bearer and authorize
/// this exact tenant/workspace and operation. Failures must contain only public codes.
pub trait BridgeAuthorizer: Send + Sync {
    fn authorize<'a>(
        &'a self,
        bearer: &'a str,
        owner: &'a WorkspaceKey,
        action: RuntimeAction,
    ) -> ProviderFuture<'a, ()>;
}
#[derive(Clone)]
struct Bridge {
    provider: Arc<dyn RuntimeProvider>,
    authorizer: Arc<dyn BridgeAuthorizer>,
}
/// Mount at the control-plane origin. Only these five v2 routes are registered.
/// Calls are bounded to 30 seconds, never automatically retried. Providers must make
/// mutation cancellation/replay safe using the allocation's durable generation key.
pub fn provider_router(
    provider: Arc<dyn RuntimeProvider>,
    authorizer: Arc<dyn BridgeAuthorizer>,
) -> Router {
    Router::new()
        .route("/v2/runtime/ensure", post(ensure))
        .route("/v2/runtime/start", post(start))
        .route("/v2/runtime/inspect", post(inspect))
        .route("/v2/runtime/stop", post(stop))
        .route("/v2/runtime/destroy", post(destroy))
        .layer(DefaultBodyLimit::max(65_536))
        .with_state(Bridge {
            provider,
            authorizer,
        })
}
macro_rules! handler {
    ($name:ident, $action:ident) => {
        async fn $name(
            State(state): State<Bridge>,
            headers: HeaderMap,
            bytes: Bytes,
        ) -> Response {
            dispatch(state, headers, bytes, RuntimeAction::$action).await
        }
    };
}
handler!(ensure, Ensure);
handler!(start, Start);
handler!(inspect, Inspect);
handler!(stop, Stop);
handler!(destroy, Destroy);

async fn dispatch(
    state: Bridge,
    headers: HeaderMap,
    bytes: Bytes,
    action: RuntimeAction,
) -> Response {
    let auths = headers.get_all(axum::http::header::AUTHORIZATION);
    let mut values = auths.iter();
    let bearer = values
        .next()
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "));
    let Some(bearer) = bearer.filter(|s| {
        !s.is_empty() && s.len() <= 8192 && s.bytes().all(|b| b.is_ascii_graphic())
    }) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if values.next().is_some() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let request: ProtocolRequest = match serde_json::from_slice(&bytes) {
        Ok(r) => r,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    if request.validate(action).is_err() {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let a = &request.allocation;
    let authorization = tokio::time::timeout(
        Duration::from_secs(30),
        state.authorizer.authorize(bearer, &a.owner, action),
    )
    .await;
    if !matches!(authorization, Ok(Ok(()))) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if a.provider != state.provider.id() {
        return StatusCode::CONFLICT.into_response();
    }
    let call = async {
        let h = request.handle.as_ref();
        match action {
            RuntimeAction::Ensure => state.provider.ensure(a).await,
            RuntimeAction::Inspect => state.provider.inspect(a, h).await,
            RuntimeAction::Start => {
                state.provider.start(a, h.expect("validated handle")).await
            }
            RuntimeAction::Stop => {
                state.provider.stop(a, h.expect("validated handle")).await
            }
            RuntimeAction::Destroy => {
                state
                    .provider
                    .destroy(a, h.expect("validated handle"))
                    .await
            }
        }
    };
    let result = tokio::time::timeout(Duration::from_secs(30), call).await;
    match result {
        Ok(Ok(status)) => {
            let response = ProtocolResponse::new(status);
            if response.validate(a, request.handle.as_ref()).is_err() {
                return StatusCode::BAD_GATEWAY.into_response();
            }
            Json(response).into_response()
        }
        Ok(Err(error)) => match error.code {
            FailureCode::Unauthorized => StatusCode::FORBIDDEN,
            FailureCode::NotFound => StatusCode::NOT_FOUND,
            FailureCode::Conflict | FailureCode::Identity => StatusCode::CONFLICT,
            FailureCode::Rejected | FailureCode::Protocol => StatusCode::BAD_GATEWAY,
            FailureCode::Timeout => StatusCode::GATEWAY_TIMEOUT,
            FailureCode::Transport | FailureCode::Unavailable => {
                StatusCode::SERVICE_UNAVAILABLE
            }
        }
        .into_response(),
        Err(_) => StatusCode::GATEWAY_TIMEOUT.into_response(),
    }
}
