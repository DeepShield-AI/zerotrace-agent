use std::any::TypeId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleError {
    StartFailed(String),
    StopFailed(String),
    Health(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundleError {
    MissingDep { dep: TypeId },
    Factory(String),
    Cycle(Vec<String>),
    DuplicateProvider { tid: TypeId, existing: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PipelineError {
    ChannelClosed,
    Source(String),
    Processor(String),
    Reporter(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    Parse(String),
    Watch(String),
    Io(String),
}

/// Classification for programmatic error handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorClass {
    /// The caller can retry immediately — e.g. transient I/O, lock contention.
    Retryable,
    /// The caller should back off before retrying — e.g. rate limit,
    /// channel full, resource temporarily unavailable.
    RetryableWithBackoff,
    /// The error is permanent — retrying will not help (e.g. missing
    /// dependency, config parse error, cycle detected).
    Fatal,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("resource not found: {type_name}")]
    ResourceNotFound { type_name: &'static str },

    /// A resource exists at the given key, but its storage mode doesn't
    /// match the requested accessor.  For example, the resource was stored
    /// via [`insert_raw`] (direct `Arc<T>`) but retrieved via [`get`]
    /// (expecting `Arc<RwLock<T>>`), or vice versa.
    #[error(
        "resource type mismatch for [{type_name}]: stored as {storage_mode} but accessed as {access_mode}. \
         Use get() for RwLock<T> resources and get_raw() for direct Arc<T> resources."
    )]
    ResourceTypeMismatch {
        type_name: &'static str,
        storage_mode: &'static str,
        access_mode: &'static str,
    },

    #[error("lifecycle error in [{component}]: {message}")]
    Lifecycle {
        component: &'static str,
        message: String,
    },

    #[error("config dispatch error: {0}")]
    ConfigDispatch(String),

    #[error("bundle [{bundle_id}] failed: {message}")]
    Bundle {
        bundle_id: &'static str,
        message: String,
    },

    #[error("pipeline error: {message}")]
    Pipeline {
        message: String,
        /// Whether the error is permanent (channel closed, missing
        /// component) vs. transient (backpressure, retryable I/O).
        /// Used by [`Error::class`] instead of fragile string matching.
        fatal: bool,
    },

    #[error("config error: {0}")]
    Config(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(String),
}

impl Error {
    /// Classify the error for programmatic handling.
    pub fn class(&self) -> ErrorClass {
        match self {
            Error::ResourceNotFound { .. } | Error::ResourceTypeMismatch { .. } =>
                ErrorClass::Fatal,
            Error::Lifecycle { .. } => ErrorClass::Fatal,
            Error::ConfigDispatch(_) => ErrorClass::RetryableWithBackoff,
            Error::Bundle { .. } => ErrorClass::Fatal,
            Error::Pipeline { fatal, .. } =>
                if *fatal {
                    ErrorClass::Fatal
                } else {
                    ErrorClass::RetryableWithBackoff
                },
            Error::Config(_) => ErrorClass::Fatal,
            Error::Io(e) => {
                use std::io::ErrorKind;
                match e.kind() {
                    ErrorKind::NotFound | ErrorKind::PermissionDenied => ErrorClass::Fatal,
                    ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted =>
                        ErrorClass::Retryable,
                    _ => ErrorClass::RetryableWithBackoff,
                }
            },
            Error::Other(_) => ErrorClass::Fatal,
        }
    }

    /// Shorthand: true if [`class`](Self::class) returns [`ErrorClass::Fatal`].
    pub fn is_fatal(&self) -> bool {
        matches!(self.class(), ErrorClass::Fatal)
    }

    /// Shorthand: true if the error is transient and may succeed on retry.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self.class(),
            ErrorClass::Retryable | ErrorClass::RetryableWithBackoff
        )
    }

    /// Construct a lifecycle start-failure error.
    pub fn lifecycle_start(component: &'static str, reason: impl Into<String>) -> Self {
        Error::Lifecycle {
            component,
            message: reason.into(),
        }
    }

    /// Construct a bundle missing-dependency error.
    pub fn bundle_missing_dep(bundle_id: &'static str, dep: TypeId) -> Self {
        Error::Bundle {
            bundle_id,
            message: format!("missing dependency TypeId({:?})", dep),
        }
    }

    /// Construct a bundle factory failure error.
    pub fn bundle_factory(
        bundle_id: &'static str,
        component_id: &'static str,
        reason: impl Into<String>,
    ) -> Self {
        Error::Bundle {
            bundle_id,
            message: format!("factory for [{}] failed: {}", component_id, reason.into()),
        }
    }

    /// Construct a pipeline channel-closed error (permanent).
    pub fn pipeline_closed() -> Self {
        Error::Pipeline {
            message: "channel closed".into(),
            fatal: true,
        }
    }

    /// Construct a pipeline source error (may be transient).
    pub fn pipeline_source(name: &'static str, reason: impl Into<String>) -> Self {
        Error::Pipeline {
            message: format!("source [{}] error: {}", name, reason.into()),
            fatal: false,
        }
    }

    /// Construct a config parse error.
    pub fn config_parse(detail: impl Into<String>) -> Self {
        Error::Config(detail.into())
    }

    /// Construct a resource type-mismatch error.
    /// `storage_mode` describes how the resource was stored (e.g. "direct Arc<T>" or "Arc<RwLock<T>>").
    /// `access_mode` describes how the caller tried to retrieve it.
    pub fn resource_type_mismatch(
        type_name: &'static str,
        storage_mode: &'static str,
        access_mode: &'static str,
    ) -> Self {
        Error::ResourceTypeMismatch {
            type_name,
            storage_mode,
            access_mode,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resource_not_found_is_fatal() {
        let err = Error::ResourceNotFound { type_name: "Foo" };
        assert!(err.is_fatal());
        assert!(!err.is_retryable());
    }

    #[test]
    fn test_io_not_found_is_fatal() {
        let err = Error::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "file not found",
        ));
        assert!(err.is_fatal());
    }

    #[test]
    fn test_io_would_block_is_retryable() {
        let err = Error::Io(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "try again",
        ));
        assert!(err.is_retryable());
        assert!(!err.is_fatal());
    }

    #[test]
    fn test_pipeline_channel_closed_is_fatal() {
        let err = Error::Pipeline {
            message: "channel closed".into(),
            fatal: true,
        };
        assert!(err.is_fatal());
    }

    #[test]
    fn test_bundle_is_fatal() {
        let err = Error::Bundle {
            bundle_id: "test",
            message: "cycle".into(),
        };
        assert!(err.is_fatal());
    }

    #[test]
    fn test_config_dispatch_is_retryable() {
        let err = Error::ConfigDispatch("timeout".into());
        assert!(err.is_retryable());
    }

    #[test]
    fn test_lifecycle_start_constructor() {
        let err = Error::lifecycle_start("my_comp", "connection refused");
        assert!(err.to_string().contains("my_comp"));
        assert!(err.to_string().contains("connection refused"));
    }

    #[test]
    fn test_bundle_missing_dep_constructor() {
        let tid = std::any::TypeId::of::<String>();
        let err = Error::bundle_missing_dep("my_bundle", tid);
        assert!(err.to_string().contains("my_bundle"));
    }

    #[test]
    fn test_pipeline_closed_constructor() {
        assert!(Error::pipeline_closed().is_fatal());
    }

    #[test]
    fn test_config_parse_constructor() {
        let err = Error::config_parse("invalid YAML at line 3");
        assert!(err.to_string().contains("invalid YAML"));
        assert!(err.is_fatal());
    }

    #[test]
    fn test_resource_type_mismatch_is_fatal() {
        let err = Error::resource_type_mismatch("MyType", "direct Arc<T>", "Arc<RwLock<T>>");
        assert!(err.is_fatal());
        assert!(!err.is_retryable());
        let msg = err.to_string();
        assert!(msg.contains("MyType"));
        assert!(msg.contains("type mismatch"));
    }

    #[test]
    fn test_resource_type_mismatch_error_message_guides_fix() {
        let err = Error::resource_type_mismatch("DbPool", "direct Arc<T>", "Arc<RwLock<T>>");
        let msg = err.to_string();
        assert!(
            msg.contains("get() for RwLock<T>") || msg.contains("get_raw() for direct Arc<T>"),
            "error message should guide user to correct API, got: {msg}"
        );
    }
}
