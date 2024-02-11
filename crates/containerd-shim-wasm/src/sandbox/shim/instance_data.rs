use std::sync::Arc;

use chrono::{DateTime, Utc};
use tokio::sync::{OnceCell, RwLock};

use crate::sandbox::instance::Nop;
use crate::sandbox::shim::instance_option::InstanceOption;
use crate::sandbox::shim::task_state::TaskState;
use crate::sandbox::{Instance, InstanceConfig, Result};

pub(super) struct InstanceData<T: Instance + Send + Sync> {
    pub instance: InstanceOption<T>,
    cfg: InstanceConfig<T::Engine>,
    pid: OnceCell<u32>,
    state: Arc<RwLock<TaskState>>,
}

impl<T: Instance> InstanceData<T> {
    pub async fn new_instance(id: impl AsRef<str>, cfg: InstanceConfig<T::Engine>) -> Result<Self> {
        let id = id.as_ref().to_string();
        let instance = InstanceOption::Instance(T::new(id, Some(&cfg)).await?);
        Ok(Self {
            instance,
            cfg,
            pid: OnceCell::default(),
            state: Arc::new(RwLock::new(TaskState::Created)),
        })
    }

    pub async fn new_base(id: impl AsRef<str>, cfg: InstanceConfig<T::Engine>) -> Result<Self> {
        let id = id.as_ref().to_string();
        let instance = InstanceOption::Nop(Nop::new(id, None).await?);
        Ok(Self {
            instance,
            cfg,
            pid: OnceCell::default(),
            state: Arc::new(RwLock::new(TaskState::Created)),
        })
    }

    pub fn pid(&self) -> Option<u32> {
        self.pid.get().copied()
    }

    pub fn config(&self) -> &InstanceConfig<T::Engine> {
        &self.cfg
    }

    pub async fn start(&self) -> Result<u32> {
        let mut s = self.state.write().await;
        s.start()?;

        let res = self.instance.start().await;

        // These state transitions are always `Ok(())` because
        // we hold the lock since `s.start()`
        let _ = match res {
            Ok(pid) => {
                let _ = self.pid.set(pid);
                s.started()
            }
            Err(_) => s.stop(),
        };

        res
    }

    pub async fn kill(&self, signal: u32) -> Result<()> {
        let mut s = self.state.write().await;
        s.kill()?;

        self.instance.kill(signal).await
    }

    pub async fn delete(&self) -> Result<()> {
        let mut s = self.state.write().await;
        s.delete()?;

        let res = self.instance.delete().await;

        if res.is_err() {
            // Always `Ok(())` because we hold the lock since `s.delete()`
            let _ = s.stop();
        }

        res
    }

    pub async fn wait(&self) -> (u32, DateTime<Utc>) {
        let res = self.instance.wait().await;
        let mut s = self.state.write().await;
        *s = TaskState::Exited;
        res
    }

    pub async fn try_wait(&self) -> Option<(u32, DateTime<Utc>)> {
        let res = self.instance.try_wait();
        if res.is_some() {
            let mut s = self.state.write().await;
            *s = TaskState::Exited;
        }
        res
    }
}
