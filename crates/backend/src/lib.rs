//! Shared outer composition root for native GUI and headless delivery.

mod telemetry;

pub use telemetry::{
    DIAGNOSTIC_CAPACITY, DiagnosticEvent, DiagnosticSummary, ExportError, ImportCode, StatusCode,
};
pub use vpn_application::{
    EngineError, ImportError, MAX_SUBSCRIPTION_BYTES, SubscriptionDocument, SubscriptionFormat,
};
pub use vpn_domain::ConnectionState;

use telemetry::Telemetry;
pub use vpn_application::PreparationError as BackendImportError;
use vpn_application::{EngineCapabilities, EnginePort, NormalizerPort, prepare_configuration};
use vpn_domain::NormalizedConfiguration;
use vpn_infrastructure::{UnavailableEngine, UnavailableNormalizer};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackendSnapshot {
    pub connection: Result<ConnectionState, EngineError>,
    pub connect_enabled: bool,
    pub configuration_profiles: usize,
    pub diagnostics: DiagnosticSummary,
}

/// Mutable application state lives here, not in presentation widgets.
pub struct Backend {
    engine: Box<dyn EnginePort>,
    capabilities: EngineCapabilities,
    normalizer: Box<dyn NormalizerPort>,
    configuration: Option<NormalizedConfiguration>,
    connection: Result<ConnectionState, EngineError>,
    telemetry: Telemetry,
}

impl Default for Backend {
    fn default() -> Self {
        Self::new()
    }
}

impl Backend {
    pub fn new() -> Self {
        let engine = Box::new(UnavailableEngine);
        let capabilities = engine.capabilities();
        let mut backend = Self {
            engine,
            capabilities,
            normalizer: Box::new(UnavailableNormalizer),
            configuration: None,
            connection: Err(EngineError::EngineUnavailable),
            telemetry: Telemetry::new(),
        };
        backend.telemetry.record(DiagnosticEvent::Started);
        backend.refresh_status();
        backend
    }

    /// No adapter calls or telemetry emission occur when reading a cached snapshot.
    pub fn snapshot(&self) -> BackendSnapshot {
        BackendSnapshot {
            connection: self.connection,
            connect_enabled: self.capabilities.can_connect
                && self.configuration.is_some()
                && self.connection.is_ok(),
            configuration_profiles: self
                .configuration
                .as_ref()
                .map_or(0, |configuration| configuration.profiles.len()),
            diagnostics: self.telemetry.summary(),
        }
    }

    /// Invoked on startup or explicit user action, never on every GUI repaint.
    pub fn refresh_status(&mut self) {
        self.capabilities = self.engine.capabilities();
        self.connection = self.engine.connection_state();
        self.telemetry
            .record(DiagnosticEvent::Status(status_code(self.connection)));
    }

    pub fn import(&mut self, document: SubscriptionDocument) -> Result<(), BackendImportError> {
        let result =
            prepare_configuration(&document, self.normalizer.as_ref(), self.engine.as_ref());
        self.telemetry
            .record(DiagnosticEvent::Import(import_code(result.as_ref())));
        match result {
            Ok(configuration) => {
                self.configuration = Some(configuration);
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    pub fn request_export(&mut self) -> Result<(), ExportError> {
        self.telemetry.record(DiagnosticEvent::ExportNotConfigured);
        Err(ExportError::NotConfigured)
    }
}

fn status_code(state: Result<ConnectionState, EngineError>) -> StatusCode {
    match state {
        Ok(ConnectionState::Disconnected) => StatusCode::Disconnected,
        Ok(ConnectionState::Connected) => StatusCode::Connected,
        Err(EngineError::EngineUnavailable) => StatusCode::Unavailable,
        Err(EngineError::UnsupportedFeature) => StatusCode::Unsupported,
    }
}

fn import_code(result: Result<&NormalizedConfiguration, &BackendImportError>) -> ImportCode {
    match result {
        Ok(_) => ImportCode::Imported,
        Err(BackendImportError::Normalization(ImportError::PayloadTooLarge)) => {
            ImportCode::TooLarge
        }
        Err(BackendImportError::Normalization(ImportError::NormalizerUnavailable)) => {
            ImportCode::NormalizerUnavailable
        }
        Err(BackendImportError::Normalization(ImportError::UnsupportedFormat)) => {
            ImportCode::UnsupportedFormat
        }
        Err(BackendImportError::Normalization(ImportError::UnsupportedFeature))
        | Err(BackendImportError::Engine(EngineError::UnsupportedFeature)) => {
            ImportCode::UnsupportedFeature
        }
        Err(BackendImportError::Engine(EngineError::EngineUnavailable)) => {
            ImportCode::EngineUnavailable
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct AcceptingNormalizer;

    impl NormalizerPort for AcceptingNormalizer {
        fn normalize(
            &self,
            _document: &SubscriptionDocument,
        ) -> Result<NormalizedConfiguration, ImportError> {
            Ok(NormalizedConfiguration {
                profiles: vec![vpn_domain::ConnectionProfile {
                    name: "test fixture".to_owned(),
                    protocol: vpn_domain::ProtocolKind::WireGuard,
                }],
            })
        }
    }

    struct RejectingEngine;

    impl EnginePort for RejectingEngine {
        fn capabilities(&self) -> EngineCapabilities {
            EngineCapabilities {
                supported_protocols: &[],
                can_connect: false,
            }
        }

        fn connection_state(&self) -> Result<ConnectionState, EngineError> {
            Ok(ConnectionState::Disconnected)
        }

        fn validate_configuration(
            &self,
            _configuration: &NormalizedConfiguration,
        ) -> Result<(), EngineError> {
            Err(EngineError::UnsupportedFeature)
        }
    }

    #[test]
    fn engine_rejection_does_not_accept_normalized_configuration() {
        let mut backend = Backend::new();
        backend.normalizer = Box::new(AcceptingNormalizer);
        backend.engine = Box::new(RejectingEngine);
        backend.refresh_status();
        let document = SubscriptionDocument::new(SubscriptionFormat::XrayJson, b"fixture".to_vec());
        assert_eq!(
            backend.import(document),
            Err(BackendImportError::Engine(EngineError::UnsupportedFeature))
        );
        assert_eq!(backend.snapshot().configuration_profiles, 0);
        assert!(!backend.snapshot().connect_enabled);
    }

    #[test]
    fn missing_engine_never_enables_connect() {
        let backend = Backend::new();
        let snapshot = backend.snapshot();
        assert_eq!(snapshot.connection, Err(EngineError::EngineUnavailable));
        assert!(!snapshot.connect_enabled);
        assert_eq!(snapshot.configuration_profiles, 0);
    }

    #[test]
    fn snapshot_does_not_generate_diagnostics() {
        let backend = Backend::new();
        let initial = backend.snapshot();
        for _ in 0..100 {
            assert_eq!(backend.snapshot(), initial);
        }
    }

    #[test]
    fn import_failure_preserves_empty_configuration_and_export_stays_disabled() {
        let mut backend = Backend::new();
        let document = SubscriptionDocument::new(SubscriptionFormat::XrayJson, b"{}".to_vec());
        assert_eq!(
            backend.import(document),
            Err(BackendImportError::Normalization(
                ImportError::NormalizerUnavailable
            ))
        );
        assert_eq!(backend.snapshot().configuration_profiles, 0);
        assert_eq!(backend.request_export(), Err(ExportError::NotConfigured));
        assert!(!backend.snapshot().connect_enabled);
    }

    #[test]
    fn repeated_user_refreshes_keep_diagnostics_bounded() {
        let mut backend = Backend::new();
        for _ in 0..(DIAGNOSTIC_CAPACITY + 10) {
            backend.refresh_status();
        }
        assert_eq!(backend.snapshot().diagnostics.buffered, DIAGNOSTIC_CAPACITY);
        assert_eq!(backend.snapshot().diagnostics.dropped, 12);
    }
}
