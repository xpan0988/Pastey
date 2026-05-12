use thiserror::Error;

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("{0}")]
    InvalidInput(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Timeout(String),
    #[error("{0}")]
    Network(String),
    #[error("{0}")]
    Crypto(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("database error: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("url error: {0}")]
    Url(#[from] url::ParseError),
    #[error("utf-8 error: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
    #[error("base64 error: {0}")]
    Base64(#[from] base64::DecodeError),
}

impl AppError {
    pub fn message(&self) -> String {
        match self {
            Self::InvalidInput(message)
            | Self::NotFound(message)
            | Self::Timeout(message)
            | Self::Network(message)
            | Self::Crypto(message) => message.clone(),
            Self::Io(error) => match error.kind() {
                std::io::ErrorKind::NotFound => "File is no longer available.".to_string(),
                std::io::ErrorKind::PermissionDenied => {
                    "Could not read or write that file. Check folder permissions.".to_string()
                }
                std::io::ErrorKind::WriteZero => {
                    "Could not write to destination folder.".to_string()
                }
                _ => "File transfer failed because the file could not be read or written."
                    .to_string(),
            },
            Self::Sql(_) | Self::Json(_) => "Could not update local pastey storage.".to_string(),
            Self::Http(error) if error.is_timeout() => "Transfer interrupted".to_string(),
            Self::Http(error) if error.is_connect() => "Peer disconnected".to_string(),
            Self::Http(_) => "Transfer interrupted".to_string(),
            Self::Url(_) => "Selected file path is invalid.".to_string(),
            Self::Utf8(_) => "Received text was not valid UTF-8.".to_string(),
            Self::Base64(_) => "Invalid file metadata".to_string(),
        }
    }
}
