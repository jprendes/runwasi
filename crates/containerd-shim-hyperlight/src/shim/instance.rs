use std::sync::Arc;

use chrono::{DateTime, Utc};
use containerd_client::tonic::async_trait;
use containerd_shimkit::sandbox::sync::WaitableCell;
use containerd_shimkit::sandbox::{
    Error as SandboxError, Instance as SandboxInstance, InstanceConfig,
};
use oci_spec::image::Platform;
use oci_spec::runtime::Spec;
use tokio::sync::OnceCell;

use crate::containerd;
use crate::shim::hooks::run_hooks;

use super::container::Layer;
use super::Container;

pub struct Instance<C: Container> {
    exit_code: WaitableCell<(u32, DateTime<Utc>)>,
    container: Arc<C>,
    id: String,
}

#[async_trait]
trait OciClient {
    async fn load_modules(&self, id: &str) -> Result<(Vec<Layer>, Platform), SandboxError>;
}

struct EngineOciClient {
    client: containerd::Client,
}

#[async_trait]
impl OciClient for EngineOciClient {
    async fn load_modules(&self, id: &str) -> Result<(Vec<Layer>, Platform), SandboxError> {
        self.client.load_modules(id, &[
            "application/hyperlight"
        ]).await
    }
}

static OCI_CLIENT: OnceCell<Box<dyn OciClient + Send + Sync + 'static>> = OnceCell::const_new();

impl<C: Container> SandboxInstance for Instance<C> {
    async fn new(id: String, cfg: &InstanceConfig) -> Result<Self, SandboxError> {
        log::info!("Creating instance {}", id);
        let oci_client = OCI_CLIENT
            .get_or_try_init(|| async {
                let client =
                    containerd::Client::connect(&cfg.containerd_address, &cfg.namespace).await?;
                Result::<_, SandboxError>::Ok(Box::new(EngineOciClient { client }) as _)
            })
            .await?;

        // check if container is OCI image with wasm layers and attempt to read the module
        let (modules, _) = oci_client
            .load_modules(&id)
            .await
            .unwrap_or_else(|e| {
                log::warn!("Error obtaining layers for container {id}.  Will attempt to use files inside container image. Error: {e}");
                (vec![], Platform::default())
            });

        let source_spec_path = cfg.bundle.join("config.json");
        let spec = Spec::load(source_spec_path)?;
        log::info!("Running prestart hooks for instance {}: {:?}", id, spec.hooks());
        if let Some(hooks) = spec.hooks() {
            let hooks = hooks.prestart().iter().flatten();
            run_hooks(hooks).await?
        }

        let container = C::new(id.clone(), cfg.clone(), modules).await?;

        log::info!("Done creating instance {}", id);

        Ok(Self {
            id,
            exit_code: WaitableCell::new(),
            container: Arc::new(container),
        })
    }

    /// Start the instance
    /// The returned value should be a unique ID (such as a PID) for the instance.
    /// Nothing internally should be using this ID, but it is returned to containerd where a user may want to use it.
    async fn start(&self) -> Result<u32, SandboxError> {
        log::info!("Starting instance: {}", self.id);
        // make sure we have an exit code by the time we finish (even if there's a panic)
        let guard = self.exit_code.clone().set_guard_with(|| (137, Utc::now()));

        let exit_code = self.exit_code.clone();

        let container = self.container.clone();

        let id = self.id.clone();

        tokio::spawn(async move {
            log::info!("Running container {id}");
            // move the exit code guard into this task
            let _guard = guard;

            let status = container.start().await.unwrap_or(137);

            log::info!("Done running container {id}");

            let _ = exit_code.set((status, Utc::now()));
        });

        log::info!("Done starting instance: {}", self.id);

        Ok(self.container.pid())
    }

    /// Send a signal to the instance
    async fn kill(&self, signal: u32) -> Result<(), SandboxError> {
        log::info!("sending signal {signal} to instance: {}", self.id);
        self.container.kill(signal).await?;
        Ok(())
    }

    /// Delete any reference to the instance
    /// This is called after the instance has exited.
    async fn delete(&self) -> Result<(), SandboxError> {
        log::info!("deleting instance: {}", self.id);
        self.container.delete().await?;
        Ok(())
    }

    /// Waits for the instance to finish and returns its exit code
    /// Returns None if the timeout is reached before the instance has finished.
    /// This is an async call.
    async fn wait(&self) -> (u32, DateTime<Utc>) {
        log::info!("waiting for instance: {}", self.id);
        let res = *self.exit_code.wait().await;
        log::info!("done waiting for instance: {}", self.id);
        res
    }
}
