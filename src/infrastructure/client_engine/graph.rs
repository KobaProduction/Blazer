//! Translate the preserved provider graph into native BlazeCore records.

use std::collections::HashMap;

use serde_json::{Value, json};
use vpn_domain::client::{
    ClientError, GraphMatch, GraphNetwork, GraphOutboundKind, GraphStrategy, GraphTarget,
    ProxyGraph, ProxyProtocol, select_least_ping,
};
use vpn_domain::routing::{DnsPolicy, RouteAction, RoutingProfile};

use super::{config, routing};

const MAX_RENDERED_RULES: usize = 4096;

struct Names<'a> {
    outbounds: HashMap<&'a str, (String, &'a GraphOutboundKind)>,
    groups: HashMap<&'a str, String>,
}

impl<'a> Names<'a> {
    fn new(graph: &'a ProxyGraph) -> Self {
        Self {
            outbounds: graph
                .outbounds
                .iter()
                .enumerate()
                .map(|(index, outbound)| {
                    (
                        outbound.tag.as_str(),
                        (format!("blazer-graph-outbound-{index}"), &outbound.kind),
                    )
                })
                .collect(),
            groups: graph
                .groups
                .iter()
                .enumerate()
                .map(|(index, group)| (group.tag.as_str(), format!("blazer-graph-group-{index}")))
                .collect(),
        }
    }

    fn target(&self, target: &GraphTarget) -> Result<Option<String>, ClientError> {
        match target {
            GraphTarget::Outbound(tag) => {
                let (name, kind) = self
                    .outbounds
                    .get(tag.as_str())
                    .ok_or(ClientError::InvalidInput)?;
                Ok((!matches!(kind, GraphOutboundKind::Block)).then(|| name.clone()))
            }
            GraphTarget::Group(tag) => self
                .groups
                .get(tag.as_str())
                .cloned()
                .map(Some)
                .ok_or(ClientError::InvalidInput),
        }
    }

    fn member(&self, member: &str) -> Result<String, ClientError> {
        if self.outbounds.contains_key(member) {
            self.target(&GraphTarget::Outbound(member.to_owned()))?
                .ok_or(ClientError::UnsupportedFeature)
        } else {
            self.target(&GraphTarget::Group(member.to_owned()))?
                .ok_or(ClientError::UnsupportedFeature)
        }
    }
}

pub(super) fn validate(graph: &ProxyGraph) -> Result<(), ClientError> {
    graph.validate()?;
    for outbound in &graph.outbounds {
        if let GraphOutboundKind::Proxy(profile) = &outbound.kind {
            if matches!(profile.protocol, ProxyProtocol::Graph(_)) {
                return Err(ClientError::UnsupportedFeature);
            }
            config::validate(profile)?;
        }
    }
    let names = Names::new(graph);
    for group in &graph.groups {
        for member in &group.members {
            names.member(member)?;
        }
        if matches!(group.strategy, GraphStrategy::LeastPing { .. }) {
            names
                .target(&select_least_ping(graph, group, &[])?)?
                .ok_or(ClientError::UnsupportedFeature)?;
        }
    }
    Ok(())
}

pub(super) fn entry_chain(graph: &ProxyGraph) -> Result<Value, ClientError> {
    Names::new(graph)
        .target(&graph.default_target)?
        .map(Value::String)
        .ok_or(ClientError::UnsupportedFeature)
}

pub(super) fn configure(
    mut listener: Value,
    graph: &ProxyGraph,
    user_routing: Option<&RoutingProfile>,
) -> Result<Vec<Value>, ClientError> {
    validate(graph)?;
    let names = Names::new(graph);
    let mut document = outbound_records(graph, &names)?;
    document.extend(group_records(graph, &names)?);
    listener["rules"] = Value::Array(render_rules(graph, user_routing, &names)?);
    apply_dns(&mut listener, &graph.dns);
    if let Some(user_routing) = user_routing {
        apply_dns(&mut listener, &user_routing.input.dns);
    }
    document.push(listener);
    Ok(document)
}

fn outbound_records(graph: &ProxyGraph, names: &Names<'_>) -> Result<Vec<Value>, ClientError> {
    let mut records = Vec::new();
    for outbound in &graph.outbounds {
        let chain = match &outbound.kind {
            GraphOutboundKind::Proxy(profile) => config::client_chain(profile)?,
            GraphOutboundKind::Direct => json!({"protocol": {"type": "direct"}}),
            GraphOutboundKind::Block => continue,
        };
        let name = &names
            .outbounds
            .get(outbound.tag.as_str())
            .ok_or(ClientError::InvalidInput)?
            .0;
        records.push(json!({"client_group": name, "client_chain": chain}));
    }
    Ok(records)
}

fn group_records(graph: &ProxyGraph, names: &Names<'_>) -> Result<Vec<Value>, ClientError> {
    graph
        .groups
        .iter()
        .map(|group| {
            let mut tags = group.members.clone();
            let initial = selector_default(graph, group, names, &mut tags)?;
            let strategy = match &group.strategy {
                GraphStrategy::RoundRobin => "round-robin",
                GraphStrategy::LeastPing { .. } => "selector",
            };
            let members = tags
                .iter()
                .map(|member| names.member(member).map(|name| json!({"chain": [name]})))
                .collect::<Result<Vec<_>, _>>()?;
            let name = names
                .groups
                .get(group.tag.as_str())
                .ok_or(ClientError::InvalidInput)?;
            let mut record =
                json!({"client_group": name, "type": strategy, "client_chains": members});
            if let Some(initial) = initial {
                record["default"] = json!(initial);
            }
            Ok(record)
        })
        .collect()
}

fn selector_default(
    graph: &ProxyGraph,
    group: &vpn_domain::client::GraphGroup,
    names: &Names<'_>,
    tags: &mut Vec<String>,
) -> Result<Option<String>, ClientError> {
    if matches!(group.strategy, GraphStrategy::RoundRobin) {
        return Ok(None);
    }
    let GraphTarget::Outbound(tag) = select_least_ping(graph, group, &[])? else {
        return Err(ClientError::UnsupportedFeature);
    };
    if !tags.contains(&tag) {
        tags.push(tag.clone());
    }
    names.member(&tag).map(Some)
}

pub(super) fn member_name(graph: &ProxyGraph, tag: &str) -> Result<String, ClientError> {
    Names::new(graph).member(tag)
}

pub(super) fn group_name(index: usize) -> String {
    format!("blazer-graph-group-{index}")
}

pub(super) fn needs_observer(profile: &vpn_domain::client::ProxyProfile) -> bool {
    matches!(&profile.protocol, ProxyProtocol::Graph(graph) if graph.observatory.is_some() || graph.groups.iter().any(|group| matches!(group.strategy, GraphStrategy::LeastPing { .. })))
}

fn render_rules(
    graph: &ProxyGraph,
    user: Option<&RoutingProfile>,
    names: &Names<'_>,
) -> Result<Vec<Value>, ClientError> {
    let mut rules = Vec::new();
    if let Some(user) = user {
        user.input.validate()?;
        for rule in &user.input.rules {
            let rendered = routing::render_rule(rule);
            if rule.action == RouteAction::Proxy {
                append_provider_rules(&mut rules, graph, names, Some(&rendered["conditions"]))?;
            } else {
                push_rule(&mut rules, rendered)?;
            }
        }
    }
    append_provider_rules(&mut rules, graph, names, None)?;
    Ok(rules)
}

fn append_provider_rules(
    rules: &mut Vec<Value>,
    graph: &ProxyGraph,
    names: &Names<'_>,
    gate: Option<&Value>,
) -> Result<(), ClientError> {
    for rule in &graph.rules {
        let condition = condition(&rule.condition);
        let condition = gate.map_or_else(
            || condition.clone(),
            |gate| json!({"all": [gate, condition]}),
        );
        push_rule(rules, action(condition, &rule.target, names)?)?;
    }
    let fallback = gate
        .cloned()
        .unwrap_or_else(|| json!({"any_destination": true}));
    push_rule(rules, action(fallback, &graph.default_target, names)?)
}

fn push_rule(rules: &mut Vec<Value>, rule: Value) -> Result<(), ClientError> {
    if rules.len() >= MAX_RENDERED_RULES {
        return Err(ClientError::UnsupportedFeature);
    }
    rules.push(rule);
    Ok(())
}

fn action(condition: Value, target: &GraphTarget, names: &Names<'_>) -> Result<Value, ClientError> {
    Ok(match names.target(target)? {
        Some(chain) => {
            json!({"conditions": condition, "action": "allow", "client_chain": chain, "no_resolve": true})
        }
        None => json!({"conditions": condition, "action": "block", "no_resolve": true}),
    })
}

fn condition(value: &GraphMatch) -> Value {
    match value {
        GraphMatch::AnyDestination => json!({"any_destination": true}),
        GraphMatch::Domain(values) => json!({"domain": values}),
        GraphMatch::DomainSuffix(values) => json!({"domain_suffix": values}),
        GraphMatch::Cidr(values) => json!({"ip_cidr": values}),
        GraphMatch::Network(networks) => {
            json!({"network": networks.iter().map(|network| match network { GraphNetwork::Tcp => "tcp", GraphNetwork::Udp => "udp" }).collect::<Vec<_>>()})
        }
        GraphMatch::Any(conditions) => {
            json!({"any": conditions.iter().map(condition).collect::<Vec<_>>()})
        }
        GraphMatch::All(conditions) => {
            json!({"all": conditions.iter().map(condition).collect::<Vec<_>>()})
        }
    }
}

fn apply_dns(listener: &mut Value, policy: &DnsPolicy) {
    if let Some(server) = routing::dns_server(policy) {
        listener["dns"] = json!({"servers": [server]});
    }
}

#[cfg(test)]
mod tests;
