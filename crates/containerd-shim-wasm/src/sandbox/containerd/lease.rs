#![cfg(unix)]

use std::path::Path;

use anyhow::Context as _;
use containerd_client::services::v1::leases_client::LeasesClient;
use containerd_client::{tonic, with_namespace};
use tonic::Request;

// Adds lease info to grpc header
// https://github.com/containerd/containerd/blob/8459273f806e068e1a6bacfaf1355bbbad738d5e/docs/garbage-collection.md#using-grpc
#[macro_export]
macro_rules! with_lease {
    ($req : ident, $ns: expr, $lease_id: expr) => {{
        let mut req = Request::new($req);
        let md = req.metadata_mut();
        // https://github.com/containerd/containerd/blob/main/namespaces/grpc.go#L27
        md.insert("containerd-namespace", $ns.parse().unwrap());
        md.insert("containerd-lease", $lease_id.parse().unwrap());
        req
    }};
}

#[derive(Debug)]
pub(crate) struct LeaseGuard {
    released: bool,
    pub(crate) lease_id: String,
    pub(crate) namespace: String,
    pub(crate) address: String,
}

impl LeaseGuard {
    pub fn new(
        lease_id: impl Into<String>,
        namespace: impl Into<String>,
        address: impl Into<String>,
    ) -> Self {
        Self {
            released: false,
            lease_id: lease_id.into(),
            namespace: namespace.into(),
            address: address.into(),
        }
    }

    pub async fn release(mut self) -> anyhow::Result<()> {
        self.released = true;
        release(&self.lease_id, &self.address, &self.namespace).await
    }
}

// Provides a best effort for dropping a lease of the content.  If the lease cannot be dropped, it will log a warning
impl Drop for LeaseGuard {
    fn drop(&mut self) {
        if self.released {
            return;
        }

        let lease_id = self.lease_id.clone();
        let address = self.address.clone();
        let namespace = self.namespace.clone();

        tokio::spawn(async move {
            match release(lease_id, address, namespace).await {
                Ok(()) => log::debug!("removed lease"),
                Err(err) => log::error!("{err}"),
            }
        });
    }
}

async fn release(
    lease_id: impl Into<String>,
    address: impl AsRef<Path>,
    namespace: impl AsRef<str>,
) -> anyhow::Result<()> {
    let id = lease_id.into();
    let channel = containerd_client::connect(address.as_ref())
        .await
        .context("Failed to connect to containerd. Lease may not be deleted.")?;

    let mut client = LeasesClient::new(channel);

    let req = containerd_client::services::v1::DeleteRequest { id, sync: false };
    let req = with_namespace!(req, namespace.as_ref());
    client.delete(req).await.context("Failed to remove lease")?;

    Ok(())
}
