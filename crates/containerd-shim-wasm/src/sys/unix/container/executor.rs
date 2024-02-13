use std::fs::File;
use std::io::Read;
use std::os::unix::prelude::PermissionsExt;
use std::path::PathBuf;
use std::sync::OnceLock;

use anyhow::{bail, Context, Result};
use libcontainer::workload::default::DefaultExecutor;
use libcontainer::workload::{
    Executor as LibcontainerExecutor, ExecutorError as LibcontainerExecutorError,
    ExecutorValidationError,
};
use oci_spec::image::Platform;
use oci_spec::runtime::Spec;

use crate::container::{Engine, PathResolve, RuntimeContext, Source, Stdio, WasiContext};
use crate::sandbox::oci::WasmLayer;

#[derive(Clone)]
enum InnerExecutor {
    Wasm,
    Linux,
    CantHandle,
}

macro_rules! run_in_new_thread {
    // Run code in a new thread so that runtimes that use tokio can do so.
    ($expression:expr) => {
        std::thread::scope(move |s| s.spawn(move || $expression).join().unwrap())
    };
}

#[derive(Clone)]
pub(crate) struct Executor<E: Engine> {
    engine: E,
    stdio: Stdio,
    inner: OnceLock<InnerExecutor>,
    wasm_layers: Vec<WasmLayer>,
    platform: Platform,
}

impl<E: Engine> LibcontainerExecutor for Executor<E> {
    fn validate(&self, spec: &Spec) -> Result<(), ExecutorValidationError> {
        // We can handle linux container. We delegate wasm container to the engine.
        match self.inner(spec) {
            InnerExecutor::CantHandle => Err(ExecutorValidationError::CantHandle(E::name())),
            _ => Ok(()),
        }
    }

    fn exec(&self, spec: &Spec) -> Result<(), LibcontainerExecutorError> {
        // If it looks like a linux container, run it as a linux container.
        // Otherwise, run it as a wasm container
        match self.inner(spec) {
            InnerExecutor::CantHandle => Err(LibcontainerExecutorError::CantHandle(E::name())),
            InnerExecutor::Linux => {
                log::info!("executing linux container");
                self.stdio.take().redirect().unwrap();
                DefaultExecutor {}.exec(spec)
            }
            InnerExecutor::Wasm => {
                log::info!("calling start function");
                match run_in_new_thread!(self.engine.run_wasi(&self.ctx(spec), self.stdio.take())) {
                    Ok(code) => std::process::exit(code),
                    Err(err) => {
                        log::info!("error running start function: {err}");
                        std::process::exit(137)
                    }
                };
            }
        }
    }
}

impl<E: Engine> Executor<E> {
    pub fn new(engine: E, stdio: Stdio, wasm_layers: Vec<WasmLayer>, platform: Platform) -> Self {
        Self {
            engine,
            stdio,
            inner: Default::default(),
            wasm_layers,
            platform,
        }
    }

    fn ctx<'a>(&'a self, spec: &'a Spec) -> WasiContext<'a> {
        let wasm_layers = &self.wasm_layers;
        let platform = &self.platform;
        WasiContext {
            spec,
            wasm_layers,
            platform,
        }
    }

    fn inner(&self, spec: &Spec) -> &InnerExecutor {
        self.inner.get_or_init(|| {
            let ctx = self.ctx(spec);
            if is_linux_container(&ctx).is_ok() {
                InnerExecutor::Linux
            } else if run_in_new_thread! { self.engine.can_handle(&ctx).is_ok() } {
                InnerExecutor::Wasm
            } else {
                InnerExecutor::CantHandle
            }
        })
    }
}

fn is_linux_container(ctx: &impl RuntimeContext) -> Result<()> {
    if let Source::Oci(_) = ctx.entrypoint().source {
        bail!("the entry point contains wasm layers")
    };

    let executable = ctx
        .entrypoint()
        .arg0
        .context("no entrypoint provided")?
        .resolve_in_path()
        .find_map(|p| -> Option<PathBuf> {
            let mode = p.metadata().ok()?.permissions().mode();
            (mode & 0o001 != 0).then_some(p)
        })
        .context("entrypoint not found")?;

    // check the shebang and ELF magic number
    // https://en.wikipedia.org/wiki/Executable_and_Linkable_Format#File_header
    let mut buffer = [0; 4];
    File::open(executable)?.read_exact(&mut buffer)?;

    match buffer {
        [0x7f, 0x45, 0x4c, 0x46] => Ok(()), // ELF magic number
        [0x23, 0x21, ..] => Ok(()),         // shebang
        _ => bail!("not a valid script or elf file"),
    }
}
