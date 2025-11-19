use axum::{Json, http::StatusCode, response::IntoResponse};
use serde_json::json;
use thiserror::Error;

use super::models::ErrorResponse;

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("payload invalid: {0}")]
    InvalidPayload(String),
    #[error("payload too large: {0} bytes")]
    PayloadTooLarge(usize),
    #[error("unsupported job type: {0}")]
    UnsupportedJobType(String),
    #[error("resource not found: {0}")]
    NotFound(String),
    #[error("internal error: {0}")]
    Internal(String),
}

impl ApiError {
    pub fn status_code(&self) -> StatusCode {
        match self {
            ApiError::InvalidPayload(_) => StatusCode::BAD_REQUEST,
            ApiError::PayloadTooLarge(_) => StatusCode::PAYLOAD_TOO_LARGE,
            ApiError::UnsupportedJobType(_) => StatusCode::FORBIDDEN,
            ApiError::NotFound(_) => StatusCode::NOT_FOUND,
            ApiError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            ApiError::InvalidPayload(_) => "INVALID_PAYLOAD",
            ApiError::PayloadTooLarge(_) => "PAYLOAD_TOO_LARGE",
            ApiError::UnsupportedJobType(_) => "UNSUPPORTED_JOB_TYPE",
            ApiError::NotFound(_) => "NOT_FOUND",
            ApiError::Internal(_) => "INTERNAL_ERROR",
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let status = self.status_code();
        let body = ErrorResponse {
            code: self.code(),
            message: self.to_string(),
        };

        (status, Json(json!(body))).into_response()
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(value: serde_json::Error) -> Self {
        ApiError::InvalidPayload(value.to_string())
    }
}
