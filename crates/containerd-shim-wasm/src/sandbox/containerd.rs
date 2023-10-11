#![cfg(unix)]
use containerd_client;
use containerd_client::services::v1::containers_client::ContainersClient;
use containerd_client::services::v1::content_client::ContentClient;
use containerd_client::services::v1::images_client::ImagesClient;
use containerd_client::services::v1::{
    GetContainerRequest, GetImageRequest, ReadContentRequest, ReadContentResponse,
};
use containerd_client::tonic::transport::Channel;
use containerd_client::tonic::{self, Streaming};
use containerd_client::with_namespace;
use oci_spec::image::ImageManifest;
use tokio::runtime::Runtime;
use tonic::Request;

use crate::sandbox::error::{Error as ShimError, Result};
use crate::sandbox::oci::{self, OciArtifact};

pub struct Client {
    inner: Channel,
    rt: Runtime,
    namespace: String,
}

// sync wrapper implementation from https://tokio.rs/tokio/topics/bridging
impl Client {
    // wrapper around connection that will establish a connection and create a client
    pub fn connect(address: String, namespace: &str) -> Result<Client> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;

        let inner = rt.block_on(async {
            containerd_client::connect(address)
                .await
                .map_err(|err| ShimError::Others(err.to_string()))
        })?;

        Ok(Client {
            inner,
            rt,
            namespace: namespace.to_string(),
        })
    }

    fn content(&self) -> ContentClient<Channel> {
        ContentClient::new(self.inner.clone())
    }

    fn images(&self) -> ImagesClient<Channel> {
        ImagesClient::new(self.inner.clone())
    }

    fn containers(&self) -> ContainersClient<Channel> {
        ContainersClient::new(self.inner.clone())
    }

    // wrapper around read that will read the entire content file
    pub fn read_content(&mut self, digest: String) -> Result<Vec<u8>> {
        let resp: Result<Vec<u8>> = self.rt.block_on(async {
            let req = ReadContentRequest {
                digest,
                offset: 0,
                size: 0,
            };
            let req = with_namespace!(req, self.namespace);
            let response: tonic::Response<Streaming<ReadContentResponse>> = self
                .content()
                .read(req)
                .await
                .map_err(|err| ShimError::Others(err.to_string()))?;
            let mut resp_stream = response.into_inner();

            let mut data = vec![];
            while let Some(mut next_message) = resp_stream
                .message()
                .await
                .map_err(|err| ShimError::Others(err.to_string()))?
            {
                data.append(&mut next_message.data);
            }

            Ok(data)
        });

        resp
    }

    pub fn get_image_content_sha(&mut self, image_name: String) -> Option<String> {
        let resp: Result<Option<String>> = self.rt.block_on(async {
            let req = GetImageRequest { name: image_name };
            let req = with_namespace!(req, self.namespace);
            let resp = self
                .images()
                .get(req)
                .await
                .map_err(|err| ShimError::Others(err.to_string()))?;

            let image_response = resp.into_inner();
            match image_response.image {
                Some(i) => {
                    let desc = i.target.unwrap();
                    Ok(Some(desc.digest))
                }
                None => Ok(None),
            }
        });
        match resp {
            Ok(layer_option) => layer_option,
            Err(_) => None,
        }
    }

    pub fn get_oci_image(&mut self, container_name: String) -> Option<String> {
        let resp: Result<Option<String>> = self.rt.block_on(async {
            let req = GetContainerRequest { id: container_name };
            let req = with_namespace!(req, self.namespace);
            let resp = self
                .containers()
                .get(req)
                .await
                .map_err(|err| ShimError::Others(err.to_string()))?;

            let container_response = resp.into_inner();
            match container_response.container {
                Some(c) => Ok(Some(c.image)),
                None => Ok(None),
            }
        });

        match resp {
            Ok(layer_option) => layer_option,
            Err(_) => None,
        }
    }

    // load module will query the containerd store to find an image that has an ArtifactType of WASM OCI Artifact
    // If found it continues to parse the manifest and return the layers that contains the WASM modules
    // and possibly other configuration artifacts
    pub fn load_modules(&mut self, containerd_id: String) -> Option<Vec<oci::OciArtifact>> {
        let image_name = self.get_oci_image(containerd_id.clone())?;
        let container_digest = self.get_image_content_sha(image_name)?;
        let manifest = match self.read_content(container_digest) {
            Ok(m) => m,
            Err(_) => {
                log::warn!("failed to read manifest");
                return None;
            }
        };
        let manifest = match ImageManifest::from_reader(manifest.as_slice()) {
            Ok(m) => m,
            Err(_) => {
                log::warn!("failed to parse manifest");
                return None;
            }
        };

        match manifest.artifact_type() {
            Some(oci_spec::image::MediaType::Other(s))
                if s == oci::COMPONENT_ARTIFACT_TYPE || s == oci::MODULE_ARTIFACT_TYPE =>
            {
                log::info!("manifest with OCI Artifact of type {}", s)
            }
            Some(m) => {
                log::info!("manifest is not an known OCI Artifact: {}", m);
                return None;
            }
            None => {
                log::info!("manifest is not an OCI Artifact");
                return None;
            }
        }

        let mut oci_artifacts = Vec::<oci::OciArtifact>::new();
        for layer in manifest.layers().iter() {
            let module = match self.read_content(layer.digest().clone()) {
                Ok(m) => m,
                Err(_) => {
                    log::warn!("failed to read module layer");
                    return None;
                }
            };
            oci_artifacts.push(OciArtifact {
                config: layer.clone(),
                layer: module,
            });
        }

        Some(oci_artifacts)
    }
}
