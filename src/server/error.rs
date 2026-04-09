use axum::Json;
use axum::http::StatusCode;
use serde::Serialize;

/// Standard error envelope returned by all API endpoints.
#[derive(Serialize)]
pub struct ErrorBody {
    pub error: ErrorDetail,
}

/// Inner detail of an API error response.
#[derive(Serialize)]
pub struct ErrorDetail {
    pub code: String,
    pub message: String,
    pub status: u16,
}

/// Build a typed error tuple suitable for returning from axum handlers.
pub fn error_response(
    code: &str,
    message: &str,
    status: StatusCode,
) -> (StatusCode, Json<ErrorBody>) {
    (
        status,
        Json(ErrorBody {
            error: ErrorDetail {
                code: code.to_string(),
                message: message.to_string(),
                status: status.as_u16(),
            },
        }),
    )
}

/// Convenience alias for handler return types using the standard error envelope.
pub type ApiResult<T> = Result<(StatusCode, Json<T>), (StatusCode, Json<ErrorBody>)>;
