use std::env;
use std::ffi::OsString;
use std::fmt;
use std::io::{self, Read};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use opentelemetry_http::{Bytes, HttpClient, HttpError, Request, Response};
use opentelemetry_otlp::{
    LogExporter, MetricExporter, Protocol, RetryPolicy, SpanExporter, WithExportConfig,
    WithHttpConfig,
};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::logs::{
    BatchConfigBuilder as LogBatchConfig, BatchLogProcessor, SdkLoggerProvider,
};
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};
use opentelemetry_sdk::trace::{
    BatchConfigBuilder as SpanBatchConfig, BatchSpanProcessor, Sampler, SdkTracerProvider,
};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use reqwest::{StatusCode, Url, blocking::Client, redirect::Policy};
use vpn_application::client::{Cancellation, ClientError, TelemetrySettings};
use vpn_application::session::TelemetryMode;

const REQUEST_TIMEOUT: Duration = Duration::from_millis(1500);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const BATCH_DELAY: Duration = Duration::from_secs(5);
const METRIC_INTERVAL: Duration = Duration::from_secs(30);
const MAX_QUEUE: usize = 256;
const MAX_BATCH: usize = 128;
const MAX_REQUEST_BYTES: usize = 256 * 1024;
const SIGNALS: [&str; 3] = ["TRACES", "METRICS", "LOGS"];
const PROTOBUF_CONTENT_TYPE: &str = "application/x-protobuf";
const BUILD_ENDPOINT: Option<&str> = option_env!("OTEL_ENDPOINT");

pub(super) struct ExportProviders {
    pub(super) tracer: SdkTracerProvider,
    pub(super) meter: SdkMeterProvider,
    pub(super) logger: SdkLoggerProvider,
    revoked: Arc<AtomicBool>,
}

impl ExportProviders {
    pub(super) fn revoke(&self) {
        self.revoked.store(true, Ordering::Release);
    }
}

impl Drop for ExportProviders {
    fn drop(&mut self) {
        let tracer = &self.tracer;
        let logger = &self.logger;
        std::thread::scope(|scope| {
            let traces = std::thread::Builder::new()
                .spawn_scoped(scope, || tracer.shutdown_with_timeout(SHUTDOWN_TIMEOUT));
            let logs = std::thread::Builder::new()
                .spawn_scoped(scope, || logger.shutdown_with_timeout(SHUTDOWN_TIMEOUT));

            // The SDK's synchronous periodic reader limits shutdown to five seconds.
            // Run all three shutdowns concurrently so an unreachable collector cannot
            // extend the total wait by stacking these timeouts.
            let _ = self.meter.shutdown();
            if let Ok(worker) = traces {
                let _ = worker.join();
            } else {
                let _ = tracer.shutdown_with_timeout(Duration::ZERO);
            }
            if let Ok(worker) = logs {
                let _ = worker.join();
            } else {
                let _ = logger.shutdown_with_timeout(Duration::ZERO);
            }
        });
    }
}

pub(super) fn configure(
    resource: &Resource,
    settings: &TelemetrySettings,
) -> Option<ExportProviders> {
    if sdk_disabled() {
        return None;
    }
    let endpoints = configured_endpoints(settings)?;
    if endpoints.iter().all(Option::is_none) {
        return None;
    }
    let client = http_client()?;
    let revoked = Arc::new(AtomicBool::new(false));
    let transport = |headers| ExportTransport {
        client: client.clone(),
        headers,
        revoked: revoked.clone(),
    };
    let [traces, metrics, logs] = endpoints;
    let traces = traces.and_then(|endpoint| {
        SpanExporter::builder()
            .with_http()
            .with_protocol(Protocol::HttpBinary)
            .with_endpoint(endpoint.url)
            .with_timeout(REQUEST_TIMEOUT)
            .with_http_client(transport(endpoint.headers))
            .with_retry_policy(RetryPolicy::disabled())
            .with_max_request_body_size(MAX_REQUEST_BYTES)
            .build()
            .ok()
    });
    let metrics = metrics.and_then(|endpoint| {
        MetricExporter::builder()
            .with_http()
            .with_protocol(Protocol::HttpBinary)
            .with_endpoint(endpoint.url)
            .with_timeout(REQUEST_TIMEOUT)
            .with_http_client(transport(endpoint.headers))
            .with_retry_policy(RetryPolicy::disabled())
            .with_max_request_body_size(MAX_REQUEST_BYTES)
            .build()
            .ok()
    });
    let logs = logs.and_then(|endpoint| {
        LogExporter::builder()
            .with_http()
            .with_protocol(Protocol::HttpBinary)
            .with_endpoint(endpoint.url)
            .with_timeout(REQUEST_TIMEOUT)
            .with_http_client(transport(endpoint.headers))
            .with_retry_policy(RetryPolicy::disabled())
            .with_max_request_body_size(MAX_REQUEST_BYTES)
            .build()
            .ok()
    });
    if traces.is_none() && metrics.is_none() && logs.is_none() {
        return None;
    }
    Some(ExportProviders {
        tracer: trace_provider(resource, traces),
        meter: meter_provider(resource, metrics),
        logger: logger_provider(resource, logs),
        revoked,
    })
}

struct Endpoint {
    url: String,
    headers: HeaderPolicy,
}

enum HeaderPolicy {
    Inherited,
    Isolated(Option<HeaderValue>),
}

impl HeaderPolicy {
    fn apply(&self, headers: &mut HeaderMap) {
        if let Self::Isolated(authorization) = self {
            // The OTLP SDK adds environment headers even with an explicit endpoint.
            headers.clear();
            headers.insert(
                CONTENT_TYPE,
                HeaderValue::from_static(PROTOBUF_CONTENT_TYPE),
            );
            if let Some(value) = authorization {
                headers.insert(AUTHORIZATION, value.clone());
            }
        }
    }
}

struct ExportTransport {
    client: Client,
    headers: HeaderPolicy,
    revoked: Arc<AtomicBool>,
}

impl fmt::Debug for ExportTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExportTransport")
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl HttpClient for ExportTransport {
    async fn send_bytes(&self, mut request: Request<Bytes>) -> Result<Response<Bytes>, HttpError> {
        self.headers.apply(request.headers_mut());
        if self.revoked.load(Ordering::Acquire) {
            return Err(
                io::Error::new(io::ErrorKind::Interrupted, "telemetry export disabled").into(),
            );
        }
        self.client.send_bytes(request).await
    }
}

fn configured_endpoints(settings: &TelemetrySettings) -> Option<[Option<Endpoint>; 3]> {
    match settings.mode {
        TelemetryMode::Disabled => None,
        TelemetryMode::Default => Some(SIGNALS.map(default_endpoint)),
        TelemetryMode::Manual => {
            let (endpoint, authorization) = manual_credentials(settings)?;
            Some(SIGNALS.map(|signal| isolated_endpoint(endpoint, signal, authorization.clone())))
        }
    }
}

fn manual_credentials(settings: &TelemetrySettings) -> Option<(&str, Option<HeaderValue>)> {
    settings.validate().ok()?;
    let endpoint = settings.endpoint.as_deref()?;
    let authorization = match &settings.bearer_token {
        Some(token) => Some(bearer_header(token.expose())?),
        None => None,
    };
    Some((endpoint, authorization))
}

fn sdk_disabled() -> bool {
    env::var("OTEL_SDK_DISABLED").is_ok_and(|value| value.trim().eq_ignore_ascii_case("true"))
}

pub(super) fn verify_manual_settings(
    settings: &TelemetrySettings,
    cancel: &Cancellation,
) -> Result<(), ClientError> {
    cancel.check()?;
    let unavailable = ClientError::TelemetryEndpointUnavailable;
    if sdk_disabled() {
        return Err(unavailable);
    }
    let (base, authorization) = manual_credentials(settings).ok_or(unavailable)?;
    let endpoint = isolated_endpoint(base, "LOGS", authorization).ok_or(unavailable)?;
    let client = http_client().ok_or(unavailable)?;
    cancel.check()?;
    let acknowledged = check_collector_ack(&client, endpoint);
    cancel.check()?;
    if acknowledged {
        Ok(())
    } else {
        Err(unavailable)
    }
}

fn check_collector_ack(client: &Client, endpoint: Endpoint) -> bool {
    // A default ExportLogsServiceRequest serializes to zero bytes and contains no records.
    let mut headers = HeaderMap::new();
    endpoint.headers.apply(&mut headers);
    let response = client.post(endpoint.url).headers(headers).body("").send();
    let Ok(mut response) = response else {
        return false;
    };
    response.status() == StatusCode::OK
        && response.headers().get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                let media_type = value.split(';').next().unwrap_or_default().trim();
                media_type.eq_ignore_ascii_case(PROTOBUF_CONTENT_TYPE)
            })
        // A full success response, with partial_success unset, is the empty Protobuf message.
        && response.read(&mut [0; 1]).is_ok_and(|bytes| bytes == 0)
}

fn default_endpoint(signal: &str) -> Option<Endpoint> {
    if env::var(format!("OTEL_{signal}_EXPORTER"))
        .is_ok_and(|exporter| exporter.trim().eq_ignore_ascii_case("none"))
    {
        return None;
    }
    if let Some(endpoint) = env_option("OTEL_ENDPOINT") {
        return isolated_endpoint(endpoint.to_str()?, signal, operator_authorization()?);
    }
    if let Some(endpoint) = env_option(&format!("OTEL_EXPORTER_OTLP_{signal}_ENDPOINT")) {
        return inherited_endpoint(endpoint.to_str()?, signal, None);
    }
    if let Some(base) = env_option("OTEL_EXPORTER_OTLP_ENDPOINT") {
        return inherited_endpoint(base.to_str()?, signal, Some(&signal.to_ascii_lowercase()));
    }
    isolated_endpoint(BUILD_ENDPOINT?, signal, operator_authorization()?)
}

fn inherited_endpoint(endpoint: &str, signal: &str, path: Option<&str>) -> Option<Endpoint> {
    if let Some(protocol) = signal_option(signal, "PROTOCOL")
        && protocol != "http/protobuf"
    {
        return None;
    }
    Some(Endpoint {
        url: endpoint_url(endpoint, path)?,
        headers: HeaderPolicy::Inherited,
    })
}

fn isolated_endpoint(
    endpoint: &str,
    signal: &str,
    authorization: Option<HeaderValue>,
) -> Option<Endpoint> {
    Some(Endpoint {
        url: endpoint_url(endpoint, Some(&signal.to_ascii_lowercase()))?,
        headers: HeaderPolicy::Isolated(authorization),
    })
}

fn operator_authorization() -> Option<Option<HeaderValue>> {
    let Some(token) = env_option("OTEL_BEARER_TOKEN") else {
        return Some(None);
    };
    let token = token.to_str()?;
    if token.trim().is_empty() {
        return Some(None);
    }
    bearer_header(token).map(Some)
}

fn bearer_header(token: &str) -> Option<HeaderValue> {
    TelemetrySettings::validate_bearer_token(token).ok()?;
    let mut header = HeaderValue::from_str(&format!("Bearer {token}")).ok()?;
    header.set_sensitive(true);
    Some(header)
}

fn env_option(name: &str) -> Option<OsString> {
    env::var_os(name).filter(|value| !value.is_empty())
}

fn signal_option(signal: &str, option: &str) -> Option<OsString> {
    env_option(&format!("OTEL_EXPORTER_OTLP_{signal}_{option}"))
        .or_else(|| env_option(&format!("OTEL_EXPORTER_OTLP_{option}")))
}

fn endpoint_url(endpoint: &str, signal: Option<&str>) -> Option<String> {
    TelemetrySettings::validate_endpoint(endpoint).ok()?;
    let mut url = Url::parse(endpoint).ok()?;
    if let Some(signal) = signal {
        url.path_segments_mut()
            .ok()?
            .pop_if_empty()
            .extend(["v1", signal]);
    }
    Some(url.into())
}

fn http_client() -> Option<Client> {
    // Constructed on the backend's OS thread. The SDK invokes this blocking
    // transport on its own batch and periodic reader threads.
    let builder = Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .redirect(Policy::none())
        .no_proxy()
        .retry(reqwest::retry::never());
    #[cfg(not(target_os = "android"))]
    let builder = builder.tls_backend_native();
    #[cfg(target_os = "android")]
    let builder = {
        let roots =
            rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let config = rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .ok()?
        .with_root_certificates(roots)
        .with_no_client_auth();
        builder.tls_backend_preconfigured(config)
    };
    builder.build().ok()
}

fn trace_provider(resource: &Resource, exporter: Option<SpanExporter>) -> SdkTracerProvider {
    let builder = SdkTracerProvider::builder().with_resource(resource.clone());
    let Some(exporter) = exporter else {
        return builder.with_sampler(Sampler::AlwaysOff).build();
    };
    let batch = SpanBatchConfig::default()
        .with_max_queue_size(MAX_QUEUE)
        .with_max_export_batch_size(MAX_BATCH)
        .with_scheduled_delay(BATCH_DELAY)
        .build();
    builder
        .with_sampler(Sampler::AlwaysOn)
        .with_span_processor(
            BatchSpanProcessor::builder(exporter)
                .with_batch_config(batch)
                .build(),
        )
        .build()
}

fn meter_provider(resource: &Resource, exporter: Option<MetricExporter>) -> SdkMeterProvider {
    let builder = SdkMeterProvider::builder().with_resource(resource.clone());
    match exporter {
        Some(exporter) => builder.with_reader(
            PeriodicReader::builder(exporter)
                .with_interval(METRIC_INTERVAL)
                .build(),
        ),
        None => builder,
    }
    .build()
}

fn logger_provider(resource: &Resource, exporter: Option<LogExporter>) -> SdkLoggerProvider {
    let builder = SdkLoggerProvider::builder().with_resource(resource.clone());
    let Some(exporter) = exporter else {
        return builder.build();
    };
    let batch = LogBatchConfig::default()
        .with_max_queue_size(MAX_QUEUE)
        .with_max_export_batch_size(MAX_BATCH)
        .with_scheduled_delay(BATCH_DELAY)
        .build();
    builder
        .with_log_processor(
            BatchLogProcessor::builder(exporter)
                .with_batch_config(batch)
                .build(),
        )
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isolated_transport_replaces_inherited_headers() {
        let mut request = Request::new(Bytes::new());
        let headers = request.headers_mut();
        headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer inherited"));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/plain"));
        headers.insert("x-provider-key", HeaderValue::from_static("inherited"));
        HeaderPolicy::Isolated(Some(HeaderValue::from_static("Bearer manual")))
            .apply(request.headers_mut());
        assert_eq!(request.headers().len(), 2);
        assert_eq!(request.headers()[AUTHORIZATION], "Bearer manual");
        assert_eq!(request.headers()[CONTENT_TYPE], "application/x-protobuf");

        HeaderPolicy::Isolated(None).apply(request.headers_mut());
        assert_eq!(request.headers().len(), 1);
        assert!(!request.headers().contains_key(AUTHORIZATION));
    }

    #[test]
    fn general_endpoint_appends_a_signal_path_without_changing_custom_endpoints() {
        assert_eq!(
            endpoint_url("http://127.0.0.1:4318/gateway/", Some("traces")).as_deref(),
            Some("http://127.0.0.1:4318/gateway/v1/traces")
        );
        assert_eq!(
            endpoint_url("https://collector.invalid/custom-metrics", None).as_deref(),
            Some("https://collector.invalid/custom-metrics")
        );
        assert_eq!(
            endpoint_url("http://[::1]:4318", Some("logs")).as_deref(),
            Some("http://[::1]:4318/v1/logs")
        );
    }

    #[test]
    fn rejects_endpoints_with_extra_url_components_or_non_http_transports() {
        for endpoint in [
            "file:///tmp/collector",
            "127.0.0.1:4318",
            "http://",
            "http:///127.0.0.1:4318",
            "http://@127.0.0.1:4318",
            "http://user:token@127.0.0.1:4318",
            "http://127.0.0.1:4318/?token=fixture",
            "https://collector.invalid/#fragment",
        ] {
            assert!(endpoint_url(endpoint, Some("traces")).is_none());
        }
    }
}
