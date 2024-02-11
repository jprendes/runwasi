use std::fs::File;
use std::marker::PhantomData;
use std::os::fd::{FromRawFd, RawFd};
use std::path::{Path, PathBuf};

use anyhow::Context;
use chrono::{DateTime, Utc};
use libcontainer::container::builder::ContainerBuilder;
use libcontainer::container::Container;
use libcontainer::signal::Signal;
use libcontainer::syscall::syscall::SyscallType;
use nix::sys::wait::{waitid, Id as WaitID, WaitPidFlag, WaitStatus};
use nix::unistd::Pid;
use oci_spec::image::Platform;
use tokio::io::unix::AsyncFd;

use crate::container::Engine;
use crate::sandbox::instance_utils::{determine_rootdir, get_instance_root, instance_exists};
use crate::sandbox::sync::WaitableCell;
use crate::sandbox::{
    containerd, Error as SandboxError, Instance as SandboxInstance, InstanceConfig, Stdio,
};
use crate::sys::container::executor::Executor;

static DEFAULT_CONTAINER_ROOT_DIR: &str = "/run/containerd";

pub struct Instance<E: Engine> {
    exit_code: WaitableCell<(u32, DateTime<Utc>)>,
    rootdir: PathBuf,
    id: String,
    _phantom: PhantomData<E>,
}

impl<E: Engine> SandboxInstance for Instance<E> {
    type Engine = E;

    async fn new(
        id: String,
        cfg: Option<&InstanceConfig<Self::Engine>>,
    ) -> Result<Self, SandboxError> {
        let cfg = cfg.context("missing configuration")?;
        let engine = cfg.get_engine();
        let bundle = cfg.get_bundle().to_path_buf();
        let namespace = cfg.get_namespace();
        let rootdir = Path::new(DEFAULT_CONTAINER_ROOT_DIR).join(E::name());
        let rootdir = determine_rootdir(&bundle, &namespace, rootdir)?;
        let stdio = Stdio::init_from_cfg(cfg)?;

        // check if container is OCI image with wasm layers and attempt to read the module
        let (modules, platform) = containerd::Client::connect(cfg.get_containerd_address().as_str(), &namespace).await?
            .load_modules(&id, &engine)
            .await
            .unwrap_or_else(|e| {
                log::warn!("Error obtaining wasm layers for container {id}.  Will attempt to use files inside container image. Error: {e}");
                (vec![], Platform::default())
            });

        ContainerBuilder::new(id.clone(), SyscallType::Linux)
            .with_executor(Executor::new(engine, stdio, modules, platform))
            .with_root_path(rootdir.clone())?
            .as_init(&bundle)
            .with_systemd(false)
            .build()?;

        Ok(Self {
            id,
            exit_code: WaitableCell::new(),
            rootdir,
            _phantom: Default::default(),
        })
    }

    /// Start the instance
    /// The returned value should be a unique ID (such as a PID) for the instance.
    /// Nothing internally should be using this ID, but it is returned to containerd where a user may want to use it.
    async fn start(&self) -> Result<u32, SandboxError> {
        log::info!("starting instance: {}", self.id);
        // make sure we have an exit code by the time we finish (even if there's a panic)
        let guard = self.exit_code.set_guard_with(|| (137, Utc::now()));

        let container_root = get_instance_root(&self.rootdir, &self.id)?;
        let mut container = Container::load(container_root)?;
        let pid = container.pid().context("failed to get pid")?;

        // Use a pidfd FD so that we can wait for the process to exit asynchronously.
        let pidfd = PidFd::new(&Pid::from_raw(pid.as_raw()))?;

        container.start()?;

        let exit_code = self.exit_code.clone();

        tokio::spawn(async move {
            // move the exit code guard into this thread
            let _guard = guard;

            let status = match pidfd.wait().await {
                Ok(WaitStatus::Exited(_, status)) => status,
                Ok(WaitStatus::Signaled(_, sig, _)) => sig as i32,
                Ok(res) => {
                    log::error!("waitpid unexpected result: {res:?}");
                    137
                }
                Err(e) => {
                    log::error!("waitpid failed: {e}");
                    137
                }
            };

            let _ = exit_code.set((status as u32, Utc::now()));
        });

        Ok(pid.as_raw() as u32)
    }

    /// Send a signal to the instance
    async fn kill(&self, signal: u32) -> Result<(), SandboxError> {
        log::info!("sending signal {signal} to instance: {}", self.id);
        let signal = Signal::try_from(signal as i32).map_err(|err| {
            SandboxError::InvalidArgument(format!("invalid signal number: {}", err))
        })?;
        let container_root = get_instance_root(&self.rootdir, &self.id)?;
        let mut container = Container::load(container_root)
            .with_context(|| format!("could not load state for container {}", self.id))?;

        container.kill(signal, true)?;

        Ok(())
    }

    /// Delete any reference to the instance
    /// This is called after the instance has exited.
    async fn delete(&self) -> Result<(), SandboxError> {
        log::info!("deleting instance: {}", self.id);
        match instance_exists(&self.rootdir, &self.id) {
            Ok(true) => {}
            Ok(false) => return Ok(()),
            Err(err) => {
                log::error!("could not find the container, skipping cleanup: {}", err);
                return Ok(());
            }
        }
        let container_root = get_instance_root(&self.rootdir, &self.id)?;
        match Container::load(container_root) {
            Ok(mut container) => {
                container.delete(true)?;
            }
            Err(err) => {
                log::error!("could not find the container, skipping cleanup: {}", err);
            }
        }
        Ok(())
    }

    /// Waits for the instance to finish and retunrs its exit code
    /// Returns None if the timeout is reached before the instance has finished.
    /// This is a blocking call.
    async fn wait(&self) -> (u32, DateTime<Utc>) {
        *self.exit_code.wait().await
    }

    fn try_wait(&self) -> Option<(u32, DateTime<Utc>)> {
        self.exit_code.try_wait().copied()
    }
}

struct PidFd {
    pid: Pid,
    fd: AsyncFd<File>,
}

impl PidFd {
    fn new(pid: &Pid) -> anyhow::Result<Self> {
        use libc::{syscall, SYS_pidfd_open, PIDFD_NONBLOCK};
        let pidfd = unsafe { syscall(SYS_pidfd_open, pid.as_raw(), PIDFD_NONBLOCK) };
        if pidfd == -1 {
            return Err(std::io::Error::last_os_error().into());
        }
        let fd = AsyncFd::new(unsafe { File::from_raw_fd(pidfd as RawFd) })?;
        Ok(Self { pid: *pid, fd })
    }

    async fn wait(self) -> std::io::Result<WaitStatus> {
        let _ = self.fd.readable().await?;
        let pid = WaitID::Pid(self.pid);
        let status = waitid(pid, WaitPidFlag::WEXITED | WaitPidFlag::WNOHANG)?;
        Ok(status)
    }
}
