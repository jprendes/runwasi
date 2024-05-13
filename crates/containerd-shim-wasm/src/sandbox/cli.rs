use std::path::PathBuf;
use std::sync::mpsc::channel;
use std::sync::Arc;

use containerd_shim::{parse, run, Config};
use opentelemetry::global::{set_text_map_propagator, shutdown_tracer_provider};
use opentelemetry_sdk::propagation::TraceContextPropagator;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::{EnvFilter, Registry};
use ttrpc::Server;

use crate::sandbox::manager::Shim;
use crate::sandbox::shim::{init_tracer, Local};
use crate::sandbox::{Instance, ManagerService, ShimCli};
use crate::services::sandbox_ttrpc::{create_manager, Manager};

const OTEL_EXPORTER_OTLP_ENDPOINT: &str = "OTEL_EXPORTER_OTLP_ENDPOINT";

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

/// Main entry point for the shim with OpenTelemetry tracing.
///
/// It parses the `OTEL_EXPORTER_OTLP_ENDPOINT` environment variable to determine
/// if the shim should be started with OpenTelemetry tracing.
///
/// If the environment variable is not set, the shim will be started without tracing.
/// If the environment variable is empty, the shim will be started without tracing.
#[cfg(feature = "opentelemetry")]
pub fn shim_main_with_otel<'a, I>(
    name: &str,
    version: &str,
    revision: impl Into<Option<&'a str>>,
    shim_version: impl Into<Option<&'a str>>,
    config: Option<Config>,
) where
    I: 'static + Instance + Sync + Send,
    I::Engine: Default,
{
    match std::env::var(OTEL_EXPORTER_OTLP_ENDPOINT) {
        Ok(otel_endpoint) => {
            let tracer = init_tracer(&otel_endpoint, name).expect("Failed to initialize tracer.");
            let telemetry = tracing_opentelemetry::layer().with_tracer(tracer);
            set_text_map_propagator(TraceContextPropagator::new());

            // Create an environment filter
            let filter = EnvFilter::try_new("info,h2=off") // Set default level to `info` and exclude all traces from `h2`
                .expect("Invalid filter directive");

            let subscriber = Registry::default().with(telemetry).with(filter);

            tracing::subscriber::set_global_default(subscriber)
                .expect("setting default subscriber failed");
            shim_main::<I>(name, version, revision, shim_version, config);
            shutdown_tracer_provider();
        }
        Err(_) => {
            shim_main::<I>(name, version, revision, shim_version, config);
        }
    }
}

#[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip_all, level = "Info"))]
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
            run::<ShimCli<I>>(&shim_id, config);
        }
        s if s == shim_client => {
            run::<Shim>(&shim_client, config);
        }
        s if s == shim_daemon => {
            log::info!("starting up!");
            let s: ManagerService<Local<I>> = Default::default();
            let s = Arc::new(Box::new(s) as Box<dyn Manager + Send + Sync>);
            let service = create_manager(s);

            let mut server = Server::new()
                .bind("unix:///run/io.containerd.wasmwasi.v1/manager.sock")
                .expect("failed to bind to socket")
                .register_service(service);

            server.start().expect("failed to start daemon");
            log::info!("server started!");
            let (_tx, rx) = channel::<()>();
            rx.recv().unwrap();
        }
        _ => {
            eprintln!("error: unrecognized binary name, expected one of {shim_cli}, {shim_client}, or {shim_daemon}.");
            std::process::exit(1);
        }
    }
}
