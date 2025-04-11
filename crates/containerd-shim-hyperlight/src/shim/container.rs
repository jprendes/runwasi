use containerd_shimkit::sandbox::InstanceConfig;

use oci_spec::image::Descriptor;

#[derive(Clone, Debug)]
pub struct Layer {
    pub config: Descriptor,
    pub layer: Vec<u8>,
}

#[trait_variant::make(Send)]
pub trait Container: Send + Sync + 'static {
    async fn new(id: String, cfg: InstanceConfig, modules: Vec<Layer>) -> anyhow::Result<Self>
    where
        Self: Sized;

    fn pid(&self) -> u32;
    async fn start(&self) -> anyhow::Result<u32>;
    async fn kill(&self, _signal: u32) -> anyhow::Result<()>;
    async fn delete(&self) -> anyhow::Result<()>;
}
