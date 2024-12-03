use std::error::Error as StdError;
use std::fmt::{Debug, Display, Formatter, Result as FmtResult};

use anyhow::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct IpcError(IpcErrorInner);

impl IpcError {
    fn new(err: &(impl StdError + ?Sized)) -> Self {
        Self(IpcErrorInner::new(err))
    }

    fn into_anyhow(self) -> Error {
        self.0.into_anyhow()
    }
}

#[derive(Serialize, Deserialize)]
struct IpcErrorInner {
    description: String,
    source: Option<Box<IpcErrorInner>>,
}

impl IpcErrorInner {
    fn new(err: &(impl StdError + ?Sized)) -> Self {
        IpcErrorInner {
            description: err.to_string(),
            source: err.source().map(|src| Box::new(Self::new(src))),
        }
    }

    fn into_anyhow(self) -> Error {
        Error::new(self)
    }
}

impl<E: Into<Error>> From<E> for IpcError {
    fn from(err: E) -> Self {
        let err = err.into();
        Self::new(AsRef::<dyn StdError>::as_ref(&err))
    }
}

impl StdError for IpcErrorInner {
    fn source(&self) -> Option<&(dyn 'static + StdError)> {
        self.source
            .as_ref()
            .map(|s| &**s as &(dyn 'static + StdError))
    }

    fn description(&self) -> &str {
        &self.description
    }
}

impl Display for IpcErrorInner {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "{}", self.description)
    }
}

impl Debug for IpcErrorInner {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "{}", self.description)
    }
}

impl Display for IpcError {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        Display::fmt(&self.0, f)
    }
}

impl Debug for IpcError {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        Debug::fmt(&self.0, f)
    }
}

pub type IpcResult<T> = Result<T, IpcError>;

pub trait IntoAnyhow {
    type T;
    fn into_anyhow(self) -> Result<Self::T>;
}

impl<T> IntoAnyhow for IpcResult<T> {
    type T = T;
    fn into_anyhow(self) -> Result<Self::T> {
        self.map_err(|err| err.into_anyhow())
    }
}
