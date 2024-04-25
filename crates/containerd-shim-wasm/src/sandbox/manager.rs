//! This experimental module implements a manager service which can be used to
//! manage multiple instances of a sandbox in-process.
//! The idea behind this module is to only need a single shim process for the entire node rather than one per pod/container.

use std::collections::HashMap;
use std::env::current_dir;

use async_trait::async_trait;
use containerd_shim::error::Error as ShimError;
use containerd_shim::publisher::RemotePublisher;
use containerd_shim::{self as shim, api, Task, TtrpcResult};
use oci_spec::runtime::Spec;
use shim::protos::trapeze::{Client, Server};
use shim::util::write_str_to_file;
use shim::{io_error, Flags};
use tokio::sync::RwLock;
use trapeze::service;

use super::error::Error;
use super::instance::Instance;

mod sandbox {
    trapeze::include_protos!(["protos/sandbox.proto"]);
    pub use self::runwasi::services::sandbox::v1::*;
}

pub use sandbox::Manager;

/// Sandbox wraps an Instance and is used with the `Service` to manage multiple instances.
pub trait Sandbox: Task + Send + Sync {
    type Instance: Instance;

    fn new(
        namespace: String,
        containerd_address: String,
        id: String,
        engine: <Self::Instance as Instance>::Engine,
        publisher: RemotePublisher,
    ) -> Self;
}

/// Service is a manager service which can be used to manage multiple instances of a sandbox in-process.
pub struct Service<T: Sandbox> {
    sandboxes: RwLock<HashMap<String, String>>,
    engine: <T::Instance as Instance>::Engine,
    phantom: std::marker::PhantomData<T>,
}

impl<T: Sandbox> Service<T> {
    pub fn new(engine: <T::Instance as Instance>::Engine) -> Self {
        Self {
            sandboxes: RwLock::new(HashMap::new()),
            engine,
            phantom: std::marker::PhantomData,
        }
    }
}

impl<T: Sandbox> Default for Service<T>
where
    <T::Instance as Instance>::Engine: Default,
{
    fn default() -> Self {
        Self::new(Default::default())
    }
}

impl<T: Sandbox + 'static> Manager for Service<T> {
    async fn create(&self, req: sandbox::CreateRequest) -> TtrpcResult<sandbox::CreateResponse> {
        let mut sandboxes = self.sandboxes.write().await;

        if sandboxes.contains_key(&req.id) {
            return Err(Error::AlreadyExists(req.id).into());
        }

        let sock = format!("unix://{}/shim.sock", &req.working_directory);

        let publisher = RemotePublisher::new(req.ttrpc_address).await?;

        let sb = T::new(
            req.namespace.clone(),
            req.containerd_address.clone(),
            req.id.clone(),
            self.engine.clone(),
            publisher,
        );

        sandboxes.insert(req.id.clone(), sock.clone());

        // TODO: why did we need setup_namespaces here?
        // setup_namespaces(&cfg)?;
        tokio::spawn({
            let sock = sock.clone();
            async move { Server::new().register(service!(sb : Task)).bind(sock).await }
        });

        Ok(sandbox::CreateResponse { socket_path: sock })
    }

    async fn delete(&self, req: sandbox::DeleteRequest) -> TtrpcResult<sandbox::DeleteResponse> {
        let mut sandboxes = self.sandboxes.write().await;
        if !sandboxes.contains_key(&req.id) {
            return Err(Error::NotFound(req.id).into());
        }
        let sock = sandboxes.remove(&req.id).unwrap();
        let c = Client::connect(&sock).await.map_err(|_| trapeze::Status {
            code: trapeze::Code::Internal.into(),
            message: "Error connecting to containerd".into(),
            ..Default::default()
        })?;

        c.shutdown(api::ShutdownRequest {
            id: req.id,
            now: true,
        })
        .await?;

        Ok(sandbox::DeleteResponse::default())
    }
}

/// Shim implements the containerd-shim CLI for connecting to a Manager service.
pub struct Shim {
    id: String,
    namespace: String,
}

impl Task for Shim {}

#[async_trait]
impl shim::Shim for Shim {
    type T = Self;

    async fn new(_runtime_id: &str, args: &Flags, _config: &mut shim::Config) -> Self {
        Shim {
            id: args.id.to_string(),
            namespace: args.namespace.to_string(),
        }
    }

    async fn start_shim(&mut self, opts: containerd_shim::StartOpts) -> shim::Result<String> {
        let dir = current_dir().map_err(|err| ShimError::Other(err.to_string()))?;
        let spec = Spec::load(dir.join("config.json").to_str().unwrap()).map_err(|err| {
            shim::Error::InvalidArgument(format!("error loading runtime spec: {}", err))
        })?;

        let default = HashMap::new() as HashMap<String, String>;
        let annotations = spec.annotations().as_ref().unwrap_or(&default);

        let sandbox = annotations
            .get("io.kubernetes.cri.sandbox-id")
            .unwrap_or(&opts.id)
            .to_string();

        let client = Client::connect("unix:///run/io.containerd.wasmwasi.v1/manager.sock")
            .await
            .map_err(io_error!(err, "Error connecting to manager"))?;

        let addr = match Manager::create(
            &client,
            sandbox::CreateRequest {
                id: sandbox.clone(),
                working_directory: dir.as_path().to_str().unwrap().to_string(),
                ttrpc_address: opts.ttrpc_address.clone(),
                ..Default::default()
            },
        )
        .await
        {
            Ok(res) => res.socket_path,
            Err(_) => {
                let res = Manager::connect(
                    &client,
                    sandbox::ConnectRequest {
                        id: sandbox,
                        ttrpc_address: opts.ttrpc_address,
                    },
                )
                .await?;
                res.socket_path
            }
        };

        write_str_to_file("address", &addr).await?;

        Ok(addr)
    }

    async fn wait(&mut self) {
        todo!()
    }

    async fn create_task_service(&self, _publisher: RemotePublisher) -> Self::T {
        todo!() // but not really, haha
    }

    async fn delete_shim(&mut self) -> shim::Result<api::DeleteResponse> {
        let dir = current_dir().map_err(|err| ShimError::Other(err.to_string()))?;
        let spec = Spec::load(dir.join("config.json").to_str().unwrap()).map_err(|err| {
            shim::Error::InvalidArgument(format!("error loading runtime spec: {}", err))
        })?;

        let default = HashMap::new() as HashMap<String, String>;
        let annotations = spec.annotations().as_ref().unwrap_or(&default);

        let sandbox = annotations
            .get("io.kubernetes.cri.sandbox-id")
            .unwrap_or(&self.id)
            .to_string();
        if sandbox != self.id {
            return Ok(api::DeleteResponse::default());
        }

        let client = Client::connect("unix:///run/io.containerd.wasmwasi.v1/manager.sock")
            .await
            .map_err(io_error!(err, "Error connecting to manager"))?;
        Manager::delete(
            &client,
            sandbox::DeleteRequest {
                id: sandbox,
                namespace: self.namespace.clone(),
                ..Default::default()
            },
        )
        .await?;

        // TODO: write pid, exit code, etc to disk so we can use it here.
        Ok(api::DeleteResponse::default())
    }
}
