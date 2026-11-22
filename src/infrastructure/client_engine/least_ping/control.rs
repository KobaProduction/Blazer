use std::future::Future;
#[cfg(not(target_os = "android"))]
use std::net::SocketAddr;
use std::pin::Pin;

use vpn_domain::client::ClientError;
#[cfg(not(target_os = "android"))]
use vpn_domain::client::Secret;

pub type SelectorControlFuture<'a> =
    Pin<Box<dyn Future<Output = Result<(), ClientError>> + Send + 'a>>;

/// Control of named selectors in the same engine session as the observer.
/// Implementations must not block while polling their future. The monitor owns
/// cancellation and bounds each control operation independently of HTTP probes.
pub trait SelectorControl: Send + Sync {
    fn ready(&self) -> SelectorControlFuture<'_>;
    fn select<'a>(&'a self, group: &'a str, member: &'a str) -> SelectorControlFuture<'a>;
}

#[cfg(not(target_os = "android"))]
pub(super) struct HttpSelectorControl {
    address: SocketAddr,
    secret: Secret,
    client: reqwest::Client,
}

#[cfg(not(target_os = "android"))]
impl HttpSelectorControl {
    pub(super) fn new(address: SocketAddr, secret: Secret) -> Result<Self, ClientError> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(2))
            .build()
            .map_err(|_| ClientError::EngineStart)?;
        Ok(Self {
            address,
            secret,
            client,
        })
    }
}

#[cfg(not(target_os = "android"))]
impl SelectorControl for HttpSelectorControl {
    fn ready(&self) -> SelectorControlFuture<'_> {
        Box::pin(async move {
            let response = self
                .client
                .get(format!("http://{}/proxies", self.address))
                .bearer_auth(self.secret.expose())
                .send()
                .await
                .map_err(|_| ClientError::EngineReadiness)?;
            if response.status() == reqwest::StatusCode::OK {
                Ok(())
            } else {
                Err(ClientError::EngineReadiness)
            }
        })
    }

    fn select<'a>(&'a self, group: &'a str, member: &'a str) -> SelectorControlFuture<'a> {
        Box::pin(async move {
            let body = serde_json::to_vec(&serde_json::json!({"name": member}))
                .map_err(|_| ClientError::EngineReadiness)?;
            let response = self
                .client
                .put(format!("http://{}/proxies/{group}", self.address))
                .bearer_auth(self.secret.expose())
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body)
                .send()
                .await
                .map_err(|_| ClientError::EngineReadiness)?;
            if response.status() == reqwest::StatusCode::NO_CONTENT {
                Ok(())
            } else {
                Err(ClientError::EngineReadiness)
            }
        })
    }
}
