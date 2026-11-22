//! Xray leastPing selection over one shared observation report.

use std::fmt;
use std::time::Duration;

use super::{ClientError, GraphGroup, GraphStrategy, GraphTarget, MAX_PROFILES, ProxyGraph};

#[derive(Clone, PartialEq, Eq)]
pub struct ProbeReport {
    pub tag: String,
    /// None is a completed failed observation; an absent record is unobserved.
    pub delay: Option<Duration>,
}

impl fmt::Debug for ProbeReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProbeReport")
            .field("alive", &self.delay.is_some())
            .finish_non_exhaustive()
    }
}

/// Preserve first-report arrival order when an existing subject is updated.
pub fn record_graph_probe(
    reports: &mut Vec<ProbeReport>,
    tag: &str,
    delay: Option<Duration>,
) -> Result<(), ClientError> {
    if let Some(report) = reports.iter_mut().find(|report| report.tag == tag) {
        report.delay = delay;
        return Ok(());
    }
    if reports.len() >= MAX_PROFILES {
        return Err(ClientError::PayloadTooLarge);
    }
    reports.push(ProbeReport {
        tag: tag.to_owned(),
        delay,
    });
    Ok(())
}

pub fn select_least_ping(
    graph: &ProxyGraph,
    group: &GraphGroup,
    reports: &[ProbeReport],
) -> Result<GraphTarget, ClientError> {
    let GraphStrategy::LeastPing { fallback } = &group.strategy else {
        return Err(ClientError::UnsupportedFeature);
    };
    let observer = graph
        .observatory
        .as_ref()
        .ok_or(ClientError::UnsupportedFeature)?;
    if reports.len() > MAX_PROFILES {
        return Err(ClientError::PayloadTooLarge);
    }
    let mut best_delay = 99_999_999u128;
    let mut selected = None;
    for report in reports {
        let Some(delay) = report.delay.filter(|_| {
            group.members.contains(&report.tag) && observer.members.contains(&report.tag)
        }) else {
            continue;
        };
        // Xray stores integer milliseconds and only replaces a strictly faster report.
        if delay.as_millis() < best_delay {
            best_delay = delay.as_millis();
            selected = Some(report.tag.clone());
        }
    }
    if let Some(tag) = selected.or_else(|| fallback.clone()) {
        return Ok(GraphTarget::Outbound(tag));
    }
    match &graph.default_target {
        GraphTarget::Outbound(_) => Ok(graph.default_target.clone()),
        GraphTarget::Group(_) => Err(ClientError::UnsupportedFeature),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::{GraphObservatory, GraphOutbound, GraphOutboundKind, Secret};
    use crate::routing::DnsPolicy;

    fn graph(fallback: Option<&str>) -> ProxyGraph {
        ProxyGraph {
            outbounds: ["a", "b", "global", "fallback"]
                .map(|tag| GraphOutbound {
                    tag: tag.into(),
                    kind: GraphOutboundKind::Direct,
                })
                .into(),
            groups: vec![GraphGroup {
                tag: "least".into(),
                members: vec!["a".into(), "b".into()],
                strategy: GraphStrategy::LeastPing {
                    fallback: fallback.map(str::to_owned),
                },
            }],
            rules: Vec::new(),
            default_target: GraphTarget::Outbound("global".into()),
            dns: DnsPolicy::Automatic,
            observatory: Some(GraphObservatory {
                members: vec!["a".into(), "b".into()],
                probe_url: Secret::new("http://probe.invalid/"),
                interval: Duration::from_secs(1),
                enable_concurrency: true,
            }),
        }
    }

    #[test]
    fn equal_millisecond_latencies_choose_first_report_arrival_not_outbound_order() {
        let graph = graph(None);
        let reports = vec![
            ProbeReport {
                tag: "b".into(),
                delay: Some(Duration::from_micros(1900)),
            },
            ProbeReport {
                tag: "a".into(),
                delay: Some(Duration::from_micros(1100)),
            },
        ];
        assert_eq!(
            select_least_ping(&graph, &graph.groups[0], &reports).unwrap(),
            GraphTarget::Outbound("b".into())
        );
    }

    #[test]
    fn failed_observation_replaces_alive_and_recovery_preserves_arrival_order() {
        let graph = graph(Some("fallback"));
        let mut reports = Vec::new();
        record_graph_probe(&mut reports, "a", Some(Duration::from_millis(5))).unwrap();
        record_graph_probe(&mut reports, "b", Some(Duration::from_millis(50))).unwrap();
        assert_eq!(
            select_least_ping(&graph, &graph.groups[0], &reports).unwrap(),
            GraphTarget::Outbound("a".into())
        );
        record_graph_probe(&mut reports, "a", None).unwrap();
        assert_eq!(
            select_least_ping(&graph, &graph.groups[0], &reports).unwrap(),
            GraphTarget::Outbound("b".into())
        );
        record_graph_probe(&mut reports, "b", None).unwrap();
        assert_eq!(
            select_least_ping(&graph, &graph.groups[0], &reports).unwrap(),
            GraphTarget::Outbound("fallback".into())
        );
        record_graph_probe(&mut reports, "a", Some(Duration::from_millis(5))).unwrap();
        assert_eq!(reports[0].tag, "a");
        assert_eq!(reports.len(), 2);
        assert_eq!(
            select_least_ping(&graph, &graph.groups[0], &reports).unwrap(),
            GraphTarget::Outbound("a".into())
        );
    }

    #[test]
    fn unobserved_unselected_or_failed_candidates_use_global_default_without_fallback() {
        let mut graph = graph(None);
        graph.observatory.as_mut().unwrap().members = vec!["b".into()];
        let reports = vec![
            ProbeReport {
                tag: "a".into(),
                delay: Some(Duration::ZERO),
            },
            ProbeReport {
                tag: "global".into(),
                delay: Some(Duration::ZERO),
            },
            ProbeReport {
                tag: "b".into(),
                delay: None,
            },
        ];
        assert_eq!(
            select_least_ping(&graph, &graph.groups[0], &reports).unwrap(),
            GraphTarget::Outbound("global".into())
        );
        assert_eq!(
            select_least_ping(&graph, &graph.groups[0], &[]).unwrap(),
            GraphTarget::Outbound("global".into())
        );
    }

    #[test]
    fn observations_do_not_leak_subject_tags_and_are_bounded() {
        let mut reports = Vec::new();
        for index in 0..MAX_PROFILES {
            record_graph_probe(&mut reports, &format!("private-{index}"), None).unwrap();
        }
        assert!(!format!("{reports:?}").contains("private-"));
        assert_eq!(
            record_graph_probe(&mut reports, "overflow", None),
            Err(ClientError::PayloadTooLarge)
        );
        record_graph_probe(&mut reports, "private-0", Some(Duration::ZERO)).unwrap();
        assert_eq!(reports.len(), MAX_PROFILES);
    }
}
