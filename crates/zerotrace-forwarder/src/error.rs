/// Errors surfaced by the forwarder.
#[derive(Debug, thiserror::Error)]
pub enum ForwarderError {
    #[error("http transport error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("server returned status {0}")]
    Status(u16),
    #[error("protobuf decode error: {0}")]
    Decode(#[from] prost::DecodeError),
}

pub type Result<T> = std::result::Result<T, ForwarderError>;
