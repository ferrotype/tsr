//! Production content-mapper host setup for embedding.
//!
//! Supply a [`Spawner`] that fits the runtime, create a [`Host`] with
//! [`new_host`], then open its [`ProjectSpec`]. Pass that project through
//! [`crate::SessionServices`]. No process or filesystem implementation is
//! selected here. The caller closes the project and host after the sessions
//! using them have finished, including any later incremental loads.

pub use tsr_contentmapper::{
    new_host, DiagnosticDirectiveErrorKind, Error, Host, HostImpl, InitializeError,
    InitializeErrorKind, MappedResult, OptionDiagnostic, OptionPathSegment, PositionEncoding,
    Project, ProjectErrorKind, ProjectSpec, Request, SpawnError, Spawner, SpawnerFunc,
    TransformErrorKind, TransformResult,
};
pub use tsr_ipc::{Context, Error as IpcError, Stream};
pub use tsr_locale::Locale;
