use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0} not found")]
    NotFound(String),
    #[error("{0} already exists")]
    Conflict(String),
    #[error("invalid input: {0}")]
    Invalid(String),
    #[error("{0}")]
    Forbidden(String),
    #[error("storage error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("render error: {0}")]
    Render(#[from] minijinja::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    pub fn status(&self) -> StatusCode {
        match self {
            Error::NotFound(_) => StatusCode::NOT_FOUND,
            Error::Conflict(_) => StatusCode::CONFLICT,
            Error::Invalid(_) | Error::Json(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Error::Forbidden(_) => StatusCode::FORBIDDEN,
            Error::Io(_) | Error::Render(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let status = self.status();
        if status.is_server_error() {
            tracing::error!("{self}");
        }
        (status, Json(json!({ "error": self.to_string() }))).into_response()
    }
}
