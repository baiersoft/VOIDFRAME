//! The engine's single error type.

use std::backtrace::Backtrace;
use thiserror::Error as ThisError;

pub type Result<T> = std::result::Result<T, Error>;

/// The engine's single error type.
///
/// The specific failure mode is intentionally not part of the public API;
/// callers inspect it via the `is_xxx()` predicates below instead of
/// matching on a private `ErrorKind` (Microsoft's Pragmatic Rust
/// Guidelines, M-ERRORS-CANONICAL-STRUCTS).
///
/// `Display`/`std::error::Error` are implemented by hand rather than via
/// `#[derive(thiserror::Error)]`: a field literally named `backtrace` makes
/// thiserror emit an `Error::provide` impl gated behind the nightly-only
/// `error_generic_member_access` feature, which this crate's pinned stable
/// toolchain does not have.
#[derive(Debug)]
pub struct Error {
    kind: ErrorKind,
    backtrace: Backtrace,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.kind, f)
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        std::error::Error::source(&self.kind)
    }
}

#[derive(Debug, ThisError)]
enum ErrorKind {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("json: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("schema_version mismatch: expected {expected}, found {found}")]
    SchemaVersion { expected: String, found: String },

    #[error("module type `{kind}` is not supported in M1: {reason}")]
    UnsupportedModule { kind: String, reason: String },

    #[error("scenario conflict: {0}")]
    Conflict(String),

    #[error("mock: {0}")]
    Mock(String),

    #[error("preflight: {0}")]
    Preflight(String),

    #[error("another VOIDFRAME instance is already running")]
    AlreadyRunning,

    #[error("csv: {0}")]
    Csv(#[from] csv::Error),

    #[error("aborted: {0}")]
    Aborted(String),

    #[error("{0}")]
    Msg(String),
}

impl From<ErrorKind> for Error {
    fn from(kind: ErrorKind) -> Self {
        Self {
            kind,
            backtrace: Backtrace::capture(),
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        ErrorKind::from(e).into()
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        ErrorKind::from(e).into()
    }
}

impl From<csv::Error> for Error {
    fn from(e: csv::Error) -> Self {
        ErrorKind::from(e).into()
    }
}

impl Error {
    // `message: String` (not `impl Into<String>`) deliberately mirrors the
    // pre-M10 tuple variant's parameter type: every call site already had to
    // produce a `String` (via `.into()`/`format!()`/`.to_string()`) to build
    // the old enum variant, so keeping the parameter concrete here means
    // those call sites keep compiling completely unchanged. A generic bound
    // would make the `.into()` many of them already call ambiguous (E0283).
    pub(crate) fn msg(message: String) -> Self {
        ErrorKind::Msg(message).into()
    }

    pub(crate) fn schema_version(expected: String, found: String) -> Self {
        ErrorKind::SchemaVersion { expected, found }.into()
    }

    pub(crate) fn unsupported_module(kind: String, reason: String) -> Self {
        ErrorKind::UnsupportedModule { kind, reason }.into()
    }

    pub(crate) fn conflict(message: String) -> Self {
        ErrorKind::Conflict(message).into()
    }

    pub(crate) fn mock(message: String) -> Self {
        ErrorKind::Mock(message).into()
    }

    pub(crate) fn preflight(message: String) -> Self {
        ErrorKind::Preflight(message).into()
    }

    pub(crate) fn already_running() -> Self {
        ErrorKind::AlreadyRunning.into()
    }

    /// Constructed at every point the run loop observes an operator-issued
    /// `ControlMsg::Abort` -- `is_aborted()` is how `spawn_run` distinguishes
    /// "the operator asked us to stop" from every other run failure, to
    /// decide whether this run's on-disk files should be pruned.
    pub(crate) fn aborted(message: String) -> Self {
        ErrorKind::Aborted(message).into()
    }

    /// The backtrace captured when this error was constructed. Empty unless
    /// `RUST_BACKTRACE` (or `RUST_LIB_BACKTRACE`) is set; see
    /// [`std::backtrace::Backtrace::capture`].
    pub fn backtrace(&self) -> &Backtrace {
        &self.backtrace
    }

    pub fn is_io(&self) -> bool {
        matches!(self.kind, ErrorKind::Io(_))
    }

    pub fn is_serde(&self) -> bool {
        matches!(self.kind, ErrorKind::Serde(_))
    }

    pub fn is_schema_version(&self) -> bool {
        matches!(self.kind, ErrorKind::SchemaVersion { .. })
    }

    pub fn is_unsupported_module(&self) -> bool {
        matches!(self.kind, ErrorKind::UnsupportedModule { .. })
    }

    pub fn is_conflict(&self) -> bool {
        matches!(self.kind, ErrorKind::Conflict(_))
    }

    pub fn is_mock(&self) -> bool {
        matches!(self.kind, ErrorKind::Mock(_))
    }

    pub fn is_preflight(&self) -> bool {
        matches!(self.kind, ErrorKind::Preflight(_))
    }

    pub fn is_already_running(&self) -> bool {
        matches!(self.kind, ErrorKind::AlreadyRunning)
    }

    pub fn is_csv(&self) -> bool {
        matches!(self.kind, ErrorKind::Csv(_))
    }

    pub fn is_aborted(&self) -> bool {
        matches!(self.kind, ErrorKind::Aborted(_))
    }

    pub fn is_msg(&self) -> bool {
        matches!(self.kind, ErrorKind::Msg(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_error_converts_and_displays() {
        let e: Error = std::io::Error::new(std::io::ErrorKind::NotFound, "nope").into();
        assert!(e.to_string().contains("nope"));
        assert!(e.is_io());
    }

    #[test]
    fn unsupported_module_message_names_reason() {
        let e = Error::unsupported_module("driver_install".into(), "needs M3".into());
        assert!(e.to_string().contains("driver_install"));
        assert!(e.to_string().contains("needs M3"));
        assert!(e.is_unsupported_module());
    }
}
