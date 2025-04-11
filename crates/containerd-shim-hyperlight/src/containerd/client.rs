#![cfg(unix)]

use std::fmt::Debug;
use std::path::Path;

use containerd_client::services::v1::containers_client::ContainersClient;
use containerd_client::services::v1::content_client::ContentClient;
use containerd_client::services::v1::images_client::ImagesClient;
use containerd_client::services::v1::{
    Container, DeleteContentRequest, GetContainerRequest, GetImageRequest, Image,
    ReadContentRequest,
};
use containerd_client::tonic::transport::Channel;
use containerd_client::{tonic, with_namespace};
use containerd_shimkit::sandbox::error::{Error as ShimError, Result};
use futures::TryStreamExt;
use oci_spec::image::{Arch, Digest, ImageManifest, MediaType, Platform};
use tonic::Request;

use crate::shim::Layer;

#[derive(Debug)]
pub struct Client {
    inner: Channel,
    namespace: String,
}

// sync wrapper implementation from https://tokio.rs/tokio/topics/bridging
impl Client {
    // wrapper around connection that will establish a connection and create a client
    pub async fn connect(
        address: impl AsRef<Path> + std::fmt::Debug,
        namespace: impl Into<String> + std::fmt::Debug,
    ) -> Result<Client> {
        let inner = containerd_client::connect(address.as_ref())
            .await
            .map_err(|err| ShimError::Containerd(err.to_string()))?;

        Ok(Client {
            inner,
            namespace: namespace.into(),
        })
    }

    // wrapper around read that will read the entire content file
    async fn read_content(&self, digest: impl ToString + std::fmt::Debug) -> Result<Vec<u8>> {
        let req = ReadContentRequest {
            digest: digest.to_string(),
            ..Default::default()
        };
        let req = with_namespace!(req, self.namespace);
        ContentClient::new(self.inner.clone())
            .read(req)
            .await
            .map_err(|err| ShimError::Containerd(err.to_string()))?
            .into_inner()
            .map_ok(|msg| msg.data)
            .try_concat()
            .await
            .map_err(|err| ShimError::Containerd(err.to_string()))
    }

    // used in tests to clean up content
    #[allow(dead_code)]
    async fn delete_content(&self, digest: impl ToString + std::fmt::Debug) -> Result<()> {
        let req = DeleteContentRequest {
            digest: digest.to_string(),
        };
        let req = with_namespace!(req, self.namespace);
        ContentClient::new(self.inner.clone())
            .delete(req)
            .await
            .map_err(|err| ShimError::Containerd(err.to_string()))?;
        Ok(())
    }

    async fn get_image(&self, image_name: impl ToString + std::fmt::Debug) -> Result<Image> {
        let name = image_name.to_string();
        let req = GetImageRequest { name };
        let req = with_namespace!(req, self.namespace);
        let image = ImagesClient::new(self.inner.clone())
            .get(req)
            .await
            .map_err(|err| ShimError::Containerd(err.to_string()))?
            .into_inner()
            .image
            .ok_or_else(|| {
                ShimError::Containerd(format!(
                    "failed to get image for image {}",
                    image_name.to_string()
                ))
            })?;
        Ok(image)
    }

    fn extract_image_content_sha(&self, image: &Image) -> Result<String> {
        let digest = image
            .target
            .as_ref()
            .ok_or_else(|| {
                ShimError::Containerd(format!(
                    "failed to get image content sha for image {}",
                    image.name
                ))
            })?
            .digest
            .clone();
        Ok(digest)
    }

    async fn get_container(&self, container_name: impl AsRef<str> + Debug) -> Result<Container> {
        let container_name = container_name.as_ref();
        let id = container_name.to_string();
        let req = GetContainerRequest { id };
        let req = with_namespace!(req, self.namespace);
        let container = ContainersClient::new(self.inner.clone())
            .get(req)
            .await
            .map_err(|err| ShimError::Containerd(err.to_string()))?
            .into_inner()
            .container
            .ok_or_else(|| {
                ShimError::Containerd(format!(
                    "failed to get image for container {container_name}",
                ))
            })?;
        Ok(container)
    }

    async fn get_image_manifest_and_digest(
        &self,
        image_name: &str,
    ) -> Result<(ImageManifest, Digest)> {
        let image = self.get_image(image_name).await?;
        let image_digest = self.extract_image_content_sha(&image)?.try_into()?;
        let manifest =
            ImageManifest::from_reader(self.read_content(&image_digest).await?.as_slice())?;
        Ok((manifest, image_digest))
    }

    // load module will query the containerd store to find an image that has an OS of type 'wasm'
    // If found it continues to parse the manifest and return the layers that contains the WASM modules
    // and possibly other configuration layers.
    pub async fn load_modules(
        &self,
        containerd_id: impl AsRef<str> + Debug,
        supported_layer_types: &[&str],
    ) -> Result<(Vec<Layer>, Platform)> {
        let container = self.get_container(containerd_id).await?;
        let (manifest, _image_digest) =
            self.get_image_manifest_and_digest(&container.image).await?;

        let image_config_descriptor = manifest.config();
        let image_config = self.read_content(image_config_descriptor.digest()).await?;
        let image_config = image_config.as_slice();

        // the only part we care about here is the platform values
        let platform: Platform = serde_json::from_slice(image_config)?;
        let Arch::Wasm = platform.architecture() else {
            log::info!("manifest is not in WASM OCI image format");
            return Ok((vec![], platform));
        };

        log::info!("found manifest with WASM OCI image format");
        // This label is unique across runtimes and version of the shim running
        // a precompiled component/module will not work across different runtimes or versions

        let configs = manifest
            .layers()
            .iter()
            .filter(|x| is_wasm_layer(x.media_type(), supported_layer_types))
            .collect::<Vec<_>>();

        if configs.is_empty() {
            log::info!("no WASM layers found in OCI image");
            return Ok((vec![], platform));
        }

        log::info!("using OCI layers");

        let mut layers = vec![];
        for config in configs {
            let layer = self.read_original_layer(config).await?;
            layers.push(layer);
        }
        return Ok((layers, platform));
    }

    async fn read_original_layer(
        &self,
        config: &oci_spec::image::Descriptor,
    ) -> Result<Layer, ShimError> {
        let digest = config.digest();
        log::debug!("loading digest: {} ", digest);
        self.read_content(digest).await.map(|module| Layer {
            config: config.clone(),
            layer: module,
        })
    }
}

fn is_wasm_layer(media_type: &MediaType, supported_layer_types: &[&str]) -> bool {
    let supported = supported_layer_types.contains(&media_type.to_string().as_str());
    log::debug!(
        "layer type {} is supported: {}",
        media_type.to_string().as_str(),
        supported
    );
    supported
}
