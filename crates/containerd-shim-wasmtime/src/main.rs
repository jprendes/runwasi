use containerd_shim_wasm::sandbox::shim::Local;
use containerd_shim_wasm::shimkit::utils::cri_sandbox_id;
use containerd_shim_wasmtime::instance::WasmtimeEngine;
use containerd_shim_wasmtime::WasmtimeInstance;
use tokio::signal::ctrl_c;

#[containerd_shim_wasm::main(flavor = "current_thread", shimkit = "containerd_shim_wasm::shimkit")]
async fn main(args: containerd_shim_wasm::shimkit::args::Arguments) -> anyhow::Result<()> {
    env_logger::init();

    let address = if args.is_interactive() {
        log::info!("Running shim interactively, a debug address will be used");
        args.socket_address_debug("debug")
    } else if let Some(id) = cri_sandbox_id() {
        args.socket_address(&id)
    } else {
        args.socket_address(&args.id)
    };

    #[cfg(unix)]
    let _ = tokio::fs::remove_file(&address).await;

    log::info!("getting publisher {:?}", args.ttrpc_address);
    let publisher = args.event_publisher().await?;
    log::info!("got publisher");
    let instance = WasmtimeEngine::default();
    let server =
        Local::<WasmtimeInstance>::new(instance, publisher, &args.namespace, &args.grpc_address);
    let handle = args.serve(&address, server).await?;

    log::info!("Listening on {}", address.display());
    log::info!("Press Ctrl+C to exit.");

    let controller = handle.controller();
    tokio::spawn(async move {
        if ctrl_c().await.is_err() {
            log::error!("Failed to wait for Ctrl+C.");
        }
        log::info!("Shutting down server");
        controller.shutdown();
    });

    handle.await.expect("Error shutting down server");

    Ok(())

    //shim_main::<WasmtimeInstance>("wasmtime", version!(), revision!(), "v1", None);
}
