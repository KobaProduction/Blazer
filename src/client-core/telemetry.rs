//! Bounded local diagnostics and OpenTelemetry for the shared client worker.

mod export;

use std::collections::VecDeque;
use std::time::{Duration, SystemTime};

use opentelemetry::logs::{LogRecord, Logger, LoggerProvider, Severity};
use opentelemetry::metrics::{Counter, Histogram, MeterProvider};
use opentelemetry::trace::{Span, Status, Tracer, TracerProvider};
use opentelemetry::{Context, KeyValue};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::logs::SdkLogger;
use opentelemetry_sdk::trace::SdkTracer;
use vpn_application::client::{Cancellation, ClientError, TelemetrySettings};
use vpn_application::session::{ClientEvent, ClientEventKind, DiagnosticSummary};

use self::export::ExportProviders;

const CAPACITY: usize = 128;
const SCOPE: &str = env!("CARGO_PKG_NAME");

pub(super) struct Telemetry {
    instruments: Option<Instruments>,
    events: VecDeque<ClientEvent>,
    next_sequence: u64,
    dropped: u64,
}

impl Telemetry {
    pub(super) fn new(settings: &TelemetrySettings) -> Self {
        Self::with_instruments(Instruments::configure(settings))
    }

    fn with_instruments(instruments: Option<Instruments>) -> Self {
        Self {
            instruments,
            events: VecDeque::with_capacity(CAPACITY),
            next_sequence: 1,
            dropped: 0,
        }
    }

    pub(super) fn record(&mut self, kind: ClientEventKind) {
        self.record_with_duration(kind, None);
    }

    pub(super) fn record_with_duration(
        &mut self,
        kind: ClientEventKind,
        duration: Option<Duration>,
    ) {
        let evicted = self.events.len() == CAPACITY;
        if evicted {
            self.events.pop_front();
            self.dropped = self.dropped.saturating_add(1);
        }
        self.events.push_back(ClientEvent {
            sequence: self.next_sequence,
            kind,
        });
        self.next_sequence = self.next_sequence.saturating_add(1);
        if let Some(instruments) = &self.instruments {
            instruments.record(kind, duration, evicted);
        }
    }

    pub(super) fn export_configured(&self) -> bool {
        self.instruments.is_some()
    }

    pub(super) fn verify_manual_settings(
        settings: &TelemetrySettings,
        cancel: &Cancellation,
    ) -> Result<(), ClientError> {
        export::verify_manual_settings(settings, cancel)
    }

    pub(super) fn reconfigure(&mut self, settings: &TelemetrySettings) {
        if let Some(instruments) = &self.instruments {
            instruments.providers.revoke();
        }
        self.instruments = Instruments::configure(settings);
    }

    pub(super) fn summary(&self) -> DiagnosticSummary {
        DiagnosticSummary {
            buffered: self.events.len(),
            dropped: self.dropped,
        }
    }

    pub(super) fn events(&self) -> Vec<ClientEvent> {
        self.events.iter().copied().collect()
    }
}

struct Instruments {
    tracer: SdkTracer,
    logger: SdkLogger,
    events: Counter<u64>,
    duration: Histogram<f64>,
    dropped: Counter<u64>,
    providers: ExportProviders,
}

impl Instruments {
    fn configure(settings: &TelemetrySettings) -> Option<Self> {
        let resource = Resource::builder_empty()
            .with_attributes([
                KeyValue::new("service.name", "blazer"),
                KeyValue::new("service.version", vpn_application::APP_VERSION),
            ])
            .build();
        export::configure(&resource, settings).map(Self::new)
    }

    fn new(providers: ExportProviders) -> Self {
        let meter = providers.meter.meter(SCOPE);
        Self {
            tracer: providers.tracer.tracer(SCOPE),
            logger: providers.logger.logger(SCOPE),
            events: meter
                .u64_counter("blazer.client.events")
                .with_description("Network client lifecycle and operation events")
                .with_unit("{event}")
                .build(),
            duration: meter
                .f64_histogram("blazer.client.operation.duration")
                .with_description("Time to complete a client operation")
                .with_unit("s")
                .build(),
            dropped: meter
                .u64_counter("blazer.client.diagnostic.events.dropped")
                .with_description("Events evicted from the local diagnostics ring")
                .with_unit("{event}")
                .build(),
            providers,
        }
    }

    fn record(&self, kind: ClientEventKind, duration: Option<Duration>, evicted: bool) {
        // Use only static event labels for network telemetry.
        let (name, result) = attributes(kind);
        let dimensions = [
            KeyValue::new("operation", name),
            KeyValue::new("result", result),
        ];
        self.events.add(1, &dimensions);
        if let Some(duration) = duration {
            self.duration.record(duration.as_secs_f64(), &dimensions);
        }
        if evicted {
            self.dropped.add(1, &[]);
        }

        let now = SystemTime::now();
        let start = duration
            .and_then(|elapsed| now.checked_sub(elapsed))
            .unwrap_or(now);
        let mut span = self.tracer.build_with_context(
            self.tracer.span_builder(name).with_start_time(start),
            &Context::new(),
        );
        span.set_attribute(KeyValue::new("result", result));
        let failed = matches!(result, "failed" | "settings_failed");
        if failed {
            span.set_status(Status::error("operation failed"));
        }
        let context = span.span_context();
        let mut log = self.logger.create_log_record();
        log.set_event_name(name);
        log.set_timestamp(now);
        log.set_severity_number(if failed {
            Severity::Error
        } else {
            Severity::Info
        });
        log.set_body(result.into());
        log.add_attribute("result", result);
        if context.is_valid() {
            log.set_trace_context(
                context.trace_id(),
                context.span_id(),
                Some(context.trace_flags()),
            );
        }
        self.logger.emit(log);
        span.end_with_timestamp(now);
    }
}

fn attributes(event: ClientEventKind) -> (&'static str, &'static str) {
    match event {
        ClientEventKind::Started => ("client.lifecycle", "started"),
        ClientEventKind::SubscriptionUpdated => ("client.subscription", "updated"),
        ClientEventKind::SubscriptionFallbackUsed => ("client.subscription", "fallback_used"),
        ClientEventKind::SubscriptionFailed(_) => ("client.subscription", "failed"),
        ClientEventKind::SubscriptionSettingsSaved => ("client.subscription", "settings_saved"),
        ClientEventKind::SubscriptionSettingsFailed(_) => {
            ("client.subscription", "settings_failed")
        }
        ClientEventKind::SubscriptionRemoved => ("client.subscription", "removed"),
        ClientEventKind::ProfilesImported => ("client.profile", "imported"),
        ClientEventKind::ProfileSelected => ("client.profile", "selected"),
        ClientEventKind::ProfileRemoved => ("client.profile", "removed"),
        ClientEventKind::ConnectionStarting => ("client.connection", "starting"),
        ClientEventKind::Connected(_) => ("client.connection", "connected"),
        ClientEventKind::Disconnected => ("client.connection", "disconnected"),
        ClientEventKind::SettingsSaved => ("client.settings", "saved"),
        ClientEventKind::Failed(_) => ("client.operation", "failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_evicts_oldest_and_counts_dropped_events() {
        let mut telemetry = Telemetry::with_instruments(None);
        telemetry.record(ClientEventKind::Started);
        for _ in 0..CAPACITY {
            telemetry.record(ClientEventKind::SettingsSaved);
        }
        assert_eq!(telemetry.summary().buffered, CAPACITY);
        assert_eq!(telemetry.summary().dropped, 1);
        assert_eq!(
            telemetry.events().first(),
            Some(&ClientEvent {
                sequence: 2,
                kind: ClientEventKind::SettingsSaved
            }),
        );
    }

    #[test]
    fn dropped_counter_saturates() {
        let mut telemetry = Telemetry::with_instruments(None);
        for _ in 0..CAPACITY {
            telemetry.record(ClientEventKind::Started);
        }
        telemetry.dropped = u64::MAX;
        telemetry.record(ClientEventKind::Started);
        assert_eq!(telemetry.summary().dropped, u64::MAX);
    }
}
