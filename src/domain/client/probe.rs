use std::time::Duration;

use super::{ClientError, TcpProbeEndpoint};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ProbeMethod {
    #[default]
    Automatic,
    Tcp,
    Icmp,
    Proxy,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ProbeHttpMethod {
    Get,
    #[default]
    Head,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ProbeStrategy {
    #[default]
    Last,
    Minimum,
    Average,
    Median,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeSettings {
    pub method: ProbeMethod,
    pub http_method: ProbeHttpMethod,
    pub url: String,
    pub timeout_ms: u32,
    pub attempts: u8,
    pub warmup: bool,
    pub strategy: ProbeStrategy,
}

impl Default for ProbeSettings {
    fn default() -> Self {
        Self {
            method: ProbeMethod::Automatic,
            http_method: ProbeHttpMethod::Head,
            url: "https://connectivitycheck.gstatic.com/generate_204".into(),
            timeout_ms: 5000,
            attempts: 2,
            warmup: true,
            strategy: ProbeStrategy::Last,
        }
    }
}

impl ProbeSettings {
    /// HTTPS latency is measured only after its TCP/TLS connection is established.
    /// The stored preference controls optional warm-up for plain HTTP.
    pub fn http_warmup_required(&self) -> bool {
        self.warmup || self.url.starts_with("https://")
    }

    pub fn validate(&self) -> Result<(), ClientError> {
        if !(250..=10_000).contains(&self.timeout_ms) || !(1..=5).contains(&self.attempts) {
            return Err(ClientError::InvalidInput);
        }
        if self.url.len() > 2048 || self.url.chars().any(char::is_whitespace) {
            return Err(ClientError::InvalidUrl);
        }
        let address = self
            .url
            .strip_prefix("https://")
            .or_else(|| self.url.strip_prefix("http://"))
            .ok_or(ClientError::InvalidUrl)?;
        let authority = address.split(['/', '?']).next().unwrap_or_default();
        if authority.is_empty()
            || authority.contains('@')
            || self.url.contains('#')
            || self.url.chars().any(char::is_control)
        {
            return Err(ClientError::InvalidUrl);
        }
        validate_authority(authority)
    }

    /// Samples contain measured attempts only; callers discard warm-up work.
    pub fn aggregate(&self, samples: &[Duration]) -> Result<Duration, ClientError> {
        let last = samples.last().copied().ok_or(ClientError::InvalidInput)?;
        match self.strategy {
            ProbeStrategy::Last => Ok(last),
            ProbeStrategy::Minimum => samples
                .iter()
                .copied()
                .min()
                .ok_or(ClientError::InvalidInput),
            ProbeStrategy::Average => Ok(average(samples)),
            ProbeStrategy::Median => {
                let mut sorted = samples.to_vec();
                sorted.sort_unstable();
                let middle = sorted.len() / 2;
                Ok(if sorted.len().is_multiple_of(2) {
                    average(&sorted[middle - 1..=middle])
                } else {
                    sorted[middle]
                })
            }
        }
    }
}

fn validate_authority(authority: &str) -> Result<(), ClientError> {
    let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
        let (host, suffix) = bracketed.split_once(']').ok_or(ClientError::InvalidUrl)?;
        host.parse::<std::net::Ipv6Addr>()
            .map_err(|_| ClientError::InvalidUrl)?;
        let port = if suffix.is_empty() {
            80
        } else {
            suffix
                .strip_prefix(':')
                .and_then(|port| port.parse().ok())
                .ok_or(ClientError::InvalidUrl)?
        };
        (host, port)
    } else {
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) => (host, port.parse().map_err(|_| ClientError::InvalidUrl)?),
            None => (authority, 80),
        };
        if host.contains(':') {
            return Err(ClientError::InvalidUrl);
        }
        (host, port)
    };
    TcpProbeEndpoint::new(host, port)
        .map(|_| ())
        .ok_or(ClientError::InvalidUrl)
}

fn average(samples: &[Duration]) -> Duration {
    let total = samples
        .iter()
        .fold(0u128, |sum, value| sum.saturating_add(value.as_nanos()));
    let nanos = total / samples.len() as u128;
    Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_warmup_is_mandatory_while_http_remains_configurable() {
        let mut settings = ProbeSettings {
            warmup: false,
            ..Default::default()
        };
        assert!(settings.http_warmup_required());
        assert_eq!(settings.http_method, ProbeHttpMethod::Head);
        assert_eq!(settings.attempts, 2);
        settings.url = "http://probe.example.test/check".into();
        assert!(!settings.http_warmup_required());
        settings.warmup = true;
        assert!(settings.http_warmup_required());
    }

    #[test]
    fn aggregation_obeys_the_selected_strategy_and_even_median() {
        let samples = [90, 10, 30, 70].map(Duration::from_millis);
        for (strategy, expected) in [
            (ProbeStrategy::Last, 70),
            (ProbeStrategy::Minimum, 10),
            (ProbeStrategy::Average, 50),
            (ProbeStrategy::Median, 50),
        ] {
            let settings = ProbeSettings {
                strategy,
                ..ProbeSettings::default()
            };
            assert_eq!(
                settings.aggregate(&samples),
                Ok(Duration::from_millis(expected))
            );
            assert_eq!(settings.aggregate(&[]), Err(ClientError::InvalidInput));
        }
    }

    #[test]
    fn probe_settings_reject_invalid_bounds_and_non_http_resources() {
        for url in [
            "",
            "https://",
            "file:///tmp/resource",
            "https://user:password@host/path",
            "https://host/#fragment",
            "https://[",
            "https://host:notaport",
            "https://host:0",
            "https://host:99999",
            "https://2001:db8::1:443/",
        ] {
            assert!(
                ProbeSettings {
                    url: url.into(),
                    ..ProbeSettings::default()
                }
                .validate()
                .is_err()
            );
        }
        for (timeout_ms, attempts) in [(0, 1), (10_001, 1), (5000, 0), (5000, 6)] {
            assert!(
                ProbeSettings {
                    timeout_ms,
                    attempts,
                    ..ProbeSettings::default()
                }
                .validate()
                .is_err()
            );
        }
        assert!(ProbeSettings::default().validate().is_ok());
        for url in [
            "https://[::1]:8443/status",
            "http://localhost:1234/path?q=foo",
        ] {
            assert!(
                ProbeSettings {
                    url: url.into(),
                    ..ProbeSettings::default()
                }
                .validate()
                .is_ok()
            );
        }
    }
}
