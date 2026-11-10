//! Error type for the Photo API client, queue refill and decoder.

use thiserror::Error;
use truongpaper_core::CoreError;

/// Everything that can go wrong talking to the photo API or maintaining the local queue.
#[derive(Debug, Error)]
pub enum PhotoApiError {
    /// Network / TLS / timeout failure from the HTTP transport.
    #[error("transport error: {0}")]
    Transport(String),
    /// HTTP 403 / 429 / 503: the ~100 requests per 5 minutes budget is spent or the host is
    /// shedding load. The caller must back off.
    #[error("rate limited by server (HTTP {0})")]
    RateLimited(u16),
    /// HTTP 400 `{"error":"invalid_parameters"}`.
    #[error("invalid request parameters (HTTP 400)")]
    InvalidParameters,
    /// HTTP 404 `{"error":"no_photos"}`: unknown album slug or empty pool.
    #[error("no photos available for this request (HTTP 404)")]
    NoPhotos,
    /// Any other non-success HTTP status.
    #[error("unexpected HTTP status {0}")]
    UnexpectedStatus(u16),
    /// Response body was not the documented JSON shape.
    #[error("invalid JSON response: {0}")]
    Json(#[from] serde_json::Error),
    /// The filter contains album slugs but none of them is valid.
    #[error("invalid photo filter: {0}")]
    InvalidFilter(String),
    /// Image bytes could not be decoded.
    #[error("image decode error: {0}")]
    Decode(String),
    /// Downloaded payload violates a safety limit.
    #[error("rejected download: {0}")]
    Rejected(String),
    /// Shared repository error.
    #[error(transparent)]
    Core(#[from] CoreError),
    /// Raw SQLite error.
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// Filesystem error.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// A blocking task panicked or was cancelled.
    #[error("background task failed: {0}")]
    Join(String),
}

impl PhotoApiError {
    /// True for the statuses that must trigger exponential backoff.
    pub fn is_throttle(&self) -> bool {
        matches!(self, PhotoApiError::RateLimited(_))
    }
}

/// Convenience alias.
pub type PhotoApiResult<T> = Result<T, PhotoApiError>;
