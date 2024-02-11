pub trait WithTimeout {
    type Output;
    fn with_timeout(
        self,
        t: std::time::Duration,
    ) -> impl std::future::Future<Output = Option<Self::Output>> + Send;
}

impl<F: std::future::Future + Send> WithTimeout for F {
    type Output = F::Output;
    async fn with_timeout(self, t: std::time::Duration) -> Option<Self::Output> {
        tokio::time::timeout(t, self).await.ok()
    }
}
