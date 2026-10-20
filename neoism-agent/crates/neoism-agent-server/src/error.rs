use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

/// Expected admission and work-budget failures, not storage or programming faults.
#[derive(Debug)]
pub(crate) enum ConversationSearchError {
    Busy,
    TooBroad(String),
}

impl std::fmt::Display for ConversationSearchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy => f.write_str("Conversation search is busy; retry shortly"),
            Self::TooBroad(detail) => write!(f, "Conversation search is too broad ({detail}); refine the query or select a conversation/directory and retry"),
        }
    }
}

impl std::error::Error for ConversationSearchError {}

#[derive(Debug)]
pub(crate) struct ApiError {
    status: StatusCode,
    message: String,
    classification: Option<(&'static str, bool)>,
}

impl ApiError {
    /// Preserve API failures across plugin dispatch as ordinary HTTP responses;
    /// PluginRuntimeError carries no HTTP status or structured error body.
    pub(crate) fn into_plugin_response(self) -> neoism_agent_plugin_api::RouteResponse {
        let (status, body) = self.into_payload();
        neoism_agent_plugin_api::RouteResponse::json(status.as_u16(), body)
    }

    pub(crate) fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
            classification: None,
        }
    }

    pub(crate) fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: message.into(),
            classification: None,
        }
    }

    pub(crate) fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: message.into(),
            classification: None,
        }
    }

    pub(crate) fn gone(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::GONE,
            message: message.into(),
            classification: None,
        }
    }

    pub(crate) fn forbidden(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            message: message.into(),
            classification: None,
        }
    }

    pub(crate) fn too_many_requests(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            message: message.into(),
            classification: None,
        }
    }

    pub(crate) fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: message.into(),
            classification: None,
        }
    }

    pub(crate) fn service_unavailable(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message: message.into(),
            classification: None,
        }
    }

    pub(crate) fn is_context_overflow(&self) -> bool {
        neoism_agent_builtins::provider_error::is_context_overflow(&self.message)
    }

    pub(crate) fn is_conflict(&self) -> bool {
        self.status == StatusCode::CONFLICT
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(error: anyhow::Error) -> Self {
        if let Some(expected) = error.downcast_ref::<ConversationSearchError>() {
            return expected.into();
        }
        Self::internal(error.to_string())
    }
}

impl From<&ConversationSearchError> for ApiError {
    fn from(error: &ConversationSearchError) -> Self {
        let (status, code, retryable) = match error {
            ConversationSearchError::Busy => {
                (StatusCode::TOO_MANY_REQUESTS, "search.busy", true)
            }
            ConversationSearchError::TooBroad(_) => {
                (StatusCode::UNPROCESSABLE_ENTITY, "search.too_broad", false)
            }
        };
        Self {
            status,
            message: error.to_string(),
            classification: Some((code, retryable)),
        }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl ApiError {
    fn into_payload(self) -> (StatusCode, serde_json::Value) {
        let code = match self.status {
            StatusCode::BAD_REQUEST => "request.invalid",
            StatusCode::NOT_FOUND => "resource.not_found",
            StatusCode::CONFLICT => "resource.conflict",
            StatusCode::GONE => "lifecycle.closed",
            StatusCode::FORBIDDEN => "request.forbidden",
            StatusCode::NOT_IMPLEMENTED => "feature.not_implemented",
            StatusCode::SERVICE_UNAVAILABLE => "server.starting",
            _ => "server.internal",
        };
        let (code, retryable) = self
            .classification
            .unwrap_or((code, self.status.is_server_error()));
        (
            self.status,
            json!({
                "code": code,
                "message": self.message,
                "retryable": retryable,
                "details": {}
            }),
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, body) = self.into_payload();
        (status, Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn conversation_search_expected_failures_preserve_status_and_envelope() {
        let cases = [
            (ConversationSearchError::Busy, 429, "search.busy", true),
            (
                ConversationSearchError::TooBroad("128 ancestor budget exceeded".into()),
                422,
                "search.too_broad",
                false,
            ),
        ];
        for (error, status, code, retryable) in cases {
            let message = error.to_string();
            // anyhow context must not erase the expected classification or the
            // actionable message, even across the plugin route boundary.
            let error = anyhow::Error::new(error).context("search request failed");
            let api = ApiError::from(error);
            let direct = ApiError {
                status: api.status,
                message: api.message.clone(),
                classification: api.classification,
            }
            .into_response();
            assert_eq!(direct.status().as_u16(), status);
            let direct_body: serde_json::Value = serde_json::from_slice(
                &axum::body::to_bytes(direct.into_body(), usize::MAX)
                    .await
                    .unwrap(),
            )
            .unwrap();
            let plugin = api.into_plugin_response();
            assert_eq!(plugin.status, status);
            assert_eq!(plugin.body, direct_body);
            assert_eq!(plugin.body["code"], code);
            assert_eq!(plugin.body["retryable"], retryable);
            assert_eq!(plugin.body["message"], message);
            if status == 422 {
                assert!(message.contains("refine the query"));
            }
        }
    }

    #[tokio::test]
    async fn conversation_search_real_faults_stay_internal_not_string_classified() {
        for message in [
            "database read failed",
            "Conversation search is busy; retry shortly",
            "Conversation search is too broad; refine the query",
            "2 second query budget exceeded",
        ] {
            let response =
                ApiError::from(anyhow::anyhow!("{message}")).into_plugin_response();
            assert_eq!(response.status, 500);
            assert_eq!(response.body["code"], "server.internal");
            assert_eq!(response.body["retryable"], true);
            assert_eq!(response.body["message"], message);
        }
    }
}
