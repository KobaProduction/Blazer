//! Explicitly unavailable adapters until verified implementations are integrated.

use vpn_application::{
    EngineCapabilities, EngineError, EnginePort, ImportError, NormalizerPort, SubscriptionDocument,
};
use vpn_domain::{ConnectionState, NormalizedConfiguration};

pub struct UnavailableEngine;

impl EnginePort for UnavailableEngine {
    fn capabilities(&self) -> EngineCapabilities {
        EngineCapabilities {
            supported_protocols: &[],
            can_connect: false,
        }
    }

    fn connection_state(&self) -> Result<ConnectionState, EngineError> {
        Err(EngineError::EngineUnavailable)
    }

    fn validate_configuration(
        &self,
        _configuration: &NormalizedConfiguration,
    ) -> Result<(), EngineError> {
        Err(EngineError::EngineUnavailable)
    }
}

pub struct UnavailableNormalizer;

impl NormalizerPort for UnavailableNormalizer {
    fn normalize(
        &self,
        _document: &SubscriptionDocument,
    ) -> Result<NormalizedConfiguration, ImportError> {
        Err(ImportError::NormalizerUnavailable)
    }
}
