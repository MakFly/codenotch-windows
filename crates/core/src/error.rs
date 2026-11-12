//! One small error type for the whole app.
//!
//! Library code returns [`Result<T>`]. Messages must be short, user-safe English and must
//! never contain a secret (token, chat id, webhook URL, Authorization header).

use std::fmt;

/// Error shared by every crate. Keep variants coarse; put detail in the message.
#[derive(Debug)]
pub enum Error {
    /// File system error, with the path or operation in the message.
    Io(String, std::io::Error),
    /// JSON (de)serialisation error.
    Json(String),
    /// HTTP transport error or non-success status.
    Http(crate::http::HttpError),
    /// Feature not available (stub, unsupported platform, provider without sign-in …).
    Unsupported(String),
    /// Anything else.
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn io(context: impl Into<String>, err: std::io::Error) -> Self {
        Error::Io(context.into(), err)
    }
    pub fn other(msg: impl Into<String>) -> Self {
        Error::Other(msg.into())
    }
    pub fn unsupported(msg: impl Into<String>) -> Self {
        Error::Unsupported(msg.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(ctx, err) => write!(f, "{ctx}: {err}"),
            Error::Json(msg) => write!(f, "invalid JSON: {msg}"),
            Error::Http(err) => write!(f, "{err}"),
            Error::Unsupported(msg) => write!(f, "not supported: {msg}"),
            Error::Other(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(_, err) => Some(err),
            _ => None,
        }
    }
}

impl From<serde_json::Error> for Error {
    fn from(err: serde_json::Error) -> Self {
        Error::Json(err.to_string())
    }
}

impl From<crate::http::HttpError> for Error {
    fn from(err: crate::http::HttpError) -> Self {
        Error::Http(err)
    }
}
