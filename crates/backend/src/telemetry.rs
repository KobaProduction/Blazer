use std::collections::VecDeque;

use opentelemetry::KeyValue;
use opentelemetry::trace::{Span, Tracer, TracerProvider};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::{Sampler, SdkTracer, SdkTracerProvider};

pub const DIAGNOSTIC_CAPACITY: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusCode {
    Disconnected,
    Connected,
    Unavailable,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportCode {
    Imported,
    TooLarge,
    NormalizerUnavailable,
    UnsupportedFormat,
    UnsupportedFeature,
    EngineUnavailable,
}

/// Fixed-size metadata only: no URLs, profile names, payloads, IPs or secrets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticEvent {
    Started,
    Status(StatusCode),
    Import(ImportCode),
    ExportNotConfigured,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiagnosticSummary {
    pub buffered: usize,
    pub dropped: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportError {
    NotConfigured,
}

struct DiagnosticRing {
    events: VecDeque<DiagnosticEvent>,
    dropped: u64,
}

impl DiagnosticRing {
    fn new() -> Self {
        Self {
            events: VecDeque::with_capacity(DIAGNOSTIC_CAPACITY),
            dropped: 0,
        }
    }

    fn push(&mut self, event: DiagnosticEvent) {
        if self.events.len() == DIAGNOSTIC_CAPACITY {
            self.events.pop_front();
            self.dropped = self.dropped.saturating_add(1);
        }
        self.events.push_back(event);
    }

    fn summary(&self) -> DiagnosticSummary {
        DiagnosticSummary {
            buffered: self.events.len(),
            dropped: self.dropped,
        }
    }
}

pub(crate) struct Telemetry {
    _provider: SdkTracerProvider,
    tracer: SdkTracer,
    ring: DiagnosticRing,
}

impl Telemetry {
    pub(crate) fn new() -> Self {
        // No resource detectors, environment configuration, exporters or processors.
        let resource = Resource::builder_empty()
            .with_attributes([
                KeyValue::new("service.name", "vpn-client"),
                KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
            ])
            .build();
        let provider = SdkTracerProvider::builder()
            .with_sampler(Sampler::AlwaysOff)
            .with_resource(resource)
            .build();
        let tracer = provider.tracer("vpn-client-backend");
        Self {
            _provider: provider,
            tracer,
            ring: DiagnosticRing::new(),
        }
    }

    pub(crate) fn record(&mut self, event: DiagnosticEvent) {
        let (name, result) = event_attributes(event);
        let mut span = self.tracer.start(name);
        span.set_attribute(KeyValue::new("result", result));
        span.end();
        self.ring.push(event);
    }

    pub(crate) fn summary(&self) -> DiagnosticSummary {
        self.ring.summary()
    }
}

fn event_attributes(event: DiagnosticEvent) -> (&'static str, &'static str) {
    match event {
        DiagnosticEvent::Started => ("client.lifecycle", "started"),
        DiagnosticEvent::Status(code) => ("client.status", status_code(code)),
        DiagnosticEvent::Import(code) => ("client.import", import_code(code)),
        DiagnosticEvent::ExportNotConfigured => ("client.telemetry", "not_configured"),
    }
}

fn status_code(code: StatusCode) -> &'static str {
    match code {
        StatusCode::Disconnected => "disconnected",
        StatusCode::Connected => "connected",
        StatusCode::Unavailable => "unavailable",
        StatusCode::Unsupported => "unsupported",
    }
}

fn import_code(code: ImportCode) -> &'static str {
    match code {
        ImportCode::Imported => "imported",
        ImportCode::TooLarge => "too_large",
        ImportCode::NormalizerUnavailable => "normalizer_unavailable",
        ImportCode::UnsupportedFormat => "unsupported_format",
        ImportCode::UnsupportedFeature => "unsupported_feature",
        ImportCode::EngineUnavailable => "engine_unavailable",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_evicts_oldest_and_counts_dropped_events() {
        let mut ring = DiagnosticRing::new();
        ring.push(DiagnosticEvent::Started);
        for _ in 0..DIAGNOSTIC_CAPACITY {
            ring.push(DiagnosticEvent::ExportNotConfigured);
        }
        assert_eq!(ring.summary().buffered, DIAGNOSTIC_CAPACITY);
        assert_eq!(ring.summary().dropped, 1);
        assert_eq!(
            ring.events.front(),
            Some(&DiagnosticEvent::ExportNotConfigured)
        );
    }

    #[test]
    fn dropped_counter_saturates() {
        let mut ring = DiagnosticRing::new();
        for _ in 0..DIAGNOSTIC_CAPACITY {
            ring.push(DiagnosticEvent::Started);
        }
        ring.dropped = u64::MAX;
        ring.push(DiagnosticEvent::Started);
        assert_eq!(ring.summary().dropped, u64::MAX);
    }
}
