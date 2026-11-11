//! Use cases and ports independent of GUI, protocol implementations and OS APIs.

use vpn_domain::{ConnectionState, NormalizedConfiguration, ProtocolKind};

pub const MAX_SUBSCRIPTION_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscriptionFormat {
    XrayJson,
    ShareLinkList,
}

/// Input stays outside the domain model and must not be logged.
pub struct SubscriptionDocument {
    format: SubscriptionFormat,
    bytes: Vec<u8>,
}

impl SubscriptionDocument {
    pub fn new(format: SubscriptionFormat, bytes: Vec<u8>) -> Self {
        Self { format, bytes }
    }

    pub fn format(&self) -> SubscriptionFormat {
        self.format
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportError {
    PayloadTooLarge,
    NormalizerUnavailable,
    UnsupportedFormat,
    UnsupportedFeature,
}

pub trait NormalizerPort {
    fn normalize(
        &self,
        document: &SubscriptionDocument,
    ) -> Result<NormalizedConfiguration, ImportError>;
}

/// Reject oversized input before invoking any parser or adapter.
pub fn import_configuration(
    document: &SubscriptionDocument,
    normalizer: &dyn NormalizerPort,
) -> Result<NormalizedConfiguration, ImportError> {
    if document.bytes().len() > MAX_SUBSCRIPTION_BYTES {
        return Err(ImportError::PayloadTooLarge);
    }
    normalizer.normalize(document)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineError {
    EngineUnavailable,
    UnsupportedFeature,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineCapabilities {
    pub supported_protocols: &'static [ProtocolKind],
    pub can_connect: bool,
}

/// Engine integration belongs to an outer adapter; no raw Xray JSON crosses here.
pub trait EnginePort {
    fn capabilities(&self) -> EngineCapabilities;
    fn connection_state(&self) -> Result<ConnectionState, EngineError>;
    fn validate_configuration(
        &self,
        configuration: &NormalizedConfiguration,
    ) -> Result<(), EngineError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreparationError {
    Normalization(ImportError),
    Engine(EngineError),
}

/// Normalize and validate through ports before any outer layer accepts state.
pub fn prepare_configuration(
    document: &SubscriptionDocument,
    normalizer: &dyn NormalizerPort,
    engine: &dyn EnginePort,
) -> Result<NormalizedConfiguration, PreparationError> {
    let configuration =
        import_configuration(document, normalizer).map_err(PreparationError::Normalization)?;
    engine
        .validate_configuration(&configuration)
        .map_err(PreparationError::Engine)?;
    Ok(configuration)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

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
                    protocol: ProtocolKind::WireGuard,
                }],
            })
        }
    }

    struct RejectingEngine;

    impl EnginePort for RejectingEngine {
        fn capabilities(&self) -> EngineCapabilities {
            EngineCapabilities {
                supported_protocols: &[ProtocolKind::WireGuard],
                can_connect: true,
            }
        }

        fn connection_state(&self) -> Result<ConnectionState, EngineError> {
            Ok(ConnectionState::Disconnected)
        }

        fn validate_configuration(
            &self,
            configuration: &NormalizedConfiguration,
        ) -> Result<(), EngineError> {
            assert_eq!(configuration.profiles.len(), 1);
            Err(EngineError::UnsupportedFeature)
        }
    }

    #[test]
    fn normalized_input_is_rejected_when_engine_validation_fails() {
        let document = SubscriptionDocument::new(SubscriptionFormat::XrayJson, b"fixture".to_vec());
        assert_eq!(
            prepare_configuration(&document, &AcceptingNormalizer, &RejectingEngine),
            Err(PreparationError::Engine(EngineError::UnsupportedFeature))
        );
    }

    struct RejectingNormalizer {
        calls: Cell<usize>,
        error: ImportError,
    }

    impl NormalizerPort for RejectingNormalizer {
        fn normalize(
            &self,
            _document: &SubscriptionDocument,
        ) -> Result<NormalizedConfiguration, ImportError> {
            self.calls.set(self.calls.get() + 1);
            Err(self.error)
        }
    }

    #[test]
    fn oversized_document_never_reaches_the_normalizer() {
        let normalizer = RejectingNormalizer {
            calls: Cell::new(0),
            error: ImportError::NormalizerUnavailable,
        };
        let document = SubscriptionDocument::new(
            SubscriptionFormat::XrayJson,
            vec![0; MAX_SUBSCRIPTION_BYTES + 1],
        );
        assert_eq!(
            import_configuration(&document, &normalizer),
            Err(ImportError::PayloadTooLarge)
        );
        assert_eq!(normalizer.calls.get(), 0);
    }

    #[test]
    fn exact_limit_is_allowed_to_reach_the_normalizer() {
        let normalizer = RejectingNormalizer {
            calls: Cell::new(0),
            error: ImportError::NormalizerUnavailable,
        };
        let document = SubscriptionDocument::new(
            SubscriptionFormat::XrayJson,
            vec![0; MAX_SUBSCRIPTION_BYTES],
        );
        assert_eq!(
            import_configuration(&document, &normalizer),
            Err(ImportError::NormalizerUnavailable)
        );
        assert_eq!(normalizer.calls.get(), 1);
    }

    #[test]
    fn unavailable_and_unsupported_are_never_parse_success() {
        for error in [
            ImportError::NormalizerUnavailable,
            ImportError::UnsupportedFormat,
            ImportError::UnsupportedFeature,
        ] {
            let normalizer = RejectingNormalizer {
                calls: Cell::new(0),
                error,
            };
            let document = SubscriptionDocument::new(
                SubscriptionFormat::ShareLinkList,
                b"not a parsed configuration".to_vec(),
            );
            assert_eq!(import_configuration(&document, &normalizer), Err(error));
        }
    }
}
