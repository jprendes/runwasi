use std::env::current_dir;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use containerd_shim::error::Error as ShimError;
use containerd_shim::publisher::RemotePublisher;
use containerd_shim::util::write_str_to_file;
use containerd_shim::{self as shim, api, ExitSignal};
use oci_spec::runtime::Spec;
use shim::Flags;

use crate::sandbox::instance::Instance;
use crate::sandbox::shim::events::{RemoteEventSender, ToTimestamp};
use crate::sandbox::shim::local::Local;
use crate::sys::networking::setup_namespaces;

/// Cli implements the containerd-shim cli interface using `Local<T>` as the task service.
pub struct Cli<T: Instance + Sync + Send> {
    engine: T::Engine,
    namespace: String,
    containerd_address: String,
    exit: Arc<ExitSignal>,
    _id: String,
}

#[async_trait]
impl<I> shim::Shim for Cli<I>
where
    I: Instance + Sync + Send,
    <I as Instance>::Engine: Default,
{
    type T = Local<I>;

    async fn new(_runtime_id: &str, args: &Flags, _config: &mut shim::Config) -> Self {
        Cli {
            engine: Default::default(),
            namespace: args.namespace.to_string(),
            containerd_address: args.address.clone(),
            exit: Arc::default(),
            _id: args.id.to_string(),
        }
    }

    async fn start_shim(&mut self, opts: containerd_shim::StartOpts) -> shim::Result<String> {
        let dir = current_dir().map_err(|err| ShimError::Other(err.to_string()))?;
        let spec = Spec::load(dir.join("config.json")).map_err(|err| {
            shim::Error::InvalidArgument(format!("error loading runtime spec: {}", err))
        })?;

        let id = opts.id.clone();
        let grouping = spec
            .annotations()
            .as_ref()
            .and_then(|a| a.get("io.kubernetes.cri.sandbox-id"))
            .unwrap_or(&id);

        setup_namespaces(&spec)
            .map_err(|e| shim::Error::Other(format!("failed to setup namespaces: {}", e)))?;

        let address = shim::spawn(opts, grouping, vec![]).await?;

        write_str_to_file("address", &address).await?;

        Ok(address)
    }

    async fn wait(&mut self) {
        self.exit.wait().await
    }

    async fn create_task_service(&self, publisher: RemotePublisher) -> Self::T {
        let events = RemoteEventSender::new(&self.namespace, publisher);
        let exit = self.exit.clone();
        let engine = self.engine.clone();
        Local::<I>::new(
            engine,
            events,
            exit,
            &self.namespace,
            &self.containerd_address,
        )
    }

    async fn delete_shim(&mut self) -> shim::Result<api::DeleteResponse> {
        Ok(api::DeleteResponse {
            exit_status: 137,
            exited_at: Some(Utc::now().to_timestamp()).into(),
            ..Default::default()
        })
    }
}
