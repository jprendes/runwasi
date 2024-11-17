use std::future::Future;
use std::ops::{Deref, DerefMut};

use chrono::{DateTime, TimeZone};
use shimkit::types::prost::Timestamp;
use tokio::task::JoinHandle;
use tokio::time::{sleep, Duration};

pub struct AbortOnDropJoinHandle<T>(JoinHandle<T>);

impl<T> Drop for AbortOnDropJoinHandle<T> {
    fn drop(&mut self) {
        self.0.abort()
    }
}

impl<T> Deref for AbortOnDropJoinHandle<T> {
    type Target = JoinHandle<T>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for AbortOnDropJoinHandle<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

pub trait AbortOnDrop {
    type Output;
    #[allow(dead_code)]
    fn abort_on_drop(self) -> AbortOnDropJoinHandle<Self::Output>;
}

impl<T> AbortOnDrop for JoinHandle<T> {
    type Output = T;
    fn abort_on_drop(self) -> AbortOnDropJoinHandle<Self::Output> {
        AbortOnDropJoinHandle(self)
    }
}

pub trait WithTimeout {
    type Output;
    fn with_timeout(
        self,
        t: impl Into<Option<Duration>> + Send,
    ) -> impl Future<Output = Option<Self::Output>> + Send;
}

impl<F: Future + Send> WithTimeout for F {
    type Output = F::Output;
    async fn with_timeout(self, t: impl Into<Option<Duration>>) -> Option<Self::Output> {
        let t = t.into().unwrap_or(Duration::ZERO);
        tokio::select! {
            _ = sleep(t) => None,
            val = self => Some(val)
        }
    }
}

pub trait ToTimestamp {
    fn to_timestamp(self) -> Timestamp;
}

impl<Tz: TimeZone> ToTimestamp for DateTime<Tz> {
    fn to_timestamp(self) -> Timestamp {
        Timestamp {
            seconds: self.timestamp(),
            nanos: self.timestamp_subsec_nanos() as i32,
            ..Default::default()
        }
    }
}
