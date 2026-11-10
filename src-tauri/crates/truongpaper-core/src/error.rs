//! Error type shared by the `truongpaper-core` repository helpers.

use thiserror::Error;

/// Errors produced by the shared SQLite data-access layer.
#[derive(Debug, Error)]
pub enum CoreError {
    /// Any rusqlite / SQLite failure.
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// Filesystem failure (cache directory creation, file removal, ...).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// A settings value could not be (de)serialized.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    /// Caller passed a value that violates a documented invariant.
    #[error("invalid input: {0}")]
    InvalidInput(String),
}

/// Convenience alias used by every repository function.
pub type CoreResult<T> = Result<T, CoreError>;
