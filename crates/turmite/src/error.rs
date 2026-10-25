use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not found")]
    NotFound,
    #[error("This method is not allowed on this path.")]
    MethodNotAllowed,
    #[error("{0}")]
    Unauthorized(String),
    #[error("{0}")]
    Forbidden(String),
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Conflict(String),
    #[error("too many requests, try again later")]
    TooManyRequests,
    #[error("{0}")]
    Unavailable(String),
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("rate error: {0}")]
    Rate(String),
    #[error("chain error: {0}")]
    Driver(String),
    #[error("internal error: {0}")]
    Internal(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn status(&self) -> StatusCode {
        match self {
            Error::NotFound => StatusCode::NOT_FOUND,
            Error::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
            Error::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            Error::Forbidden(_) => StatusCode::FORBIDDEN,
            Error::BadRequest(_) => StatusCode::BAD_REQUEST,
            Error::Conflict(_) => StatusCode::CONFLICT,
            Error::TooManyRequests => StatusCode::TOO_MANY_REQUESTS,
            Error::Unavailable(_) | Error::Rate(_) => StatusCode::SERVICE_UNAVAILABLE,
            Error::Db(_) | Error::Driver(_) | Error::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            Error::NotFound => "NOT_FOUND",
            Error::MethodNotAllowed => "METHOD_NOT_ALLOWED",
            Error::Unauthorized(_) => "AUTH_REQUIRED",
            Error::Forbidden(_) => "FORBIDDEN",
            Error::BadRequest(_) => "INVALID_REQUEST_PARAMS",
            Error::Conflict(_) => "CONFLICT",
            Error::TooManyRequests => "TOO_MANY_REQUESTS",
            Error::Unavailable(_) | Error::Rate(_) => "UNAVAILABLE",
            Error::Db(_) | Error::Driver(_) | Error::Internal(_) => "INTERNAL_ERROR",
        }
    }

    /// Text that is safe to show to a client. Internal causes are logged, not shown.
    pub fn public_message(&self) -> String {
        match self.status() {
            StatusCode::INTERNAL_SERVER_ERROR => "Internal error.".into(),
            StatusCode::SERVICE_UNAVAILABLE if matches!(self, Error::Rate(_)) => {
                "Exchange rates are not available. Try again in a minute.".into()
            }
            _ => self.to_string(),
        }
    }

    pub fn log(&self) {
        match self.status() {
            StatusCode::INTERNAL_SERVER_ERROR => tracing::error!(error = %self, "request failed"),
            StatusCode::SERVICE_UNAVAILABLE => tracing::warn!(error = %self, "request failed"),
            _ => {}
        }
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        self.log();
        let status = self.status();
        let body = json!({
            "status": false,
            "statusCode": status.as_u16(),
            "code": self.code(),
            "message": self.public_message(),
        });
        let mut resp = (status, Json(body)).into_response();
        if matches!(self, Error::Unavailable(_)) {
            resp.headers_mut().insert(axum::http::header::RETRY_AFTER, axum::http::HeaderValue::from_static("30"));
        }
        resp
    }
}
