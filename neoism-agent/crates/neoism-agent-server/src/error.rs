use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

#[derive(Debug)]
pub(crate) struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    pub(crate) fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    pub(crate) fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: message.into(),
        }
    }

    pub(crate) fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: message.into(),
        }
    }

    pub(crate) fn gone(message: impl Into<String>) -> Self {
        Self { status: StatusCode::GONE, message: message.into() }
    }

    pub(crate) fn forbidden(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            message: message.into(),
        }
    }

    pub(crate) fn too_many_requests(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            message: message.into(),
        }
    }

    pub(crate) fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: message.into(),
        }
    }

    pub(crate) fn is_conflict(&self) -> bool {
        self.status == StatusCode::CONFLICT
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(error: anyhow::Error) -> Self {
        Self::internal(error.to_string())
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let code = match self.status {
            StatusCode::BAD_REQUEST => "request.invalid",
            StatusCode::NOT_FOUND => "resource.not_found",
            StatusCode::CONFLICT => "resource.conflict",
            StatusCode::GONE => "lifecycle.closed",
            StatusCode::FORBIDDEN => "request.forbidden",
            StatusCode::NOT_IMPLEMENTED => "feature.not_implemented",
            _ => "server.internal",
        };
        let retryable = self.status.is_server_error();
        (
            self.status,
            Json(json!({
                "code": code,
                "message": self.message,
                "retryable": retryable,
                "details": {}
            })),
        )
            .into_response()
    }
}
