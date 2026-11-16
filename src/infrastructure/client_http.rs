use std::cell::OnceCell;
use std::error::Error;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

use percent_encoding::percent_decode_str;
use reqwest::header::{
    CACHE_CONTROL, CONTENT_TYPE, ETAG, HeaderMap, HeaderName, HeaderValue, IF_NONE_MATCH, LOCATION,
    VARY,
};
use reqwest::{Client, Response, StatusCode, redirect::Policy};
use serde_json::Value;
use tokio::runtime::{Builder, Runtime};
use tokio::time::{Instant, timeout_at};
use url::{Host, Url};
use vpn_application::MAX_SUBSCRIPTION_BYTES;
use vpn_application::client::{Cancellation, ClientError, SubscriptionSourcePort};
use vpn_application::provider::{FetchError, FetchOutcome, FetchRequest};
use vpn_domain::client::{Secret, SubscriptionHttpError};
use vpn_domain::provider::{MAX_METADATA_VALUE_BYTES, ProviderEndpoint, ProviderMetadata};

use crate::{client_store, provider};

const MAX_REDIRECTS: usize = 3;
const MAX_ERROR_BODY_BYTES: usize = 1024;
const MAX_ERROR_DETAIL_BYTES: usize = 96;
const ERROR_BODY_READ_BUDGET: Duration = Duration::from_millis(200);
const DEVICE_ID_HEADER: HeaderName = HeaderName::from_static("x-hwid");

pub struct HttpSubscriptionSource {
    data_directory: PathBuf,
    transport: OnceCell<HttpTransport>,
    device_id: OnceCell<HeaderValue>,
}

struct HttpTransport {
    client: Client,
    runtime: Runtime,
}

impl HttpSubscriptionSource {
    pub fn new(data_directory: PathBuf) -> Self {
        Self {
            data_directory,
            transport: OnceCell::new(),
            device_id: OnceCell::new(),
        }
    }

    fn transport(&self) -> Result<&HttpTransport, ClientError> {
        if let Some(transport) = self.transport.get() {
            return Ok(transport);
        }
        let transport = HttpTransport::new()?;
        Ok(self.transport.get_or_init(|| transport))
    }

    fn device_id(&self, url: &Url) -> Result<Option<&HeaderValue>, ClientError> {
        if !permits_device_id(url) {
            return Ok(None);
        }
        if let Some(value) = self.device_id.get() {
            return Ok(Some(value));
        }
        let device_id = client_store::subscription_device_id(&self.data_directory)?;
        let mut value =
            HeaderValue::from_str(device_id.expose()).map_err(|_| ClientError::StorageCorrupt)?;
        value.set_sensitive(true);
        Ok(Some(self.device_id.get_or_init(|| value)))
    }
}

impl HttpTransport {
    fn new() -> Result<Self, ClientError> {
        let runtime = Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| ClientError::Network)?;
        let client = {
            let _guard = runtime.enter();
            let builder = Client::builder();
            #[cfg(target_os = "android")]
            let builder = android_tls(builder)?;
            builder
                .connect_timeout(Duration::from_secs(4))
                .redirect(Policy::none())
                .pool_max_idle_per_host(0)
                .no_proxy()
                .referer(false)
                .user_agent("VPN-Client/0.1")
                .build()
                .map_err(|_| ClientError::Network)?
        };
        Ok(Self { client, runtime })
    }

    async fn response(
        &self,
        original_url: &Url,
        follow: bool,
        version: Option<HeaderValue>,
        device_id: Option<&HeaderValue>,
        cancel: &Cancellation,
        deadline: Instant,
    ) -> Result<Response, FetchError> {
        let mut url = original_url.clone();
        for hop in 0..=MAX_REDIRECTS {
            let condition = version.as_ref().filter(|_| hop == 0);
            let mut request = self.client.get(url.clone());
            if let Some(value) = condition {
                request = request.header(IF_NONE_MATCH, value);
            }
            if let Some(value) = device_id {
                request = request.header(DEVICE_ID_HEADER, value);
            }
            let response = tokio::select! {
                biased;
                () = cancel.cancelled() => return Err(rejected(ClientError::Cancelled)),
                response = timeout_at(deadline, request.send()) => {
                    response.map_err(|_| FetchError::Timeout)?
                        .map_err(|error| http_error(&error, url.scheme()))?
                },
            };
            let status = response.status();
            if status.is_success() || (status == StatusCode::NOT_MODIFIED && condition.is_some()) {
                return Ok(response);
            }
            if follow
                && hop < MAX_REDIRECTS
                && status.is_redirection()
                && status != StatusCode::NOT_MODIFIED
                && let Some(next) = same_origin_redirect(&url, &response)
            {
                url = next;
                continue;
            }
            let error = response_error(
                response,
                original_url,
                condition,
                device_id,
                cancel,
                deadline,
            )
            .await?;
            return Err(FetchError::HttpStatus(error));
        }
        Err(rejected(ClientError::Network))
    }
}

#[cfg(target_os = "android")]
fn android_tls(builder: reqwest::ClientBuilder) -> Result<reqwest::ClientBuilder, ClientError> {
    let roots = rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|_| ClientError::Tls)?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok(builder.tls_backend_preconfigured(config))
}

impl SubscriptionSourcePort for HttpSubscriptionSource {
    fn provider_title(&self, url: &str, metadata: &ProviderMetadata) -> Option<String> {
        provider::provider_title(url, metadata)
    }

    fn endpoint(&self, url: &str) -> Result<ProviderEndpoint, ClientError> {
        provider::endpoint(url)
    }

    fn replace_domain(
        &self,
        primary: &ProviderEndpoint,
        domain: &str,
    ) -> Result<ProviderEndpoint, ClientError> {
        provider::replace_domain(primary, domain)
    }

    fn fetch(
        &self,
        request: FetchRequest<'_>,
        cancel: &Cancellation,
    ) -> Result<FetchOutcome, FetchError> {
        cancel.check().map_err(rejected)?;
        let source = Url::parse(request.endpoint.url.expose())
            .map_err(|_| rejected(ClientError::InvalidUrl))?;
        let version = request.known_version.and_then(condition_header);
        let mut request_url = source.clone();
        request_url.set_fragment(None);
        let transport = self.transport().map_err(rejected)?;
        let device_id = if request.send_hwid {
            self.device_id(&request_url).map_err(rejected)?
        } else {
            None
        };
        transport.runtime.block_on(async {
            let deadline = Instant::now() + request.timeout;
            let mut response = transport
                .response(
                    &request_url,
                    request.follow_same_origin_redirects,
                    version,
                    device_id,
                    cancel,
                    deadline,
                )
                .await?;
            timeout_at(deadline, async {
                cancel.check().map_err(rejected)?;
                let cacheable = permits_validation(response.headers());
                if response.status() == StatusCode::NOT_MODIFIED {
                    return Ok(FetchOutcome::Unchanged {
                        retain_version: cacheable,
                    });
                }
                let version = (cacheable
                    && response.status() == StatusCode::OK
                    && response.url() == &request_url)
                    .then(|| response_version(response.headers()))
                    .flatten();
                let bytes = read_body(&mut response, cancel).await?;
                let document = provider::parse_document(&source, response.headers(), bytes)
                    .map_err(rejected)?;
                cancel.check().map_err(rejected)?;
                Ok(FetchOutcome::Modified { document, version })
            })
            .await
            .map_err(|_| FetchError::Timeout)?
        })
    }
}

fn permits_device_id(url: &Url) -> bool {
    match url.scheme() {
        "https" => true,
        "http" => match url.host() {
            Some(Host::Domain("localhost")) => true,
            Some(Host::Ipv4(ip)) => ip.is_loopback(),
            Some(Host::Ipv6(ip)) => ip.is_loopback(),
            _ => false,
        },
        _ => false,
    }
}

fn condition_header(version: &Secret) -> Option<HeaderValue> {
    let value = version.expose();
    if !provider::valid_etag(value) {
        return None;
    }
    let mut header = HeaderValue::from_str(value).ok()?;
    header.set_sensitive(true);
    Some(header)
}

fn response_version(headers: &HeaderMap) -> Option<Secret> {
    let mut tags = headers.get_all(ETAG).iter();
    let first = tags.next()?.to_str().ok()?;
    (tags.next().is_none() && provider::valid_etag(first)).then(|| Secret::new(first))
}

fn permits_validation(headers: &HeaderMap) -> bool {
    !has_directive(headers, CACHE_CONTROL, "no-store") && !has_directive(headers, VARY, "*")
}

fn has_directive(headers: &HeaderMap, name: HeaderName, expected: &str) -> bool {
    headers.get_all(name).iter().any(|value| {
        value.to_str().map_or(true, |text| {
            text.split(',')
                .any(|directive| directive.trim().eq_ignore_ascii_case(expected))
        })
    })
}

async fn response_error(
    mut response: Response,
    original_url: &Url,
    condition: Option<&HeaderValue>,
    device_id: Option<&HeaderValue>,
    cancel: &Cancellation,
    deadline: Instant,
) -> Result<SubscriptionHttpError, FetchError> {
    let status = response.status();
    let read_deadline = deadline.min(Instant::now() + ERROR_BODY_READ_BUDGET);
    let bytes = tokio::select! {
        biased;
        () = cancel.cancelled() => return Err(rejected(ClientError::Cancelled)),
        bytes = timeout_at(read_deadline, read_error_body(&mut response)) => bytes.ok().flatten(),
    };
    let detail = bytes
        .and_then(|bytes| error_detail(response.headers(), &bytes))
        .filter(|detail| {
            !url_secrets_appear(original_url, detail)
                && !url_secrets_appear(response.url(), detail)
                && !condition_secret_appears(condition, detail)
                && !device_id
                    .and_then(|value| value.to_str().ok())
                    .is_some_and(|value| contains_secret(detail, value))
        });
    cancel.check().map_err(rejected)?;
    Ok(SubscriptionHttpError::new(
        status.as_u16(),
        detail
            .as_deref()
            .or_else(|| status.canonical_reason())
            .unwrap_or_default(),
    ))
}

async fn read_error_body(response: &mut Response) -> Option<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length == 0 || length > MAX_ERROR_BODY_BYTES as u64)
    {
        return None;
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if bytes.len().saturating_add(chunk.len()) > MAX_ERROR_BODY_BYTES {
            return None;
        }
        bytes.extend_from_slice(&chunk);
    }
    Some(bytes)
}

fn error_detail(headers: &HeaderMap, bytes: &[u8]) -> Option<String> {
    let content_type = match headers.get(CONTENT_TYPE) {
        Some(value) => value
            .to_str()
            .ok()?
            .split(';')
            .next()?
            .trim()
            .to_ascii_lowercase(),
        None => String::new(),
    };
    let json = content_type == "application/json" || content_type.ends_with("+json");
    if !json && !content_type.is_empty() && content_type != "text/plain" {
        return None;
    }
    let body = std::str::from_utf8(bytes).ok()?.trim();
    match serde_json::from_str::<Value>(body) {
        Ok(value) => json_error_detail(&value, 0),
        Err(_) if !json && !body.starts_with(['{', '[', '"']) => response_text(body),
        Err(_) => None,
    }
}

fn json_error_detail(value: &Value, depth: usize) -> Option<String> {
    if depth == 4 {
        return None;
    }
    match value {
        Value::String(text) => response_text(text),
        Value::Bool(_) | Value::Number(_) => response_text(&value.to_string()),
        Value::Object(fields) => ["detail", "message", "error", "reason"]
            .into_iter()
            .filter_map(|field| fields.get(field))
            .find_map(|value| json_error_detail(value, depth + 1)),
        Value::Array(values) => values
            .iter()
            .find_map(|value| json_error_detail(value, depth + 1)),
        Value::Null => None,
    }
}

fn response_text(text: &str) -> Option<String> {
    let text = text.trim();
    safe_error_detail(text).then(|| text.to_owned())
}

fn safe_error_detail(detail: &str) -> bool {
    if detail.is_empty()
        || detail.len() > MAX_ERROR_DETAIL_BYTES
        || detail.contains(['<', '>', '{', '}', '@', '=', '\\'])
        || detail.contains("//")
        || detail.chars().any(|ch| {
            ch.is_control() || matches!(ch, '\u{200e}' | '\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
    {
        return false;
    }
    let lowercase = detail.to_ascii_lowercase();
    ![
        "token:",
        "password:",
        "authorization:",
        "api-key:",
        "api_key:",
        "secret:",
        "cookie:",
    ]
    .iter()
    .any(|label| lowercase.contains(label))
        && !detail.split_whitespace().any(sensitive_word)
}

fn sensitive_word(word: &str) -> bool {
    let word = word.trim_matches(['"', '\'', '(', ')', '[', ']', ',', ';', '.', '!', '?']);
    word.eq_ignore_ascii_case("bearer")
        || word.starts_with('/')
        || word.rsplit_once('.').is_some_and(|(name, suffix)| {
            !name.is_empty()
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b".-".contains(&byte))
                && suffix.len() >= 2
                && suffix.bytes().all(|byte| byte.is_ascii_alphabetic())
        })
        || (word.len() >= 24
            && word
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
            && word.bytes().any(|byte| byte.is_ascii_digit()))
}

fn url_secrets_appear(url: &Url, detail: &str) -> bool {
    url.host_str()
        .is_some_and(|host| contains_secret(detail, host))
        || url.path_segments().into_iter().flatten().any(|segment| {
            contains_secret(detail, segment)
                || percent_decode_str(segment)
                    .decode_utf8()
                    .is_ok_and(|decoded| contains_secret(detail, &decoded))
        })
        || url.query_pairs().any(|(key, value)| {
            contains_secret(detail, if value.is_empty() { &key } else { &value })
        })
        || url.query().is_some_and(|query| {
            query.split('&').any(|pair| {
                let value = pair.split_once('=').map_or(pair, |(_, value)| value);
                contains_secret(detail, value)
            })
        })
}

fn condition_secret_appears(condition: Option<&HeaderValue>, detail: &str) -> bool {
    let Some(value) = condition.and_then(|value| value.to_str().ok()) else {
        return false;
    };
    let tag = value.strip_prefix("W/").unwrap_or(value).trim_matches('"');
    contains_secret(detail, tag)
}

fn contains_secret(detail: &str, secret: &str) -> bool {
    !secret.is_empty()
        && detail.match_indices(secret).any(|(start, _)| {
            let end = start + secret.len();
            let boundary = |ch: char| !ch.is_alphanumeric();
            detail[..start].chars().next_back().is_none_or(boundary)
                && detail[end..].chars().next().is_none_or(boundary)
        })
}

async fn read_body(response: &mut Response, cancel: &Cancellation) -> Result<Vec<u8>, FetchError> {
    let mut bytes = Vec::new();
    loop {
        let chunk = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(rejected(ClientError::Cancelled)),
            chunk = response.chunk() => chunk.map_err(|error| http_error(&error, response.url().scheme()))?,
        };
        let Some(chunk) = chunk else {
            return Ok(bytes);
        };
        if bytes.len().saturating_add(chunk.len()) > MAX_SUBSCRIPTION_BYTES {
            return Err(rejected(ClientError::PayloadTooLarge));
        }
        bytes.extend_from_slice(&chunk);
    }
}

fn same_origin_redirect(previous: &Url, response: &Response) -> Option<Url> {
    let location = response.headers().get(LOCATION)?.to_str().ok()?;
    let next = previous.join(location).ok()?;
    if next.origin() == previous.origin()
        && next.username().is_empty()
        && next.password().is_none()
        && next.as_str().len() <= MAX_METADATA_VALUE_BYTES
    {
        return Some(next);
    }
    None
}

fn rejected(error: ClientError) -> FetchError {
    FetchError::Rejected(error)
}

fn http_error(error: &reqwest::Error, scheme: &str) -> FetchError {
    if error.is_timeout() {
        return FetchError::Timeout;
    }
    if error.is_builder() {
        return rejected(ClientError::InvalidUrl);
    }
    if error.is_connect() && scheme == "https" && !has_socket_error(error) {
        return rejected(ClientError::Network);
    }
    FetchError::Transport
}

fn has_socket_error(error: &reqwest::Error) -> bool {
    let mut source = error.source();
    while let Some(cause) = source {
        if let Some(error) = cause.downcast_ref::<io::Error>()
            && matches!(
                error.kind(),
                io::ErrorKind::ConnectionRefused
                    | io::ErrorKind::ConnectionAborted
                    | io::ErrorKind::ConnectionReset
                    | io::ErrorKind::NetworkUnreachable
                    | io::ErrorKind::HostUnreachable
                    | io::ErrorKind::AddrNotAvailable
            )
        {
            return true;
        }
        source = cause.source();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_id_requires_tls_or_local_loopback() {
        for url in [
            "https://provider.test/subscription",
            "http://localhost/subscription",
            "http://127.0.0.1/subscription",
            "http://[::1]/subscription",
        ] {
            assert!(permits_device_id(&Url::parse(url).unwrap()));
        }
        for url in [
            "http://provider.test/subscription",
            "http://localhost.provider.test/subscription",
            "http://192.0.2.1/subscription",
            "http://[2001:db8::1]/subscription",
        ] {
            assert!(!permits_device_id(&Url::parse(url).unwrap()));
        }
    }

    #[test]
    fn extracts_short_structured_and_plain_response_details() {
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/problem+json"),
        );
        assert_eq!(
            error_detail(&headers, br#"{"detail":"HWID header required"}"#).as_deref(),
            Some("HWID header required")
        );
        assert_eq!(
            error_detail(&headers, br#"{"error":{"reason":123}}"#).as_deref(),
            Some("123")
        );
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("text/plain; charset=utf-8"),
        );
        assert_eq!(
            error_detail(&headers, b"Condition required\n").as_deref(),
            Some("Condition required")
        );
    }

    #[test]
    fn excludes_html_links_controls_and_credentials() {
        let headers = HeaderMap::new();
        for body in [
            "<html>Forbidden</html>",
            "See https://example.test/sub",
            "Authorization: Bearer sample",
            "Token: sample",
            "Forbidden\nTry later",
            "\u{202e}Forbidden",
            "{\"message\":",
        ] {
            assert_eq!(error_detail(&headers, body.as_bytes()), None);
        }
        let url =
            Url::parse("https://example.test/my%20subscription?access=query%2Dvalue").unwrap();
        for secret in [
            "my%20subscription",
            "my subscription",
            "query-value",
            "query%2Dvalue",
        ] {
            assert!(url_secrets_appear(
                &url,
                &format!("Invalid token {secret}.")
            ));
        }
        let tag = HeaderValue::from_static("W/\"private-version\"");
        assert!(condition_secret_appears(
            Some(&tag),
            "Invalid tag private-version"
        ));
    }
}
