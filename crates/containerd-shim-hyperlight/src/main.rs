use container::SomeContainer;
use containerd_shimkit::shim_version;
use shim::Instance;

mod container;
mod containerd;
mod shim;

fn main() {
    //console_subscriber::init();
    let config = containerd_shimkit::Config {
        no_setup_logger: false,
        default_log_level: "info".into(),
        no_reaper: false,
        no_sub_reaper: false,
    };
    containerd_shimkit::sandbox::cli::shim_main::<Instance<SomeContainer>>(
        "hyperlight",
        shim_version!(),
        Some(config),
    );
}
