use crate::shell::ShellParseError;

/// Structured, user-presentable errors of the mongo-raabit core.
#[derive(Debug, thiserror::Error)]
pub enum RaabitError {
    #[error("invalid {what}: {source}")]
    Parse {
        what: &'static str,
        #[source]
        source: ShellParseError,
    },
    #[error("invalid connection settings: {0}")]
    InvalidProfile(String),
    #[error("{0}")]
    Invalid(String),
    #[error("MongoDB error: {0}")]
    Mongo(#[from] mongodb::error::Error),
    #[error("secret store error: {0}")]
    Secret(String),
    #[error("SSH tunnel failed: {0}")]
    Tunnel(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Serde(String),
}

impl RaabitError {
    pub fn parse(what: &'static str, source: ShellParseError) -> Self {
        Self::Parse { what, source }
    }
}

pub type Result<T, E = RaabitError> = std::result::Result<T, E>;
