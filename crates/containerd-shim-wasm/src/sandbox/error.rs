//! Error types used by shims
//! This handles converting to the appropriate ttrpc error codes

use anyhow::Error as AnyError;
use oci_spec::OciSpecError;
use shimkit::types::Status as TtrpcStatus;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    /// An error occurred while parsing the OCI spec
    #[error("{0}")]
    Oci(#[from] OciSpecError),
    /// An error that can occur while setting up the environment for the container
    #[error("{0}")]
    Stdio(#[from] std::io::Error),
    #[error("{0}")]
    Others(String),
    /// Errors to/from the containerd shim library.
    #[error("{0}")]
    Ttrpc(#[from] TtrpcStatus),
    /// Requested item is not found
    #[error("not found: {0}")]
    NotFound(String),
    /// Requested item already exists
    #[error("already exists: {0}")]
    AlreadyExists(String),
    /// Supplied arguments/options/config is invalid
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// Any other error
    #[error("{0}")]
    Any(#[from] AnyError),
    /// The operation was rejected because the system is not in a state required for the operation's
    #[error("{0}")]
    FailedPrecondition(String),
    /// Error while parsing JSON
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    /// Error from the system
    #[cfg(unix)]
    #[error("{0}")]
    Errno(#[from] nix::errno::Errno),
    /// Errors from libcontainer
    #[cfg(unix)]
    #[error("{0}")]
    Libcontainer(#[from] libcontainer::error::LibcontainerError),
    #[error("{0}")]
    Containerd(String),
}

pub type Result<T, E = Error> = ::std::result::Result<T, E>;

impl From<Error> for TtrpcStatus {
    fn from(e: Error) -> Self {
        match e {
            Error::Ttrpc(s) => s,
            Error::NotFound(s) => TtrpcStatus::not_found(s),
            Error::AlreadyExists(s) => TtrpcStatus::already_exists(s),
            Error::InvalidArgument(s) => TtrpcStatus::invalid_argument(s),
            Error::FailedPrecondition(s) => TtrpcStatus::failed_precondition(s),
            e => TtrpcStatus::unknown(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use shimkit::types::Code;
    use thiserror::Error;

    use super::*;

    #[derive(Debug, Error)]
    enum TestError {
        #[error("{0}")]
        AnError(String),
    }

    #[test]
    fn test_error_to_ttrpc_status() {
        let e = Error::InvalidArgument("invalid argument".to_string());
        let s: TtrpcStatus = e.into();
        assert_eq!(s.code(), Code::InvalidArgument);
        assert_eq!(s.message, "invalid argument");

        let e = Error::NotFound("not found".to_string());
        let s: TtrpcStatus = e.into();
        assert_eq!(s.code(), Code::NotFound);
        assert_eq!(s.message, "not found");

        let e = Error::AlreadyExists("already exists".to_string());
        let s: TtrpcStatus = e.into();
        assert_eq!(s.code(), Code::AlreadyExists);
        assert_eq!(s.message, "already exists");

        let e = Error::FailedPrecondition("failed precondition".to_string());
        let s: TtrpcStatus = e.into();
        assert_eq!(s.code(), Code::FailedPrecondition);
        assert_eq!(s.message, "failed precondition");

        let e = Error::Ttrpc(TtrpcStatus::invalid_argument("invalid argument"));
        let s: TtrpcStatus = e.into();
        assert_eq!(s.code(), Code::InvalidArgument);
        assert_eq!(s.message, "invalid argument");

        let e = Error::Any(AnyError::new(TestError::AnError("any error".to_string())));
        let s: TtrpcStatus = e.into();
        assert_eq!(s.code(), Code::Unknown);
        assert_eq!(s.message, "any error");
    }
}
