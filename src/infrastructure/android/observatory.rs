use std::sync::Arc;

use vpn_application::client::ClientError;

use crate::client_engine::{SelectorControl, SelectorControlFuture};

use super::AndroidPlatform;

pub(super) struct AndroidSelectorControl {
    hooks: Arc<AndroidPlatform>,
    handle: i64,
}

impl AndroidSelectorControl {
    pub(super) fn new(hooks: Arc<AndroidPlatform>) -> Result<Self, ClientError> {
        let handle = hooks
            .tunnel_handle()
            .map_err(|_| ClientError::EngineReadiness)?;
        if handle <= 0 {
            return Err(ClientError::EngineReadiness);
        }
        Ok(Self { hooks, handle })
    }
}

impl SelectorControl for AndroidSelectorControl {
    fn ready(&self) -> SelectorControlFuture<'_> {
        Box::pin(async move {
            let hooks = Arc::clone(&self.hooks);
            let handle = self.handle;
            tokio::task::spawn_blocking(move || hooks.selector_ready(handle))
                .await
                .map_err(|_| ClientError::EngineReadiness)?
                .map_err(|_| ClientError::EngineReadiness)?
                .then_some(())
                .ok_or(ClientError::UnsupportedFeature)
        })
    }

    fn select<'a>(&'a self, group: &'a str, member: &'a str) -> SelectorControlFuture<'a> {
        Box::pin(async move {
            let hooks = Arc::clone(&self.hooks);
            let handle = self.handle;
            let group = group.to_owned();
            let member = member.to_owned();
            tokio::task::spawn_blocking(move || hooks.select_outbound(handle, &group, &member))
                .await
                .map_err(|_| ClientError::EngineReadiness)?
                .map_err(|_| ClientError::EngineReadiness)?
                .then_some(())
                .ok_or(ClientError::EngineReadiness)
        })
    }
}
