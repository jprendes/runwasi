use std::marker::PhantomData;
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::container::Engine;
use crate::sandbox::{Error as SandboxError, Instance as SandboxInstance, InstanceConfig};

pub struct Instance<E: Engine>(PhantomData<E>);

impl<E: Engine> SandboxInstance for Instance<E> {
    type Engine = E;

    async fn new(
        _id: String,
        _cfg: Option<&InstanceConfig<Self::Engine>>,
    ) -> Result<Self, SandboxError> {
        todo!();
    }

    /// Start the instance
    /// The returned value should be a unique ID (such as a PID) for the instance.
    /// Nothing internally should be using this ID, but it is returned to containerd where a user may want to use it.
    async fn start(&self) -> Result<u32, SandboxError> {
        todo!();
    }

    /// Send a signal to the instance
    async fn kill(&self, _signal: u32) -> Result<(), SandboxError> {
        todo!();
    }

    /// Delete any reference to the instance
    /// This is called after the instance has exited.
    async fn delete(&self) -> Result<(), SandboxError> {
        todo!();
    }

    async fn wait(&self) -> (u32, DateTime<Utc>) {
        todo!();
    }

    /// Waits for the instance to finish and retunrs its exit code
    /// Returns None if the timeout is reached before the instance has finished.
    /// This is a blocking call.
    fn try_wait(&self) -> Option<(u32, DateTime<Utc>)> {
        todo!();
    }
}
