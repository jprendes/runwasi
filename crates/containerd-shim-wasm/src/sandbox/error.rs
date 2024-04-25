//! Error types used by shims
//! This handles converting to the appropriate ttrpc error codes

use anyhow::Error as AnyError;
use containerd_shim::Error as ShimError;
use oci_spec::OciSpecError;
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
    Shim(#[from] ShimError),
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

impl From<Error> for trapeze::Status {
    fn from(e: Error) -> Self {
        match e {
            Error::Shim(s) => match s {
                ShimError::InvalidArgument(s) => trapeze::Status {
                    code: trapeze::Code::InvalidArgument.into(),
                    message: s,
                    ..Default::default()
                },
                ShimError::NotFoundError(s) => trapeze::Status {
                    code: trapeze::Code::NotFound.into(),
                    message: s,
                    ..Default::default()
                },
                _ => trapeze::Status {
                    code: trapeze::Code::Unknown.into(),
                    message: s.to_string(),
                    ..Default::default()
                },
            },
            Error::NotFound(s) => trapeze::Status {
                code: trapeze::Code::NotFound.into(),
                message: s,
                ..Default::default()
            },
            Error::AlreadyExists(s) => trapeze::Status {
                code: trapeze::Code::AlreadyExists.into(),
                message: s,
                ..Default::default()
            },
            Error::InvalidArgument(s) => trapeze::Status {
                code: trapeze::Code::InvalidArgument.into(),
                message: s,
                ..Default::default()
            },
            Error::FailedPrecondition(s) => trapeze::Status {
                code: trapeze::Code::FailedPrecondition.into(),
                message: s,
                ..Default::default()
            },
            Error::Oci(ref _s) => trapeze::Status {
                code: trapeze::Code::Unknown.into(),
                message: e.to_string(),
                ..Default::default()
            },
            Error::Any(s) => trapeze::Status {
                code: trapeze::Code::Unknown.into(),
                message: s.to_string(),
                ..Default::default()
            },
            _ => trapeze::Status {
                code: trapeze::Code::Unknown.into(),
                message: e.to_string(),
                ..Default::default()
            },
        }
    }
}

#[cfg(test)]
mod tests {
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
        let t: trapeze::Status = e.into();
        assert_eq!(
            t,
            trapeze::Status {
                code: trapeze::Code::InvalidArgument.into(),
                message: "invalid argument".into(),
                ..Default::default()
            }
        );

        let e = Error::NotFound("not found".to_string());
        let t: trapeze::Status = e.into();
        assert_eq!(
            t,
            trapeze::Status {
                code: trapeze::Code::NotFound.into(),
                message: "not found".into(),
                ..Default::default()
            }
        );

        let e = Error::AlreadyExists("already exists".to_string());
        let t: trapeze::Status = e.into();
        assert_eq!(
            t,
            trapeze::Status {
                code: trapeze::Code::AlreadyExists.into(),
                message: "already exists".into(),
                ..Default::default()
            }
        );

        let e = Error::FailedPrecondition("failed precondition".to_string());
        let t: trapeze::Status = e.into();
        assert_eq!(
            t,
            trapeze::Status {
                code: trapeze::Code::FailedPrecondition.into(),
                message: "failed precondition".into(),
                ..Default::default()
            }
        );

        let e = Error::Shim(ShimError::InvalidArgument("invalid argument".to_string()));
        let t: trapeze::Status = e.into();
        assert_eq!(
            t,
            trapeze::Status {
                code: trapeze::Code::InvalidArgument.into(),
                message: "invalid argument".into(),
                ..Default::default()
            }
        );

        let e = Error::Any(AnyError::new(TestError::AnError("any error".to_string())));
        let t: trapeze::Status = e.into();
        assert_eq!(
            t,
            trapeze::Status {
                code: trapeze::Code::Unknown.into(),
                message: "any error".into(),
                ..Default::default()
            }
        );
    }
}
