use std::fs::{create_dir_all, remove_file};
use std::path::PathBuf;

use containerd_shim::{parse, run, Config};
use tokio::signal::unix::{signal, SignalKind};
use trapeze::Server;

use crate::sandbox::manager::{Manager, Shim};
use crate::sandbox::shim::Local;
use crate::sandbox::{Instance, ManagerService, ShimCli};

pub mod r#impl {
    pub use git_version::git_version;
}

pub use crate::{revision, version};

#[macro_export]
macro_rules! version {
    () => {
        env!("CARGO_PKG_VERSION")
    };
}

#[macro_export]
macro_rules! revision {
    () => {
        match $crate::sandbox::cli::r#impl::git_version!(
            args = ["--match=:", "--always", "--abbrev=15", "--dirty=.m"],
            fallback = "",
        ) {
            "" => None,
            version => Some(version),
        }
    };
}

pub fn shim_main<'a, I>(
    name: &str,
    version: &str,
    revision: impl Into<Option<&'a str>>,
    shim_version: impl Into<Option<&'a str>>,
    config: Option<Config>,
) where
    I: 'static + Instance + Sync + Send,
    I::Engine: Default,
{
    tokio::runtime::Builder::new_current_thread()
    .enable_all()
    .thread_keep_alive(std::time::Duration::ZERO)
    .build()
    .unwrap()
    .block_on(async {
        let os_args: Vec<_> = std::env::args_os().collect();
        let flags = parse(&os_args[1..]).unwrap();
        let argv0 = PathBuf::from(&os_args[0]);
        let argv0 = argv0.file_stem().unwrap_or_default().to_string_lossy();

        if flags.version {
            println!("{argv0}:");
            println!("  Runtime: {name}");
            println!("  Version: {version}");
            println!("  Revision: {}", revision.into().unwrap_or("<none>"));
            println!();

            std::process::exit(0);
        }
        let shim_version = shim_version.into().unwrap_or("v1");

        let lower_name = name.to_lowercase();
        let shim_cli = format!("containerd-shim-{lower_name}-{shim_version}");
        let shim_client = format!("containerd-shim-{lower_name}d-{shim_version}");
        let shim_daemon = format!("containerd-{lower_name}d");
        let shim_id = format!("io.containerd.{lower_name}.{shim_version}");

        match argv0.to_lowercase() {
            s if s == shim_cli => {
                run::<ShimCli<I>>(&shim_id, config).await;
            }
            s if s == shim_client => {
                run::<Shim>(&shim_client, config).await;
            }
            s if s == shim_daemon => {
                eprintln!("starting up!");
                let service: ManagerService<Local<I>> = Default::default();

                let mut interrupt = signal(SignalKind::interrupt()).unwrap();

                remove_file("/run/io.containerd.wasmwasi.v1/manager.sock").unwrap();
                create_dir_all("/run/io.containerd.wasmwasi.v1/").unwrap();
                let mut server = Server::new();
                let server = server
                    .register(trapeze::service!(service: Manager))
                    .bind("unix:///run/io.containerd.wasmwasi.v1/manager.sock");

                eprintln!("server started!");
                eprintln!("press Ctrl+C to shutdown.");

                tokio::select! {
                    s = server => { s.unwrap() },
                    _ = interrupt.recv() => {},
                }

                eprintln!("shutdown");
            }
            _ => {
                eprintln!("error: unrecognized binary name, expected one of {shim_cli}, {shim_client}, or {shim_daemon}.");
                std::process::exit(1);
            }
        }
    });
}
