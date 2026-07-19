/// Errors surfaced by the forwarder.
#[derive(Debug, thiserror::Error)]
pub enum ForwarderError {
    #[error("http transport error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("server returned status {0}")]
    Status(u16),
    #[error("server returned status {0}: {1}")]
    StatusWithBody(u16, String),
    #[error("protobuf decode error: {0}")]
    Decode(#[from] prost::DecodeError),
    #[error("{0}")]
    Internal(String),
}

pub type Result<T> = std::result::Result<T, ForwarderError>;
