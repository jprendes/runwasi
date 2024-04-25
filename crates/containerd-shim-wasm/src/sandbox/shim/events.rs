use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, TimeZone};
use containerd_shim::event::Event;
use containerd_shim::protos::prost_types::{Any, Timestamp};
use containerd_shim::publisher::RemotePublisher;
use log::warn;

#[async_trait]
pub trait EventSender: Clone + Send + Sync + 'static {
    async fn send(&self, event: impl Event);
}

#[derive(Clone)]
pub struct RemoteEventSender {
    inner: Arc<Inner>,
}

struct Inner {
    namespace: String,
    publisher: RemotePublisher,
}

impl RemoteEventSender {
    pub fn new(namespace: impl AsRef<str>, publisher: RemotePublisher) -> RemoteEventSender {
        let namespace = namespace.as_ref().to_string();
        RemoteEventSender {
            inner: Arc::new(Inner {
                namespace,
                publisher,
            }),
        }
    }
}

#[async_trait]
impl EventSender for RemoteEventSender {
    async fn send(&self, event: impl Event) {
        let topic = event.topic();
        let event = Any::from_msg(&event).unwrap();
        let publisher = &self.inner.publisher;
        if let Err(err) = publisher
            .publish(&topic, &self.inner.namespace, event)
            .await
        {
            warn!("failed to publish event, topic: {}: {}", &topic, err)
        }
    }
}

pub(super) trait ToTimestamp {
    fn to_timestamp(self) -> Timestamp;
}

impl<Tz: TimeZone> ToTimestamp for DateTime<Tz> {
    fn to_timestamp(self) -> Timestamp {
        Timestamp {
            seconds: self.timestamp(),
            nanos: self.timestamp_subsec_nanos() as i32,
        }
    }
}
