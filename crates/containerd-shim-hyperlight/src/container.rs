use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use anyhow::{Context, ensure};
use containerd_shimkit::sandbox::InstanceConfig;
use hyperlight_host::HyperlightError;
use oci_spec::runtime::Spec;

use hyperlight_host::func::{ParameterValue, ReturnType};
use hyperlight_host::sandbox_state::sandbox::EvolvableSandbox;
use hyperlight_host::sandbox_state::transition::Noop;
use hyperlight_host::{MultiUseSandbox, UninitializedSandbox};
use tokio::sync::Mutex;

use crate::shim::{Container, Layer};

pub struct SomeContainer {
    id: String,
    rootfs: PathBuf,
    cfg: InstanceConfig,
    sandbox: Mutex<Option<UninitializedSandbox>>,
}

impl Container for SomeContainer {
    async fn new(id: String, cfg: InstanceConfig, mut modules: Vec<Layer>) -> anyhow::Result<Self> {
        log::info!("Creating container {}", id);

        let source_spec_path = cfg.bundle.join("config.json");
        let spec = Spec::load(source_spec_path)?;

        ensure!(modules.len() == 1, "Only one module is supported");
        let module = modules.pop().unwrap();

        let mut stdout = cfg.open_stdout();

        let writer = move |msg: String| -> Result<i32, HyperlightError> {
            if let Ok(stdout) = stdout.as_mut() {
                stdout.write_all(msg.as_bytes())?;
            }
            Ok(msg.len() as i32)
        };

        let writer = Arc::new(StdMutex::new(writer));

        // Create an uninitialized sandbox with a guest binary
        let sandbox = UninitializedSandbox::new(
            hyperlight_host::GuestBinary::Buffer(module.layer),
            None, // default configuration
            None, // default run options
            Some(&writer), // default host print function
        )?;

        let rootfs = spec
            .root()
            .as_ref()
            .context("rootfs is not set in runtime spec")?
            .path()
            .to_owned();

        log::info!("Done creating container {}", id);

        Ok(Self {
            id,
            rootfs,
            cfg,
            sandbox: Mutex::new(Some(sandbox)),
        })
    }

    async fn start(&self) -> anyhow::Result<u32> {
        log::info!("Running container {}", self.id);

        let sandbox = self.sandbox.lock().await.take().context("failed precondition")?;

        let status = tokio::task::block_in_place(move || -> anyhow::Result<u32> {
            // Initialize sandbox to be able to call host functions
            let mut multi_use_sandbox: MultiUseSandbox = sandbox.evolve(Noop::default())?;

            // Call guest function
            let message = "Hello, World! I am executing inside of a VM :)\n".to_string();
            let _ = multi_use_sandbox.call_guest_function_by_name(
                "PrintOutput", // function must be defined in the guest binary
                ReturnType::Int,
                Some(vec![ParameterValue::String(message.clone())]),
            )?;

            Ok(0)
        })?;

        log::info!("Done running container {}", self.id);
        Ok(status)
    }

    fn pid(&self) -> u32 {
        std::process::id()
    }

    async fn kill(&self, _signal: u32) -> anyhow::Result<()> {
        // no-op
        Ok(())
    }

    async fn delete(&self) -> anyhow::Result<()> {
        // no-op
        Ok(())
    }
}
