use chrono::{DateTime, Utc};

use crate::sandbox::instance::Nop;
use crate::sandbox::{Instance, InstanceConfig, Result};

pub(super) enum InstanceOption<I: Instance> {
    Instance(I),
    Nop(Nop),
}

impl<I: Instance> Instance for InstanceOption<I> {
    type Engine = ();

    async fn new(_id: String, _cfg: Option<&InstanceConfig<Self::Engine>>) -> Result<Self> {
        // this is never called
        unimplemented!();
    }

    async fn start(&self) -> Result<u32> {
        match self {
            Self::Instance(i) => i.start().await,
            Self::Nop(i) => i.start().await,
        }
    }

    async fn kill(&self, signal: u32) -> Result<()> {
        match self {
            Self::Instance(i) => i.kill(signal).await,
            Self::Nop(i) => i.kill(signal).await,
        }
    }

    async fn delete(&self) -> Result<()> {
        match self {
            Self::Instance(i) => i.delete().await,
            Self::Nop(i) => i.delete().await,
        }
    }

    async fn wait(&self) -> (u32, DateTime<Utc>) {
        match self {
            Self::Instance(i) => i.wait().await,
            Self::Nop(i) => i.wait().await,
        }
    }

    fn try_wait(&self) -> Option<(u32, DateTime<Utc>)> {
        match self {
            Self::Instance(i) => i.try_wait(),
            Self::Nop(i) => i.try_wait(),
        }
    }
}
