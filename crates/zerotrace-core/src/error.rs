use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("resource not found: {0}")]
    ResourceNotFound(&'static str),
    #[error("config error: {0}")]
    Config(String),
}
pub type Result<T> = std::result::Result<T, Error>;
