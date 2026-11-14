//! Host-injected execution ownership; editor services do not start native apps.
use async_trait::async_trait;
use webtest_provider::ProviderError;

#[async_trait]
pub trait RunLifecycle: Send + Sync {
    /// Acquire a serialized host run and start its dependencies. Failed startup
    /// must finish its own partial cleanup before returning an error.
    async fn begin(&self) -> Result<Box<dyn RunLease>, ProviderError>;
}

#[async_trait]
pub trait RunLease: Send {
    /// Called after runner-owned cleanup, including failed and cancelled runs.
    async fn finish(self: Box<Self>) -> Result<(), ProviderError>;
}
