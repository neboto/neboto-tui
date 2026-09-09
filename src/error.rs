use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("AWS SDK error: {0}")]
    AwsSdk(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Service not found: {0:?}")]
    #[allow(dead_code)]
    ServiceNotFound(crate::aws::service::ServiceType),

    #[error("Resource not found: {0}")]
    #[allow(dead_code)]
    ResourceNotFound(String),

    #[error("Feature not implemented")]
    NotImplemented,

    #[error("Invalid configuration: {0}")]
    #[allow(dead_code)]
    InvalidConfig(String),

    #[error("Event channel error")]
    #[allow(dead_code)]
    ChannelError,

    #[error("Editor operation failed: {0}")]
    EditorFailed(String),

    #[error("Editor not found: {0}. Set $EDITOR environment variable.")]
    EditorNotFound(String),
}

pub type Result<T> = std::result::Result<T, Error>;

use aws_smithy_runtime_api::client::result::SdkError;
use aws_smithy_types::error::metadata::ProvideErrorMetadata;

/// Extract the fullest human-readable message from an AWS SDK error.
///
/// A bare `SdkError::to_string()` is just the generic `"service error"` — the
/// real detail (e.g. the AccessDenied explanation, the API error code) lives in
/// the modeled error's metadata. This returns `"<Code>: <message>"` when both
/// are present, otherwise whichever is available, falling back to the full
/// source chain (`DisplayErrorContext`) for non-modeled failures like timeouts
/// or dispatch errors so the cause is never hidden.
pub fn sdk_error_message<E, R>(err: &SdkError<E, R>) -> String
where
    E: std::error::Error + Send + Sync + 'static,
    R: std::fmt::Debug,
    SdkError<E, R>: ProvideErrorMetadata,
{
    match (err.code(), err.message()) {
        (Some(code), Some(msg)) => format!("{code}: {msg}"),
        (None, Some(msg)) => msg.to_string(),
        (Some(code), None) => code.to_string(),
        (None, None) => {
            // No modeled metadata (timeout / dispatch / construction failure) —
            // walk the full source chain so the underlying cause is preserved.
            format!(
                "{}",
                aws_smithy_types::error::display::DisplayErrorContext(err)
            )
        }
    }
}

// Helper for converting AWS SDK errors to our Error type. Routes through
// `sdk_error_message` so `?`-based conversions surface the real AWS message
// rather than the generic "service error" string.
impl<E, R> From<SdkError<E, R>> for Error
where
    E: std::error::Error + Send + Sync + 'static,
    R: std::fmt::Debug + Send + Sync + 'static,
    SdkError<E, R>: ProvideErrorMetadata,
{
    fn from(err: SdkError<E, R>) -> Self {
        Error::AwsSdk(sdk_error_message(&err))
    }
}
