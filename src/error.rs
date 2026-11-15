//! cdsk's error type.

use std::path::PathBuf;

/// The result type every cdsk operation returns.
pub type Result<T> = std::result::Result<T, Error>;

/// Everything cdsk can refuse or fail with.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A geometry other than logical 4096 and physical 4096.
    #[error("refusing {what}: {logical}/{physical} is not 4Kn (only 4096/4096 is allowed)")]
    NotFourKn {
        /// What was being created.
        what: String,
        /// The requested logical sector size.
        logical: u32,
        /// The requested physical sector size.
        physical: u32,
    },
    /// An external command failed.
    #[error("{command} failed: {message}")]
    Command {
        /// The command as a shell would word it.
        command: String,
        /// The command's standard error.
        message: String,
    },
    /// A filesystem operation failed.
    #[error("{path}: {source}")]
    Io {
        /// The path that failed.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },
    /// A JSON document did not parse.
    #[error("{what}: {source}")]
    Json {
        /// What the document was.
        what: String,
        /// The parser's error.
        #[source]
        source: serde_json::Error,
    },
    /// The declaration is invalid.
    #[error("declaration: {0}")]
    Declaration(String),
    /// cdsk declined an operation for safety.
    #[error("refused: {0}")]
    Refused(String),
    /// No disk is bound to the instance.
    #[error("instance {0} has no disk binding")]
    UnknownInstance(String),
}

impl Error {
    pub(crate) fn command(argv: &[String], message: impl Into<String>) -> Self {
        Self::Command {
            command: crate::plan::Action::new(argv.iter().cloned()).render(),
            message: message.into(),
        }
    }

    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    pub(crate) fn json(what: impl Into<String>, source: serde_json::Error) -> Self {
        Self::Json {
            what: what.into(),
            source,
        }
    }
}
