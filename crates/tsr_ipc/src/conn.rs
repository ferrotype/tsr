//! The connection and handler contracts and their errors.
use crate::Context;
use std::sync::Arc;
use tsr_json::{Decode, Encode};
use tsr_jsonrpc::FramingError;

/// A failure of a connection or of a request on it, rendered as the pinned
/// errors render (`errors.Join` joins lines).
#[derive(Clone, Debug)]
pub enum Error {
    /// `ErrConnClosed`, joined with the cause that closed the connection.
    ConnClosed(Option<Box<Error>>),
    /// `ErrRequestTimeout`.
    RequestTimeout,
    Context(crate::ContextError),
    /// The peer answered a call with an error response.
    Remote {
        code: i32,
        message: String,
    },
    Framing(Arc<FramingError>),
    Json(tsr_json::Error),
    Io(Arc<std::io::Error>),
    /// A handler's or a caller's own error text.
    Message(String),
    /// `fmt.Errorf("<context>: %w", cause)`.
    Wrapped(String, Box<Error>),
    /// `errors.Join`.
    Joined(Vec<Error>),
}

impl Error {
    /// The stream ended cleanly (`errors.Is(err, io.EOF)`).
    pub fn is_eof(&self) -> bool {
        match self {
            Self::Framing(error) => matches!(**error, FramingError::Eof),
            Self::Wrapped(_, cause) => cause.is_eof(),
            Self::Joined(errors) => errors.iter().any(Self::is_eof),
            _ => false,
        }
    }

    /// The context error this error wraps (`errors.Is(err, context.Canceled)`).
    pub fn context_error(&self) -> Option<crate::ContextError> {
        match self {
            Self::Context(error) => Some(*error),
            Self::ConnClosed(Some(cause)) | Self::Wrapped(_, cause) => cause.context_error(),
            Self::Joined(errors) => errors.iter().find_map(Self::context_error),
            _ => None,
        }
    }

    /// `errors.Join(self, other)`, flattening a join on the left.
    #[must_use]
    pub fn join(self, other: Self) -> Self {
        match self {
            Self::Joined(mut errors) => {
                errors.push(other);
                Self::Joined(errors)
            }
            error => Self::Joined(vec![error, other]),
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConnClosed(None) => f.write_str("ipc: connection closed"),
            Self::ConnClosed(Some(cause)) => write!(f, "ipc: connection closed\n{cause}"),
            Self::RequestTimeout => f.write_str("ipc: request timeout"),
            Self::Context(error) => write!(f, "{error}"),
            Self::Remote { code, message } => write!(f, "ipc: remote error [{code}]: {message}"),
            Self::Framing(error) => write!(f, "{error}"),
            Self::Json(error) => write!(f, "{error}"),
            Self::Io(error) => write!(f, "{error}"),
            Self::Message(message) => f.write_str(message),
            Self::Wrapped(context, cause) => write!(f, "{context}: {cause}"),
            Self::Joined(errors) => {
                for (index, error) in errors.iter().enumerate() {
                    if index > 0 {
                        f.write_str("\n")?;
                    }
                    write!(f, "{error}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for Error {}

impl From<tsr_json::Error> for Error {
    fn from(error: tsr_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io(Arc::new(error))
    }
}

impl From<crate::ContextError> for Error {
    fn from(error: crate::ContextError) -> Self {
        Self::Context(error)
    }
}

/// A handler's failure; its text becomes the error response's message.
pub type HandlerError = Box<dyn std::error::Error + Send + Sync>;

/// A handler's successful result: `None` is a nil result.
pub type HandlerResult = Result<Option<Box<dyn Encode + Send>>, HandlerError>;

/// Processes incoming requests and notifications.
pub trait Handler: Send + Sync {
    /// Handles an incoming request and returns a result or an error.
    fn handle_request(&self, ctx: &Context, method: &str, params: &[u8]) -> HandlerResult;
    /// Handles an incoming notification.
    fn handle_notification(
        &self,
        ctx: &Context,
        method: &str,
        params: &[u8],
    ) -> Result<(), HandlerError>;
}

/// A bidirectional connection.
pub trait Conn: Send + Sync {
    /// Processes messages until the context is done, the stream ends or an
    /// error occurs.
    fn run(&self, ctx: &Context) -> Result<(), Error>;
    /// Sends a request to the peer and waits for its response.
    fn call(
        &self,
        ctx: &Context,
        method: &str,
        params: &dyn Encode,
    ) -> Result<tsr_json::RawValue, Error>;
    /// Sends a notification to the peer; no response is expected.
    fn notify(&self, ctx: &Context, method: &str, params: &dyn Encode) -> Result<(), Error>;
}

/// Decodes params into a typed value, `None` when there are none.
/// port: tsc/internal/ipc/conn.go:UnmarshalParams
pub fn unmarshal_params<T: Decode + Default>(params: &[u8]) -> Result<Option<T>, tsr_json::Error> {
    if params.is_empty() {
        return Ok(None);
    }
    let mut value = T::default();
    tsr_json::unmarshal(params, &mut value, tsr_json::Options::default())?;
    Ok(Some(value))
}
