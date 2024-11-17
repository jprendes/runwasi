use std::collections::HashMap;
use std::fs::create_dir_all;
use std::path::Path;
use std::sync::{Arc, RwLock};
use std::time::Duration;

//use containerd_shim::error::Error as ShimError;
//use containerd_shim::util::IntoOption;
use log::debug;
use oci_spec::runtime::Spec;
use shimkit::event::EventPublisher;
use shimkit::types::events::*;
use shimkit::types::sandbox::Sandbox;
use shimkit::types::task::*;
use shimkit::types::{Code, Result as TtrpcResult, Status as TtrpcStatus};
use trapeze::{get_server, StatusExt as _};

use crate::sandbox::instance::{Instance, InstanceConfig};
use crate::sandbox::shim::instance_data::InstanceData;
use crate::sandbox::utils::{ToTimestamp as _, WithTimeout as _};
use crate::sandbox::{oci, Error, Result};
use crate::sys::metrics::get_metrics;

/*
#[cfg(test)]
mod tests;
*/

type LocalInstances<T> = RwLock<HashMap<String, Arc<InstanceData<T>>>>;

/// Local implements the Task service for a containerd shim.
/// It defers all task operations to the `Instance` implementation.
pub struct Local<T: Instance + Send + Sync> {
    pub engine: T::Engine,
    pub(super) instances: LocalInstances<T>,
    events: EventPublisher,
    namespace: String,
    containerd_address: String,
}

impl<T: Instance + Send + Sync> Local<T> {
    /// Creates a new local task service.
    #[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip_all, level = "Info"))]
    pub fn new(
        engine: T::Engine,
        events: EventPublisher,
        namespace: impl AsRef<str>,
        containerd_address: impl AsRef<str>,
    ) -> Self {
        let instances = RwLock::default();
        let namespace = namespace.as_ref().to_string();
        let containerd_address = containerd_address.as_ref().to_string();
        Self {
            engine,
            instances,
            events,
            namespace,
            containerd_address,
        }
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip_all, level = "Info"))]
    pub(super) fn get_instance(&self, id: &str) -> Result<Arc<InstanceData<T>>> {
        let instance = self.instances.read().unwrap().get(id).cloned();
        instance.ok_or_else(|| Error::NotFound(id.to_string()))
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip_all, level = "Info"))]
    fn has_instance(&self, id: &str) -> bool {
        self.instances.read().unwrap().contains_key(id)
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip_all, level = "Info"))]
    fn is_empty(&self) -> bool {
        self.instances.read().unwrap().is_empty()
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip_all, level = "Info"))]
    fn instance_config(&self) -> InstanceConfig<T::Engine> {
        InstanceConfig::new(
            self.engine.clone(),
            &self.namespace,
            &self.containerd_address,
        )
    }
}

// These are the same functions as in Task, but without the TtrcpContext, which is useful for testing
impl<T: Instance + Send + Sync> Local<T> {
    #[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip_all, level = "Info"))]
    async fn task_create(&self, req: CreateTaskRequest) -> TtrpcResult<CreateTaskResponse> {
        if !req.checkpoint.is_empty() || !req.parent_checkpoint.is_empty() {
            return Err(TtrpcStatus::unimplemented("checkpoint is not supported"));
        }

        if req.terminal {
            return Err(TtrpcStatus::invalid_argument("terminal is not supported"));
        }

        if self.has_instance(&req.id) {
            return Err(TtrpcStatus::already_exists(req.id));
        }

        let mut spec = Spec::load(Path::new(&req.bundle).join("config.json")).map_err(|err| {
            TtrpcStatus::invalid_argument(format!("could not load runtime spec: {err}"))
        })?;

        spec.canonicalize_rootfs(&req.bundle).map_err(|err| {
            TtrpcStatus::invalid_argument(format!("could not canonicalize rootfs: {}", err))
        })?;

        let rootfs = spec
            .root()
            .as_ref()
            .ok_or_else(|| TtrpcStatus::invalid_argument("rootfs is not set in runtime spec"))?
            .path();

        let _ = create_dir_all(rootfs);
        mount_rootfs(&req.rootfs).await?;

        let mut cfg = self.instance_config();
        cfg.set_bundle(&req.bundle)
            .set_stdin(&req.stdin)
            .set_stdout(&req.stdout)
            .set_stderr(&req.stderr);

        // Check if this is a cri container
        let instance = InstanceData::new(&req.id, cfg).await?;

        self.instances
            .write()
            .unwrap()
            .insert(req.id.clone(), Arc::new(instance));

        let _ = self
            .events
            .publish(TaskCreate {
                container_id: req.id,
                bundle: req.bundle,
                rootfs: req.rootfs,
                io: Some(TaskIo {
                    stdin: req.stdin,
                    stdout: req.stdout,
                    stderr: req.stderr,
                    ..Default::default()
                })
                .into(),
                ..Default::default()
            })
            .await;

        debug!("create done");

        // Per the spec, the prestart hook must be called as part of the create operation
        debug!("call prehook before the start");
        oci::setup_prestart_hooks(spec.hooks())?;

        Ok(CreateTaskResponse {
            pid: std::process::id(),
            ..Default::default()
        })
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip_all, level = "Info"))]
    async fn task_start(&self, req: StartRequest) -> TtrpcResult<StartResponse> {
        if !req.exec_id.is_empty() {
            return Err(TtrpcStatus::unimplemented("exec is not supported"));
        }

        let i = self.get_instance(&req.id)?;
        let pid = i.start()?;

        let _ = self
            .events
            .publish(TaskStart {
                container_id: req.id.clone(),
                pid,
                ..Default::default()
            })
            .await;

        let events = self.events.clone();

        let id = req.id.clone();

        tokio::spawn(async move {
            let (exit_code, timestamp) = i.wait().await;
            let _ = events
                .publish(TaskExit {
                    container_id: id.clone(),
                    exit_status: exit_code,
                    exited_at: timestamp.to_timestamp().into(),
                    pid,
                    id,
                })
                .await;
        });
        debug!("started: {:?}", req);

        Ok(StartResponse {
            pid,
            ..Default::default()
        })
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip_all, level = "Info"))]
    async fn task_kill(&self, req: KillRequest) -> Result<()> {
        if !req.exec_id.is_empty() {
            return Err(Error::InvalidArgument("exec is not supported".to_string()));
        }
        self.get_instance(&req.id)?.kill(req.signal)?;
        Ok(())
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip_all, level = "Info"))]
    async fn task_delete(&self, req: DeleteRequest) -> Result<DeleteResponse> {
        if !req.exec_id.is_empty() {
            return Err(Error::InvalidArgument("exec is not supported".to_string()));
        }

        let i = self.get_instance(&req.id)?;

        i.delete()?;

        let pid = i.pid().unwrap_or_default();
        let (exit_code, timestamp) = i.wait().with_timeout(Duration::ZERO).await.unzip();
        let timestamp = timestamp.map(|t| t.to_timestamp());

        self.instances.write().unwrap().remove(&req.id);

        let _ = self
            .events
            .publish(TaskDelete {
                container_id: req.id.clone(),
                pid,
                exit_status: exit_code.unwrap_or_default(),
                exited_at: timestamp.clone().into(),
                ..Default::default()
            })
            .await;

        Ok(DeleteResponse {
            pid,
            exit_status: exit_code.unwrap_or_default(),
            exited_at: timestamp.into(),
        })
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip_all, level = "Info"))]
    async fn task_wait(&self, req: WaitRequest) -> Result<WaitResponse> {
        if !req.exec_id.is_empty() {
            return Err(Error::InvalidArgument("exec is not supported".to_string()));
        }

        let i = self.get_instance(&req.id)?;
        let (exit_code, timestamp) = i.wait().await;

        debug!("wait finishes");
        Ok(WaitResponse {
            exit_status: exit_code,
            exited_at: Some(timestamp.to_timestamp()).into(),
            ..Default::default()
        })
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip_all, level = "Info"))]
    async fn task_state(&self, req: StateRequest) -> Result<StateResponse> {
        if !req.exec_id.is_empty() {
            return Err(Error::InvalidArgument("exec is not supported".to_string()));
        }

        let i = self.get_instance(&req.id)?;
        let pid = i.pid();
        let (exit_code, timestamp) = i.wait().with_timeout(Duration::ZERO).await.unzip();
        let timestamp = timestamp.map(|t| t.to_timestamp());

        let status = if pid.is_none() {
            Status::Created
        } else if exit_code.is_none() {
            Status::Running
        } else {
            Status::Stopped
        };

        Ok(StateResponse {
            bundle: i.config().get_bundle().to_string_lossy().to_string(),
            stdin: i.config().get_stdin().to_string_lossy().to_string(),
            stdout: i.config().get_stdout().to_string_lossy().to_string(),
            stderr: i.config().get_stderr().to_string_lossy().to_string(),
            pid: pid.unwrap_or_default(),
            exit_status: exit_code.unwrap_or_default(),
            exited_at: timestamp.into(),
            status: status as _,
            ..Default::default()
        })
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip_all, level = "Info"))]
    async fn task_stats(&self, req: StatsRequest) -> Result<StatsResponse> {
        let i = self.get_instance(&req.id)?;
        let pid = i
            .pid()
            .ok_or_else(|| Error::InvalidArgument("task is not running".to_string()))?;

        let _metrics = get_metrics(pid)?;

        Ok(StatsResponse {
            stats: None, //Some(metrics),
        })
    }
}

async fn mount_rootfs(_rootfs_mounts: impl IntoIterator<Item = &Mount>) -> Result<()> {
    // TODO
    /*
    for m in rootfs_mounts {
        let mount_type = match m.r#type.as_str() {
            "" => None,
            s => Some(s),
        };
        let source = match m.source.as_str() {
            "" => None,
            s => Some(s),
        };

        #[cfg(unix)]
        containerd_shim::mount::mount_rootfs(
            mount_type,
            source,
            &m.options.to_vec(),
            rootfs,
        )?;
    }
    */
    Ok(())
}

impl<T: Instance + Sync + Send> Task for Local<T> {
    #[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip_all, level = "Info"))]
    async fn create(&self, req: CreateTaskRequest) -> TtrpcResult<CreateTaskResponse> {
        debug!("create: {:?}", req);
        self.task_create(req).await.or_status(Code::Internal)
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip_all, level = "Info"))]
    async fn start(&self, req: StartRequest) -> TtrpcResult<StartResponse> {
        debug!("start: {:?}", req);
        self.task_start(req).await.or_status(Code::Internal)
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip_all, level = "Info"))]
    async fn kill(&self, req: KillRequest) -> TtrpcResult<()> {
        debug!("kill: {:?}", req);
        self.task_kill(req).await.or_status(Code::Internal)
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip_all, level = "Info"))]
    async fn delete(&self, req: DeleteRequest) -> TtrpcResult<DeleteResponse> {
        debug!("delete: {:?}", req);
        self.task_delete(req).await.or_status(Code::Internal)
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip_all, level = "Info"))]
    async fn wait(&self, req: WaitRequest) -> TtrpcResult<WaitResponse> {
        debug!("wait: {:?}", req);

        // Start a task to export interval span for long wait
        #[cfg(feature = "opentelemetry")]
        let _handle = {
            use tokio::time::{sleep, Duration};
            use tracing::{span, Level, Span};

            use crate::sandbox::utils::AbortOnDrop as _;

            let parent_span = Span::current();
            tokio::spawn(async move {
                loop {
                    let current_span =
                        span!(parent: &parent_span, Level::INFO, "task wait 60s interval");
                    let _enter = current_span.enter();
                    sleep(Duration::from_secs(60)).await;
                }
            })
            .abort_on_drop()
        };

        self.task_wait(req).await.or_status(Code::Internal)
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip_all, level = "Info"))]
    async fn connect(&self, req: ConnectRequest) -> TtrpcResult<ConnectResponse> {
        debug!("connect: {:?}", req);
        let i = self.get_instance(&req.id).or_status(Code::Internal)?;
        let shim_pid = std::process::id();
        let task_pid = i.pid().unwrap_or_default();
        Ok(ConnectResponse {
            shim_pid,
            task_pid,
            ..Default::default()
        })
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip_all, level = "Info"))]
    async fn state(&self, req: StateRequest) -> TtrpcResult<StateResponse> {
        debug!("state: {:?}", req);
        self.task_state(req).await.or_status(Code::Internal)
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip_all, level = "Info"))]
    async fn shutdown(&self, req: ShutdownRequest) -> TtrpcResult<()> {
        debug!("shutdown");
        if self.is_empty() {
            let controller = get_server();
            if req.now {
                controller.terminate()
            } else {
                controller.shutdown()
            }
        }
        Ok(())
    }

    #[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip_all, level = "Info"))]
    async fn stats(&self, req: StatsRequest) -> TtrpcResult<StatsResponse> {
        debug!("stats: {:?}", req);
        self.task_stats(req).await.or_status(Code::Internal)
    }
}

impl<T: Instance + Send + Sync> Sandbox for Local<T> {}
